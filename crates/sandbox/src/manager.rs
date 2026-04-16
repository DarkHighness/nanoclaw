use super::network_proxy::{
    DomainAllowlist, ProxyBindTarget, ProxyConfig, ProxyEndpoint, start_retained_proxy,
};
use super::policy::{
    ExecRequest, HostProcessExecutor, NetworkPolicy, ProcessExecutor, SandboxMode, SandboxPolicy,
};
use crate::{Result, SandboxError};
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::collections::hash_map::DefaultHasher;
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
enum BackendPathStatus {
    Available(PathBuf),
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BackendAvailability {
    macos_seatbelt: Option<BackendPathStatus>,
    linux_bwrap: Option<BackendPathStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxBackendKind {
    MacOsSeatbelt,
    LinuxBubblewrap,
}

impl SandboxBackendKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MacOsSeatbelt => "macos-seatbelt",
            Self::LinuxBubblewrap => "linux-bwrap",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxBackendStatus {
    NotRequired,
    Available { kind: SandboxBackendKind },
    Unavailable { reason: String },
}

impl SandboxBackendStatus {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    #[must_use]
    pub fn kind(&self) -> Option<SandboxBackendKind> {
        match self {
            Self::Available { kind } => Some(*kind),
            Self::NotRequired | Self::Unavailable { .. } => None,
        }
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unavailable { reason } => Some(reason.as_str()),
            Self::NotRequired | Self::Available { .. } => None,
        }
    }
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
        return matches!(
            availability.macos_seatbelt,
            Some(BackendPathStatus::Available(_))
        );
    }
    #[cfg(target_os = "linux")]
    {
        return matches!(
            availability.linux_bwrap,
            Some(BackendPathStatus::Available(_))
        );
    }
    #[allow(unreachable_code)]
    false
}

#[must_use]
pub fn sandbox_backend_status(policy: &SandboxPolicy) -> SandboxBackendStatus {
    sandbox_backend_status_with_availability(policy, detect_available_backends())
}

pub fn ensure_sandbox_policy_supported(policy: &SandboxPolicy) -> Result<SandboxBackendStatus> {
    let status = sandbox_backend_status(policy);
    if policy.fail_if_unavailable
        && let SandboxBackendStatus::Unavailable { reason } = &status
    {
        return Err(SandboxError::invalid_state(format!(
            "sandbox policy requires an enforcing backend, but {reason}"
        )));
    }
    Ok(status)
}

#[must_use]
pub fn describe_sandbox_policy(policy: &SandboxPolicy, status: &SandboxBackendStatus) -> String {
    let mode = match policy.mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::DangerFullAccess => "danger-full-access",
    };
    let network = match &policy.network {
        NetworkPolicy::Off => "network off".to_string(),
        NetworkPolicy::Allowlist(allowlist) => {
            let mut entries = allowlist.domains.clone();
            entries.extend(allowlist.cidrs.clone());
            format!("network allowlist({})", entries.join(","))
        }
        NetworkPolicy::Full => "network full".to_string(),
    };
    let availability = match status {
        SandboxBackendStatus::Available { kind } => format!("enforced via {}", kind.as_str()),
        SandboxBackendStatus::Unavailable { reason } if policy.fail_if_unavailable => {
            format!("backend required but unavailable ({reason})")
        }
        SandboxBackendStatus::Unavailable { reason } => {
            format!("best effort host fallback ({reason})")
        }
        SandboxBackendStatus::NotRequired => "no enforcing backend required".to_string(),
    };
    format!("{mode}, {network}, {availability}")
}

impl ProcessExecutor for ManagedPolicyProcessExecutor {
    fn prepare(&self, request: ExecRequest) -> Result<Command> {
        if !request.sandbox_policy.requires_enforcement() {
            return self.host.prepare(request);
        }

        prepare_with_available_backends(request, detect_available_backends())
    }
}

pub(crate) fn prepare_with_available_backends(
    mut request: ExecRequest,
    availability: BackendAvailability,
) -> Result<Command> {
    normalize_mcp_stdio_request(&mut request);
    if matches!(request.sandbox_policy.network, NetworkPolicy::Allowlist(_))
        && !allow_domains_backend_available(&availability)
    {
        return Err(SandboxError::invalid_state(
            "domain-scoped network policy requires a compatible enforcing sandbox backend",
        ));
    }
    attach_allow_domains_proxy_support(&mut request)?;

    #[cfg(target_os = "macos")]
    if let Some(BackendPathStatus::Available(path)) = availability.macos_seatbelt.as_ref() {
        return super::platform::macos::prepare_macos_seatbelt_command(request, path);
    }
    #[cfg(target_os = "linux")]
    if let Some(BackendPathStatus::Available(path)) = availability.linux_bwrap.as_ref() {
        return super::platform::linux::prepare_linux_bwrap_command(request, path);
    }

    if request.sandbox_policy.fail_if_unavailable {
        let status =
            sandbox_backend_status_with_availability(&request.sandbox_policy, availability);
        let reason = status
            .reason()
            .unwrap_or("no compatible backend is available");
        Err(SandboxError::invalid_state(format!(
            "sandbox policy requires an enforcing backend, but {reason}"
        )))
    } else {
        HostProcessExecutor.prepare(request)
    }
}

fn normalize_mcp_stdio_request(request: &mut ExecRequest) {
    if !matches!(
        request.origin,
        super::policy::ExecutionOrigin::McpStdioServer { .. }
    ) {
        return;
    }

    let inherited_path = std::env::var_os("PATH");
    let effective_path = request
        .env
        .get("PATH")
        .map(OsStr::new)
        .or(inherited_path.as_deref());
    let Some(resolved_program) =
        resolve_mcp_stdio_program(&request.program, request.cwd.as_deref(), effective_path)
    else {
        return;
    };

    // Built-in MCP entries intentionally keep launcher commands portable
    // (`pnpm`, `npx`, `bunx`, etc.). When the host enforces a filesystem
    // sandbox, the child process only sees explicitly mounted roots, so resolve
    // the launcher on the host first and mirror the relevant executable roots
    // before entering bubblewrap/seatbelt.
    request.program = resolved_program.program.display().to_string();
    extend_unique_paths(
        &mut request.sandbox_policy.filesystem.readable_roots,
        resolved_program.mount_roots.iter().cloned(),
    );
    extend_unique_paths(
        &mut request.sandbox_policy.filesystem.executable_roots,
        resolved_program.mount_roots,
    );
}

#[derive(Debug)]
struct ResolvedMcpProgram {
    program: PathBuf,
    mount_roots: Vec<PathBuf>,
}

fn resolve_mcp_stdio_program(
    program: &str,
    cwd: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Option<ResolvedMcpProgram> {
    let candidate = Path::new(program);
    let resolved_program = if candidate.components().count() > 1 {
        resolve_direct_program(candidate, cwd)?
    } else {
        resolve_program_from_path(candidate, path_var)?
    };
    let resolved_program = canonicalize_existing_path(resolved_program);

    let mut mount_roots = existing_path_dirs(path_var);
    if let Some(parent) = resolved_program.parent() {
        mount_roots.push(parent.to_path_buf());
    }
    Some(ResolvedMcpProgram {
        program: resolved_program,
        mount_roots: dedup_paths(mount_roots),
    })
}

fn resolve_direct_program(program: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
    let candidate = if program.is_absolute() {
        program.to_path_buf()
    } else {
        let base_dir = cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())?;
        base_dir.join(program)
    };
    candidate.is_file().then_some(candidate)
}

fn resolve_program_from_path(program: &Path, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let executable = program.to_str()?;
    std::env::split_paths(path_var?).find_map(|dir| resolve_executable_candidate(&dir, executable))
}

fn resolve_executable_candidate(dir: &Path, executable: &str) -> Option<PathBuf> {
    let direct = dir.join(executable);
    if direct.is_file() {
        return Some(direct);
    }
    #[cfg(windows)]
    {
        for extension in ["exe", "cmd", "bat"] {
            let candidate = dir.join(format!("{executable}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn existing_path_dirs(path_var: Option<&OsStr>) -> Vec<PathBuf> {
    path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_dir())
        .map(canonicalize_existing_path)
        .collect()
}

fn canonicalize_existing_path(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn extend_unique_paths(target: &mut Vec<PathBuf>, additional: impl IntoIterator<Item = PathBuf>) {
    *target = dedup_paths(target.iter().cloned().chain(additional).collect());
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn sandbox_backend_status_with_availability(
    policy: &SandboxPolicy,
    availability: BackendAvailability,
) -> SandboxBackendStatus {
    if !policy.requires_enforcement() {
        return SandboxBackendStatus::NotRequired;
    }

    #[cfg(target_os = "macos")]
    {
        return match availability.macos_seatbelt {
            Some(BackendPathStatus::Available(_)) => SandboxBackendStatus::Available {
                kind: SandboxBackendKind::MacOsSeatbelt,
            },
            Some(BackendPathStatus::Unavailable { reason }) => {
                SandboxBackendStatus::Unavailable { reason }
            }
            None => SandboxBackendStatus::Unavailable {
                reason: "`sandbox-exec` availability was not provided".to_string(),
            },
        };
    }

    #[cfg(target_os = "linux")]
    {
        return match availability.linux_bwrap {
            Some(BackendPathStatus::Available(_)) => SandboxBackendStatus::Available {
                kind: SandboxBackendKind::LinuxBubblewrap,
            },
            Some(BackendPathStatus::Unavailable { reason }) => {
                SandboxBackendStatus::Unavailable { reason }
            }
            None => SandboxBackendStatus::Unavailable {
                reason: "`bwrap` availability was not provided".to_string(),
            },
        };
    }

    #[allow(unreachable_code)]
    SandboxBackendStatus::Unavailable {
        reason: format!(
            "no enforcing sandbox backend is implemented for {}",
            std::env::consts::OS
        ),
    }
}

fn allow_domains_backend_available(availability: &BackendAvailability) -> bool {
    #[cfg(target_os = "macos")]
    {
        return matches!(
            availability.macos_seatbelt,
            Some(BackendPathStatus::Available(_))
        );
    }
    #[cfg(target_os = "linux")]
    {
        return matches!(
            availability.linux_bwrap,
            Some(BackendPathStatus::Available(_))
        );
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
    let NetworkPolicy::Allowlist(allowlist) = &request.sandbox_policy.network else {
        return Ok(());
    };
    if super::platform::macos::has_allow_domains_proxy_config(&request.env) {
        return Ok(());
    }
    let allowlist = DomainAllowlist::new(allowlist.domains.clone())
        .map_err(|error| SandboxError::invalid_state(error.to_string()))?;
    let endpoint = start_retained_proxy(ProxyConfig::localhost(allowlist))
        .map_err(|error| SandboxError::invalid_state(error.to_string()))?;
    request.env.extend(endpoint.env_vars());
    Ok(())
}

#[cfg(target_os = "linux")]
fn maybe_attach_linux_allow_domains_proxy(request: &mut ExecRequest) -> Result<()> {
    let NetworkPolicy::Allowlist(allowlist) = &request.sandbox_policy.network else {
        return Ok(());
    };
    if request
        .env
        .contains_key(super::platform::linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV)
    {
        return Ok(());
    }

    let allowlist = DomainAllowlist::new(allowlist.domains.clone())
        .map_err(|error| SandboxError::invalid_state(error.to_string()))?;
    let socket_path = default_linux_allow_domains_socket_path(allowlist.domains());
    let endpoint = start_retained_proxy(ProxyConfig {
        allowlist,
        bind: ProxyBindTarget::UnixSocket(socket_path.clone()),
    })
    .map_err(|error| SandboxError::invalid_state(error.to_string()))?;
    let ProxyEndpoint::UnixSocket(host_socket_path) = endpoint else {
        return Err(SandboxError::invalid_state(
            "Linux allow-domains proxy must be a Unix-socket endpoint",
        ));
    };

    request.env.insert(
        super::platform::linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV.to_string(),
        host_socket_path.display().to_string(),
    );
    request.env.insert(
        super::platform::linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV.to_string(),
        host_socket_path.display().to_string(),
    );
    request.env.insert(
        super::platform::linux::LINUX_ALLOW_DOMAINS_PROXY_URL_ENV.to_string(),
        format!(
            "socks5h://127.0.0.1:{}",
            super::platform::linux::LINUX_ALLOW_DOMAINS_PROXY_BRIDGE_PORT
        ),
    );
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn default_linux_allow_domains_socket_path(domains: &[String]) -> PathBuf {
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
        availability.macos_seatbelt = Some(
            super::platform::macos::sandbox_exec_path()
                .map(BackendPathStatus::Available)
                .unwrap_or_else(|| BackendPathStatus::Unavailable {
                    reason: "`sandbox-exec` is unavailable on this host".to_string(),
                }),
        );
    }
    #[cfg(target_os = "linux")]
    {
        availability.linux_bwrap = Some(match super::platform::linux::linux_bwrap_status() {
            super::platform::linux::LinuxBubblewrapStatus::Available(path) => {
                BackendPathStatus::Available(path)
            }
            super::platform::linux::LinuxBubblewrapStatus::Unavailable { reason } => {
                BackendPathStatus::Unavailable { reason }
            }
        });
    }
    availability
}

#[cfg(test)]
mod tests {
    use super::{
        BackendAvailability, BackendPathStatus, ManagedPolicyProcessExecutor, SandboxBackendStatus,
        describe_sandbox_policy, prepare_with_available_backends,
        sandbox_backend_status_with_availability,
    };
    use crate::{
        ExecRequest, ExecutionOrigin, FilesystemPolicy, HostEscapePolicy, NetworkAllowlist,
        NetworkPolicy, ProcessExecutor, ProcessStdio, RuntimeScope, SandboxMode, SandboxPolicy,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn sandbox_backend_status_is_not_required_for_permissive_policy() {
        assert_eq!(
            sandbox_backend_status_with_availability(
                &SandboxPolicy::permissive(),
                BackendAvailability::default(),
            ),
            SandboxBackendStatus::NotRequired
        );
    }

    #[test]
    fn sandbox_backend_status_reports_unavailable_when_restrictive_policy_has_no_backend() {
        let workspace = tempdir().unwrap();
        let policy = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: vec![workspace.path().to_path_buf()],
                writable_roots: vec![workspace.path().to_path_buf()],
                executable_roots: vec![],
                protected_paths: vec![],
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: true,
        };

        assert!(matches!(
            sandbox_backend_status_with_availability(&policy, BackendAvailability::default()),
            SandboxBackendStatus::Unavailable { .. }
        ));
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
                origin: ExecutionOrigin::HostUtility {
                    name: "exec_command".to_string(),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        executable_roots: vec![],
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
                origin: ExecutionOrigin::HostUtility {
                    name: "exec_command".to_string(),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        executable_roots: vec![],
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
                origin: ExecutionOrigin::HostUtility {
                    name: "exec_command".to_string(),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        executable_roots: vec![],
                        protected_paths: vec![],
                    },
                    network: NetworkPolicy::Allowlist(NetworkAllowlist::with_domains(vec![
                        "example.com".to_string(),
                    ])),
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_bwrap_resolves_mcp_stdio_launchers_from_host_path() {
        let workspace = tempdir().unwrap();
        let tool_bin = tempdir().unwrap();
        let launcher = tool_bin.path().join("pnpm");
        fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();

        let command = prepare_with_available_backends(
            ExecRequest {
                program: "pnpm".to_string(),
                args: vec![
                    "dlx".to_string(),
                    "@upstash/context7-mcp@latest".to_string(),
                ],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::from([("PATH".to_string(), tool_bin.path().display().to_string())]),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::McpStdioServer {
                    server_name: "context7".into(),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy {
                    mode: SandboxMode::WorkspaceWrite,
                    filesystem: FilesystemPolicy {
                        readable_roots: vec![workspace.path().to_path_buf()],
                        writable_roots: vec![workspace.path().to_path_buf()],
                        executable_roots: vec![],
                        protected_paths: vec![],
                    },
                    network: NetworkPolicy::Off,
                    host_escape: HostEscapePolicy::Deny,
                    fail_if_unavailable: false,
                },
            },
            BackendAvailability {
                linux_bwrap: Some(BackendPathStatus::Available("/bin/true".into())),
                ..Default::default()
            },
        )
        .unwrap();

        let args = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let bind_root = tool_bin.path().canonicalize().unwrap();
        let bind_root_str = bind_root.display().to_string();
        let resolved_launcher = launcher.canonicalize().unwrap();
        let separator = args
            .iter()
            .position(|value| value == "--")
            .expect("bubblewrap separator");

        assert!(args.windows(3).any(|window| {
            window == ["--ro-bind", bind_root_str.as_str(), bind_root_str.as_str()]
        }));
        assert_eq!(args[separator + 1], resolved_launcher.display().to_string());
    }

    #[test]
    fn describe_sandbox_policy_reports_fallback_state() {
        let policy = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy::default(),
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: false,
        };
        assert!(
            describe_sandbox_policy(
                &policy,
                &SandboxBackendStatus::Unavailable {
                    reason: "missing backend".to_string()
                }
            )
            .contains("best effort host fallback")
        );
    }
}
