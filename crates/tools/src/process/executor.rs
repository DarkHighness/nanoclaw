use crate::{Result, ToolError, ToolExecutionContext};
#[cfg(target_os = "linux")]
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_os = "linux")]
use std::hash::{Hash, Hasher};
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
    pub fn recommended_for_context(ctx: &ToolExecutionContext) -> Self {
        if !ctx.workspace_only {
            return Self::permissive();
        }

        let mut roots = vec![ctx.workspace_root.clone()];
        if let Some(worktree_root) = ctx.worktree_root.clone() {
            roots.push(worktree_root);
        }
        roots.extend(ctx.additional_roots.iter().cloned());
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
        // The current substrate still launches local processes on the host by
        // default. Keeping that posture explicit avoids pretending there is a
        // hard boundary before an enforcing backend is wired in.
        Self::permissive()
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
    pub(super) fn into_stdio(self) -> Stdio {
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

        // Phase 1 centralized child-process construction under one boundary.
        // The permissive executor intentionally keeps current behavior while
        // still carrying origin and policy metadata through the same request
        // shape the enforcing backends consume.
        let _ = request.origin;
        let _ = request.runtime_scope;
        let _ = request.sandbox_policy;

        Ok(command)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BackendAvailability {
    macos_seatbelt: Option<PathBuf>,
    linux_bwrap: Option<PathBuf>,
}

#[derive(Default)]
pub struct ManagedPolicyProcessExecutor {
    host: HostProcessExecutor,
}

impl ManagedPolicyProcessExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[must_use]
pub fn platform_sandbox_backend_available() -> bool {
    let availability = detect_available_backends();
    #[cfg(target_os = "macos")]
    {
        return availability.macos_seatbelt.is_some();
    }
    #[cfg(target_os = "linux")]
    {
        return availability.linux_bwrap.is_some();
    }
    #[allow(unreachable_code)]
    false
}

impl ProcessExecutor for ManagedPolicyProcessExecutor {
    fn prepare(&self, request: ExecRequest) -> Result<Command> {
        if !request.sandbox_policy.requires_enforcement() {
            return self.host.prepare(request);
        }

        prepare_with_available_backends(request, detect_available_backends())
    }
}

fn prepare_with_available_backends(
    mut request: ExecRequest,
    availability: BackendAvailability,
) -> Result<Command> {
    if matches!(
        request.sandbox_policy.network,
        NetworkPolicy::AllowDomains(_)
    ) && !allow_domains_backend_available(&availability)
    {
        return Err(ToolError::invalid_state(
            "domain-scoped network policy requires a compatible enforcing sandbox backend",
        ));
    }
    attach_allow_domains_proxy_support(&mut request)?;

    #[cfg(target_os = "macos")]
    if let Some(path) = availability.macos_seatbelt.as_deref() {
        return super::executor_macos::prepare_macos_seatbelt_command(request, path);
    }
    #[cfg(target_os = "linux")]
    if let Some(path) = availability.linux_bwrap.as_deref() {
        return super::executor_linux::prepare_linux_bwrap_command(request, path);
    }

    if request.sandbox_policy.fail_if_unavailable {
        Err(ToolError::invalid_state(
            "sandbox policy requires an enforcing backend, but no compatible backend is available",
        ))
    } else {
        HostProcessExecutor.prepare(request)
    }
}

fn allow_domains_backend_available(availability: &BackendAvailability) -> bool {
    #[cfg(target_os = "macos")]
    {
        return availability.macos_seatbelt.is_some();
    }
    #[cfg(target_os = "linux")]
    {
        return availability.linux_bwrap.is_some();
    }
    #[allow(unreachable_code)]
    false
}

fn attach_allow_domains_proxy_support(request: &mut ExecRequest) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        maybe_attach_macos_allow_domains_proxy(request)?;
    }
    #[cfg(target_os = "linux")]
    {
        maybe_attach_linux_allow_domains_proxy(request)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn maybe_attach_macos_allow_domains_proxy(request: &mut ExecRequest) -> Result<()> {
    let NetworkPolicy::AllowDomains(domains) = &request.sandbox_policy.network else {
        return Ok(());
    };
    if super::executor_macos::has_allow_domains_proxy_config(&request.env) {
        return Ok(());
    }
    let allowlist = super::network_proxy::DomainAllowlist::new(domains.clone())
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
    let endpoint = super::network_proxy::start_retained_proxy(
        super::network_proxy::ProxyConfig::localhost(allowlist),
    )
    .map_err(|error| ToolError::invalid_state(error.to_string()))?;
    request.env.extend(endpoint.env_vars());
    Ok(())
}

#[cfg(target_os = "linux")]
fn maybe_attach_linux_allow_domains_proxy(request: &mut ExecRequest) -> Result<()> {
    let NetworkPolicy::AllowDomains(domains) = &request.sandbox_policy.network else {
        return Ok(());
    };
    if request
        .env
        .contains_key(super::executor_linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV)
    {
        return Ok(());
    }

    let allowlist = super::network_proxy::DomainAllowlist::new(domains.clone())
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
    // The retained proxy cache is keyed by normalized allowlist semantics, so
    // the Linux socket path has to follow the same normalization or long-lived
    // hosts will leak duplicate proxies for reordered/case-varied domain lists.
    let socket_path = default_linux_allow_domains_socket_path(allowlist.domains());
    let endpoint = super::network_proxy::start_retained_proxy(super::network_proxy::ProxyConfig {
        allowlist,
        bind: super::network_proxy::ProxyBindTarget::UnixSocket(socket_path.clone()),
    })
    .map_err(|error| ToolError::invalid_state(error.to_string()))?;
    let super::network_proxy::ProxyEndpoint::UnixSocket(host_socket_path) = endpoint else {
        return Err(ToolError::invalid_state(
            "Linux allow-domains proxy must be a Unix-socket endpoint",
        ));
    };

    request.env.insert(
        super::executor_linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV.to_string(),
        host_socket_path.display().to_string(),
    );
    request.env.insert(
        super::executor_linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV.to_string(),
        host_socket_path.display().to_string(),
    );
    request.env.insert(
        super::executor_linux::LINUX_ALLOW_DOMAINS_PROXY_URL_ENV.to_string(),
        format!(
            "socks5h://127.0.0.1:{}",
            super::executor_linux::LINUX_ALLOW_DOMAINS_PROXY_BRIDGE_PORT
        ),
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn default_linux_allow_domains_socket_path(domains: &[String]) -> PathBuf {
    let mut normalized = domains
        .iter()
        .map(|domain| domain.trim().trim_matches('.').to_ascii_lowercase())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    let hash = hasher.finish();
    std::env::temp_dir().join(format!("nanoclaw-proxy-{}-{hash}.sock", std::process::id()))
}

fn detect_available_backends() -> BackendAvailability {
    let mut availability = BackendAvailability::default();
    #[cfg(target_os = "macos")]
    {
        availability.macos_seatbelt = super::executor_macos::sandbox_exec_path();
    }
    #[cfg(target_os = "linux")]
    {
        availability.linux_bwrap = super::executor_linux::find_bwrap_executable();
    }
    availability
}

pub(super) fn resolve_effective_cwd(
    cwd: Option<PathBuf>,
    policy: &SandboxPolicy,
) -> Result<Option<PathBuf>> {
    let roots = accessible_roots(policy);
    match cwd {
        Some(cwd) if roots.is_empty() || path_is_inside_any_root(&cwd, &roots) => Ok(Some(cwd)),
        Some(cwd) => Err(ToolError::invalid_state(format!(
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

pub(super) fn canonicalize_filesystem_policy(
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

pub(super) fn canonicalize_optional_path(path: Option<&Path>) -> Result<Option<PathBuf>> {
    path.map(canonicalize_policy_path).transpose()
}

pub(super) fn canonicalize_policy_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(|source| {
            ToolError::invalid_state(format!(
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
            return Err(ToolError::invalid_state(format!(
                "sandbox path {} has no existing ancestor",
                absolute.display()
            )));
        };
        suffix.push(file_name.to_os_string());
        existing_ancestor = existing_ancestor.parent().ok_or_else(|| {
            ToolError::invalid_state(format!("sandbox path {} has no parent", absolute.display()))
        })?;
    }

    let mut normalized = std::fs::canonicalize(existing_ancestor).map_err(|source| {
        ToolError::invalid_state(format!(
            "failed to canonicalize sandbox ancestor {}: {source}",
            existing_ancestor.display()
        ))
    })?;
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

pub(super) fn accessible_roots(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut roots = policy.filesystem.writable_roots.clone();
    roots.extend(policy.filesystem.readable_roots.iter().cloned());
    dedup_paths(roots)
}

pub(super) fn path_is_inside_any_root(path: &Path, roots: &[PathBuf]) -> bool {
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
    use super::{
        BackendAvailability, ExecRequest, ExecutionOrigin, FilesystemPolicy, HostEscapePolicy,
        ManagedPolicyProcessExecutor, NetworkPolicy, ProcessExecutor, ProcessStdio, RuntimeScope,
        SandboxMode, SandboxPolicy, prepare_with_available_backends,
    };
    use crate::ToolExecutionContext;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn recommended_policy_for_workspace_context_is_workspace_write_and_network_off() {
        let workspace = tempdir().unwrap();
        let extra = tempdir().unwrap();
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            additional_roots: vec![extra.path().to_path_buf()],
            workspace_only: true,
            ..Default::default()
        };

        let policy = SandboxPolicy::recommended_for_context(&context);

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
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            workspace_only: false,
            ..Default::default()
        };

        assert_eq!(
            SandboxPolicy::recommended_for_context(&context),
            SandboxPolicy::permissive()
        );
    }

    #[test]
    fn managed_policy_executor_leaves_permissive_requests_unsandboxed() {
        let executor = ManagedPolicyProcessExecutor::new();
        let command = executor
            .prepare(ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: None,
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::HostUtility {
                    name: "test".to_string(),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy::permissive(),
            })
            .unwrap();

        assert_eq!(command.as_std().get_program(), "/bin/echo");
    }

    #[test]
    fn managed_policy_executor_can_fail_closed_when_no_backend_is_available() {
        let workspace = tempdir().unwrap();
        let err = prepare_with_available_backends(
            ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::BashTool,
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        protected_paths: vec![],
                    },
                    network: NetworkPolicy::Off,
                    host_escape: HostEscapePolicy::Deny,
                    fail_if_unavailable: true,
                },
            },
            BackendAvailability::default(),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("sandbox policy requires an enforcing backend")
        );
    }

    #[test]
    fn managed_policy_executor_can_fall_back_when_backend_is_unavailable() {
        let workspace = tempdir().unwrap();
        let command = prepare_with_available_backends(
            ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::BashTool,
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        protected_paths: vec![],
                    },
                    network: NetworkPolicy::Off,
                    host_escape: HostEscapePolicy::Deny,
                    fail_if_unavailable: false,
                },
            },
            BackendAvailability::default(),
        )
        .unwrap();

        assert_eq!(command.as_std().get_program(), "/bin/echo");
    }

    #[test]
    fn allow_domains_policy_never_falls_back_without_backend() {
        let workspace = tempdir().unwrap();
        let err = prepare_with_available_backends(
            ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::BashTool,
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        protected_paths: vec![],
                    },
                    network: NetworkPolicy::AllowDomains(vec!["example.com".to_string()]),
                    host_escape: HostEscapePolicy::Deny,
                    fail_if_unavailable: false,
                },
            },
            BackendAvailability::default(),
        )
        .unwrap_err();

        assert!(err.to_string().contains(
            "domain-scoped network policy requires a compatible enforcing sandbox backend"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_allow_domains_socket_path_is_stable_for_normalized_allowlists() {
        let first = super::default_linux_allow_domains_socket_path(&[
            "Example.COM".to_string(),
            "api.example.com".to_string(),
        ]);
        let second = super::default_linux_allow_domains_socket_path(&[
            "api.example.com".to_string(),
            "example.com".to_string(),
        ]);

        assert_eq!(first, second);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_policy_executor_wraps_restrictive_requests_with_sandbox_exec() {
        let workspace = tempdir().unwrap();
        let policy = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: vec![workspace.path().to_path_buf()],
                writable_roots: vec![workspace.path().to_path_buf()],
                protected_paths: vec![workspace.path().join(".git")],
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: true,
        };
        let command = super::super::executor_macos::prepare_macos_seatbelt_command(
            ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::BashTool,
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: policy,
            },
            std::path::Path::new(super::super::executor_macos::MACOS_SANDBOX_EXEC),
        )
        .unwrap();

        assert_eq!(
            command.as_std().get_program(),
            std::ffi::OsStr::new(super::super::executor_macos::MACOS_SANDBOX_EXEC)
        );
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(import \"system.sb\")"));
        assert!(args[1].contains(".git"));
        assert!(
            args[1].contains(
                &workspace
                    .path()
                    .canonicalize()
                    .unwrap()
                    .display()
                    .to_string()
            )
        );
        assert_eq!(args[2], "/bin/echo");
        assert_eq!(args[3], "hello");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_bwrap_backend_mounts_read_write_then_protected_paths_in_order_and_attaches_seccomp() {
        let workspace = tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
        let policy = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: vec![workspace.path().to_path_buf()],
                writable_roots: vec![workspace.path().to_path_buf()],
                protected_paths: vec![workspace.path().join(".git")],
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: true,
        };

        let command = super::super::executor_linux::prepare_linux_bwrap_command(
            ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::BashTool,
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: policy,
            },
            std::path::Path::new("/usr/bin/bwrap"),
        )
        .unwrap();

        assert_eq!(
            command.as_std().get_program(),
            std::ffi::OsStr::new("/usr/bin/bwrap")
        );
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let bind_index = args
            .windows(3)
            .position(|window| {
                window[0] == "--bind"
                    && window[1] == workspace.path().display().to_string()
                    && window[2] == workspace.path().display().to_string()
            })
            .expect("workspace bind should be present");
        let protected_index = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind"
                    && window[1] == workspace.path().join(".git").display().to_string()
                    && window[2] == workspace.path().join(".git").display().to_string()
            })
            .expect("protected path bind should be present");
        let seccomp_index = args
            .windows(2)
            .position(|window| window[0] == "--seccomp")
            .expect("seccomp fd should be present");
        assert!(args.contains(&"--unshare-net".to_string()));
        assert!(bind_index < protected_index);
        assert!(seccomp_index < args.iter().position(|arg| arg == "--").unwrap());
        assert_eq!(args.last().map(String::as_str), Some("hello"));
    }
}
