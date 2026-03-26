use crate::{Result, SandboxError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use types::{CallId, RunId, SessionId, TurnId};

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
    pub protected_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    Off,
    AllowDomains(Vec<String>),
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEscapePolicy {
    Deny,
    HostManaged,
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

        let mut roots = vec![scope.workspace_root.clone()];
        if let Some(worktree_root) = scope.worktree_root.clone() {
            roots.push(worktree_root);
        }
        roots.extend(scope.additional_roots.iter().cloned());
        let roots = dedup_paths(roots);
        let protected_paths = dedup_paths(
            roots
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
                readable_roots: roots.clone(),
                writable_roots: roots,
                protected_paths,
            },
            network: NetworkPolicy::Off,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOrigin {
    BashTool,
    HookCommand,
    McpStdioServer { server_name: String },
    HostUtility { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RuntimeScope {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
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
    let mut roots = policy.filesystem.writable_roots.clone();
    roots.extend(policy.filesystem.readable_roots.iter().cloned());
    dedup_paths(roots)
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

#[cfg(test)]
mod tests {
    use super::{HostEscapePolicy, NetworkPolicy, SandboxMode, SandboxPolicy, SandboxScope};
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
}
