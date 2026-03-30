use crate::{Result, SandboxError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use types::{AgentSessionId, CallId, McpServerName, SessionId, TurnId};

const DEFAULT_PROTECTED_DIRS: &[&str] = &[".git", ".nanoclaw", ".codex"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub executable_roots: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    Off,
    AllowDomains(Vec<String>),
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GrantedFilesystemPermissions {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
}

impl GrantedFilesystemPermissions {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read_roots.is_empty() && self.write_roots.is_empty()
    }

    pub fn merge_in_place(&mut self, other: &Self) {
        self.read_roots = union_paths(&self.read_roots, &other.read_roots);
        self.write_roots = union_paths(&self.write_roots, &other.write_roots);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantedNetworkPermissions {
    AllowDomains(Vec<String>),
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GrantedPermissionProfile {
    pub file_system: GrantedFilesystemPermissions,
    pub network: Option<GrantedNetworkPermissions>,
}

impl GrantedPermissionProfile {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.file_system.is_empty() && self.network.is_none()
    }

    pub fn merge_in_place(&mut self, other: &Self) {
        self.file_system.merge_in_place(&other.file_system);
        self.network = merge_granted_network(self.network.as_ref(), other.network.as_ref());
    }

    #[must_use]
    pub fn merged(&self, other: &Self) -> Self {
        let mut merged = self.clone();
        merged.merge_in_place(other);
        merged
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEscapePolicy {
    Deny,
    HostManaged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesystemAccess {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub filesystem: FilesystemPolicy,
    pub network: NetworkPolicy,
    pub host_escape: HostEscapePolicy,
    pub fail_if_unavailable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SandboxScope {
    pub workspace_root: PathBuf,
    pub worktree_root: Option<PathBuf>,
    pub additional_roots: Vec<PathBuf>,
    pub read_only_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub exec_roots: Vec<PathBuf>,
    pub network_policy: Option<NetworkPolicy>,
    pub workspace_only: bool,
}

impl SandboxPolicy {
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            mode: SandboxMode::DangerFullAccess,
            filesystem: FilesystemPolicy::default(),
            network: NetworkPolicy::Full,
            host_escape: HostEscapePolicy::HostManaged,
            fail_if_unavailable: false,
        }
    }

    #[must_use]
    pub fn recommended_for_scope(scope: &SandboxScope) -> Self {
        if !scope.workspace_only {
            return Self::permissive();
        }

        let mut readable_roots = if scope.read_only_roots.is_empty()
            && scope.writable_roots.is_empty()
            && scope.exec_roots.is_empty()
        {
            let mut roots = vec![scope.workspace_root.clone()];
            if let Some(worktree_root) = scope.worktree_root.clone() {
                roots.push(worktree_root);
            }
            roots.extend(scope.additional_roots.iter().cloned());
            dedup_paths(roots)
        } else {
            let mut roots = scope.read_only_roots.clone();
            roots.extend(scope.writable_roots.iter().cloned());
            roots.extend(scope.exec_roots.iter().cloned());
            dedup_paths(roots)
        };
        let writable_roots = if scope.writable_roots.is_empty()
            && scope.read_only_roots.is_empty()
            && scope.exec_roots.is_empty()
        {
            readable_roots.clone()
        } else {
            dedup_paths(scope.writable_roots.clone())
        };
        let executable_roots = dedup_paths(scope.exec_roots.clone());
        let protected_paths = dedup_paths(
            readable_roots
                .iter()
                .flat_map(|root| {
                    DEFAULT_PROTECTED_DIRS
                        .iter()
                        .map(move |name| root.join(name))
                })
                .collect(),
        );

        Self {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: std::mem::take(&mut readable_roots),
                writable_roots,
                executable_roots,
                protected_paths,
            },
            network: scope.network_policy.clone().unwrap_or(NetworkPolicy::Off),
            host_escape: HostEscapePolicy::Deny,
            // Hosts can tighten this later once every platform backend exists.
            // The substrate should not silently claim fail-closed isolation on a
            // platform where it only knows how to fall back to host execution.
            fail_if_unavailable: false,
        }
    }

    #[must_use]
    pub fn with_fail_if_unavailable(mut self, fail_if_unavailable: bool) -> Self {
        self.fail_if_unavailable = fail_if_unavailable;
        self
    }

    #[must_use]
    pub fn requires_enforcement(&self) -> bool {
        !matches!(self.mode, SandboxMode::DangerFullAccess)
            || !matches!(self.network, NetworkPolicy::Full)
            || !self.filesystem.readable_roots.is_empty()
            || !self.filesystem.writable_roots.is_empty()
            || !self.filesystem.executable_roots.is_empty()
            || !self.filesystem.protected_paths.is_empty()
    }
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self::permissive()
    }
}

impl SandboxScope {
    #[must_use]
    pub fn recommended_policy(&self) -> SandboxPolicy {
        SandboxPolicy::recommended_for_scope(self)
    }
}

pub fn assert_filesystem_access(
    policy: &SandboxPolicy,
    path: &Path,
    access: FilesystemAccess,
) -> Result<()> {
    let filesystem = canonicalize_filesystem_policy(&policy.filesystem)?;
    let path = canonicalize_policy_path(path)?;

    if matches!(access, FilesystemAccess::Write)
        && path_is_inside_any_root(&path, &filesystem.protected_paths)
    {
        return Err(SandboxError::invalid_state(format!(
            "sandbox denies write access to protected path {}",
            path.display()
        )));
    }

    let allowed_roots = match access {
        FilesystemAccess::Read => accessible_roots_for_filesystem(&filesystem),
        FilesystemAccess::Write => filesystem.writable_roots,
        FilesystemAccess::Execute => filesystem.executable_roots,
    };
    if filesystem_access_is_unrestricted(policy, access, &allowed_roots)
        || path_is_inside_any_root(&path, &allowed_roots)
    {
        return Ok(());
    }

    let action = match access {
        FilesystemAccess::Read => "read",
        FilesystemAccess::Write => "write",
        FilesystemAccess::Execute => "execute",
    };
    let allowed = allowed_roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(SandboxError::invalid_state(format!(
        "sandbox denies {action} access to {}; allowed roots: [{allowed}]",
        path.display()
    )))
}

pub fn apply_granted_permission_profile(
    base: &SandboxPolicy,
    granted: &GrantedPermissionProfile,
) -> Result<SandboxPolicy> {
    if granted.is_empty() {
        return Ok(base.clone());
    }

    let filesystem = canonicalize_filesystem_policy(&base.filesystem)?;
    let granted_filesystem = canonicalize_granted_filesystem_permissions(&granted.file_system)?;
    let writable_roots = widen_write_roots(base, &filesystem.writable_roots, &granted_filesystem);

    Ok(SandboxPolicy {
        mode: widened_mode(base, &writable_roots),
        filesystem: FilesystemPolicy {
            readable_roots: widen_read_roots(base, &filesystem.readable_roots, &granted_filesystem),
            writable_roots,
            executable_roots: filesystem.executable_roots,
            protected_paths: filesystem.protected_paths,
        },
        network: widen_network_policy(base, granted.network.as_ref()),
        host_escape: base.host_escape.clone(),
        fail_if_unavailable: base.fail_if_unavailable,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOrigin {
    BashTool,
    HookCommand,
    McpStdioServer { server_name: McpServerName },
    HostUtility { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RuntimeScope {
    pub session_id: Option<SessionId>,
    pub agent_session_id: Option<AgentSessionId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<CallId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStdio {
    Inherit,
    Null,
    Piped,
}

impl ProcessStdio {
    pub fn into_stdio(self) -> Stdio {
        match self {
            Self::Inherit => Stdio::inherit(),
            Self::Null => Stdio::null(),
            Self::Piped => Stdio::piped(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub stdin: ProcessStdio,
    pub stdout: ProcessStdio,
    pub stderr: ProcessStdio,
    pub kill_on_drop: bool,
    pub origin: ExecutionOrigin,
    pub runtime_scope: RuntimeScope,
    pub sandbox_policy: SandboxPolicy,
}

pub trait ProcessExecutor: Send + Sync {
    fn prepare(&self, request: ExecRequest) -> Result<Command>;
}

#[derive(Default)]
pub struct HostProcessExecutor;

impl ProcessExecutor for HostProcessExecutor {
    fn prepare(&self, request: ExecRequest) -> Result<Command> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(request.stdin.into_stdio())
            .stdout(request.stdout.into_stdio())
            .stderr(request.stderr.into_stdio())
            .kill_on_drop(request.kill_on_drop);
        if let Some(cwd) = request.cwd {
            command.current_dir(cwd);
        }
        if !request.env.is_empty() {
            command.envs(request.env);
        }

        let _ = request.origin;
        let _ = request.runtime_scope;
        let _ = request.sandbox_policy;

        Ok(command)
    }
}

pub(crate) fn resolve_effective_cwd(
    cwd: Option<PathBuf>,
    policy: &SandboxPolicy,
) -> Result<Option<PathBuf>> {
    let roots = accessible_roots(policy);
    match cwd {
        Some(cwd) if roots.is_empty() || path_is_inside_any_root(&cwd, &roots) => Ok(Some(cwd)),
        Some(cwd) => Err(SandboxError::invalid_state(format!(
            "sandboxed process cwd {} is outside the configured sandbox roots",
            cwd.display()
        ))),
        None if roots.is_empty() => Ok(None),
        None => Ok(Some(
            roots
                .first()
                .expect("sandbox roots should be non-empty")
                .clone(),
        )),
    }
}

pub(crate) fn canonicalize_filesystem_policy(
    policy: &FilesystemPolicy,
) -> Result<FilesystemPolicy> {
    Ok(FilesystemPolicy {
        readable_roots: dedup_paths(
            policy
                .readable_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
        writable_roots: dedup_paths(
            policy
                .writable_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
        executable_roots: dedup_paths(
            policy
                .executable_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
        protected_paths: dedup_paths(
            policy
                .protected_paths
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}

pub(crate) fn canonicalize_optional_path(path: Option<&Path>) -> Result<Option<PathBuf>> {
    path.map(canonicalize_policy_path).transpose()
}

pub fn normalize_granted_permission_path(path: &Path) -> Result<PathBuf> {
    canonicalize_policy_path(path)
}

pub(crate) fn canonicalize_policy_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(|source| {
            SandboxError::invalid_state(format!(
                "failed to canonicalize sandbox path {}: {source}",
                path.display()
            ))
        });
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut existing_ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !existing_ancestor.exists() {
        let Some(file_name) = existing_ancestor.file_name() else {
            return Err(SandboxError::invalid_state(format!(
                "sandbox path {} has no existing ancestor",
                absolute.display()
            )));
        };
        suffix.push(file_name.to_os_string());
        existing_ancestor = existing_ancestor.parent().ok_or_else(|| {
            SandboxError::invalid_state(format!(
                "sandbox path {} has no parent",
                absolute.display()
            ))
        })?;
    }

    let mut normalized = std::fs::canonicalize(existing_ancestor).map_err(|source| {
        SandboxError::invalid_state(format!(
            "failed to canonicalize sandbox ancestor {}: {source}",
            existing_ancestor.display()
        ))
    })?;
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

pub(crate) fn accessible_roots(policy: &SandboxPolicy) -> Vec<PathBuf> {
    accessible_roots_for_filesystem(&policy.filesystem)
}

pub(crate) fn path_is_inside_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = BTreeSet::new();
    for path in paths {
        unique.insert(path);
    }
    unique.into_iter().collect()
}

fn accessible_roots_for_filesystem(filesystem: &FilesystemPolicy) -> Vec<PathBuf> {
    let mut roots = filesystem.writable_roots.clone();
    roots.extend(filesystem.readable_roots.iter().cloned());
    roots.extend(filesystem.executable_roots.iter().cloned());
    dedup_paths(roots)
}

fn canonicalize_granted_filesystem_permissions(
    permissions: &GrantedFilesystemPermissions,
) -> Result<GrantedFilesystemPermissions> {
    Ok(GrantedFilesystemPermissions {
        read_roots: dedup_paths(
            permissions
                .read_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
        write_roots: dedup_paths(
            permissions
                .write_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}

fn widened_mode(base: &SandboxPolicy, writable_roots: &[PathBuf]) -> SandboxMode {
    match base.mode {
        SandboxMode::DangerFullAccess => SandboxMode::DangerFullAccess,
        SandboxMode::WorkspaceWrite => SandboxMode::WorkspaceWrite,
        SandboxMode::ReadOnly if writable_roots.is_empty() => SandboxMode::ReadOnly,
        SandboxMode::ReadOnly => SandboxMode::WorkspaceWrite,
    }
}

fn widen_read_roots(
    base: &SandboxPolicy,
    current: &[PathBuf],
    granted: &GrantedFilesystemPermissions,
) -> Vec<PathBuf> {
    if matches!(base.mode, SandboxMode::DangerFullAccess) && current.is_empty() {
        return Vec::new();
    }
    union_paths(
        current,
        &union_paths(&granted.read_roots, &granted.write_roots),
    )
}

fn widen_write_roots(
    base: &SandboxPolicy,
    current: &[PathBuf],
    granted: &GrantedFilesystemPermissions,
) -> Vec<PathBuf> {
    if matches!(base.mode, SandboxMode::DangerFullAccess) && current.is_empty() {
        return Vec::new();
    }
    union_paths(current, &granted.write_roots)
}

fn widen_network_policy(
    base: &SandboxPolicy,
    granted: Option<&GrantedNetworkPermissions>,
) -> NetworkPolicy {
    match (&base.network, granted) {
        (NetworkPolicy::Full, _) => NetworkPolicy::Full,
        (policy, None) => policy.clone(),
        (_, Some(GrantedNetworkPermissions::Full)) => NetworkPolicy::Full,
        (NetworkPolicy::Off, Some(GrantedNetworkPermissions::AllowDomains(domains))) => {
            NetworkPolicy::AllowDomains(domains.clone())
        }
        (
            NetworkPolicy::AllowDomains(existing),
            Some(GrantedNetworkPermissions::AllowDomains(domains)),
        ) => NetworkPolicy::AllowDomains(
            existing
                .iter()
                .chain(domains.iter())
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
    }
}

fn merge_granted_network(
    left: Option<&GrantedNetworkPermissions>,
    right: Option<&GrantedNetworkPermissions>,
) -> Option<GrantedNetworkPermissions> {
    match (left, right) {
        (Some(GrantedNetworkPermissions::Full), _) | (_, Some(GrantedNetworkPermissions::Full)) => {
            Some(GrantedNetworkPermissions::Full)
        }
        (
            Some(GrantedNetworkPermissions::AllowDomains(left_domains)),
            Some(GrantedNetworkPermissions::AllowDomains(right_domains)),
        ) => Some(GrantedNetworkPermissions::AllowDomains(
            left_domains
                .iter()
                .chain(right_domains.iter())
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        )),
        (Some(network), None) | (None, Some(network)) => Some(network.clone()),
        (None, None) => None,
    }
}

fn filesystem_access_is_unrestricted(
    policy: &SandboxPolicy,
    access: FilesystemAccess,
    allowed_roots: &[PathBuf],
) -> bool {
    allowed_roots.is_empty()
        && matches!(policy.mode, SandboxMode::DangerFullAccess)
        && matches!(access, FilesystemAccess::Read | FilesystemAccess::Write)
}

fn union_paths(left: &[PathBuf], right: &[PathBuf]) -> Vec<PathBuf> {
    left.iter()
        .chain(right.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        FilesystemAccess, GrantedFilesystemPermissions, GrantedNetworkPermissions,
        GrantedPermissionProfile, HostEscapePolicy, NetworkPolicy, SandboxMode, SandboxPolicy,
        SandboxScope, apply_granted_permission_profile, assert_filesystem_access,
    };
    use tempfile::tempdir;

    #[test]
    fn recommended_policy_for_workspace_scope_is_workspace_write_and_network_off() {
        let workspace = tempdir().unwrap();
        let extra = tempdir().unwrap();
        let scope = SandboxScope {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            additional_roots: vec![extra.path().to_path_buf()],
            workspace_only: true,
            ..Default::default()
        };

        let policy = SandboxPolicy::recommended_for_scope(&scope);

        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(policy.network, NetworkPolicy::Off);
        assert_eq!(policy.host_escape, HostEscapePolicy::Deny);
        assert!(
            policy
                .filesystem
                .writable_roots
                .contains(&workspace.path().to_path_buf())
        );
        assert!(
            policy
                .filesystem
                .writable_roots
                .contains(&extra.path().to_path_buf())
        );
        assert!(
            policy
                .filesystem
                .protected_paths
                .contains(&workspace.path().join(".git"))
        );
        assert!(
            policy
                .filesystem
                .protected_paths
                .contains(&workspace.path().join(".nanoclaw"))
        );
    }

    #[test]
    fn recommended_policy_is_permissive_when_workspace_only_is_disabled() {
        let workspace = tempdir().unwrap();
        let scope = SandboxScope {
            workspace_root: workspace.path().to_path_buf(),
            workspace_only: false,
            ..Default::default()
        };

        assert_eq!(
            SandboxPolicy::recommended_for_scope(&scope),
            SandboxPolicy::permissive()
        );
    }

    #[test]
    fn write_access_rejects_protected_paths() {
        let workspace = tempdir().unwrap();
        let scope = SandboxScope {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };
        let policy = SandboxPolicy::recommended_for_scope(&scope);
        std::fs::create_dir_all(workspace.path().join(".nanoclaw")).unwrap();

        let err = assert_filesystem_access(
            &policy,
            &workspace.path().join(".nanoclaw/state.toml"),
            FilesystemAccess::Write,
        )
        .unwrap_err();
        assert!(err.to_string().contains("protected path"));
    }

    #[test]
    fn read_access_allows_protected_paths_inside_workspace_roots() {
        let workspace = tempdir().unwrap();
        let scope = SandboxScope {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };
        let policy = SandboxPolicy::recommended_for_scope(&scope);
        std::fs::create_dir_all(workspace.path().join(".nanoclaw")).unwrap();

        assert!(
            assert_filesystem_access(
                &policy,
                &workspace.path().join(".nanoclaw/state.toml"),
                FilesystemAccess::Read,
            )
            .is_ok()
        );
    }

    #[test]
    fn read_only_policy_denies_writes_when_no_write_roots_exist() {
        let workspace = tempdir().unwrap();
        let policy = SandboxPolicy {
            mode: SandboxMode::ReadOnly,
            filesystem: super::FilesystemPolicy {
                readable_roots: vec![workspace.path().to_path_buf()],
                writable_roots: Vec::new(),
                executable_roots: Vec::new(),
                protected_paths: Vec::new(),
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: false,
        };

        let error = assert_filesystem_access(
            &policy,
            &workspace.path().join("file.txt"),
            FilesystemAccess::Write,
        )
        .unwrap_err();
        assert!(error.to_string().contains("sandbox denies write access"));
    }

    #[test]
    fn granted_permissions_widen_restrictive_policies() {
        let workspace = tempdir().unwrap();
        let extra = tempdir().unwrap();
        let base = SandboxPolicy {
            mode: SandboxMode::ReadOnly,
            filesystem: super::FilesystemPolicy {
                readable_roots: vec![workspace.path().to_path_buf()],
                writable_roots: Vec::new(),
                executable_roots: Vec::new(),
                protected_paths: vec![workspace.path().join(".git")],
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: false,
        };

        let widened = apply_granted_permission_profile(
            &base,
            &GrantedPermissionProfile {
                file_system: GrantedFilesystemPermissions {
                    read_roots: vec![extra.path().to_path_buf()],
                    write_roots: vec![extra.path().to_path_buf()],
                },
                network: Some(GrantedNetworkPermissions::AllowDomains(vec![
                    "example.com".to_string(),
                ])),
            },
        )
        .unwrap();

        assert_eq!(widened.mode, SandboxMode::WorkspaceWrite);
        assert!(
            widened
                .filesystem
                .readable_roots
                .contains(&extra.path().to_path_buf())
        );
        assert!(
            widened
                .filesystem
                .writable_roots
                .contains(&extra.path().to_path_buf())
        );
        assert_eq!(
            widened.network,
            NetworkPolicy::AllowDomains(vec!["example.com".to_string()])
        );
    }
}
