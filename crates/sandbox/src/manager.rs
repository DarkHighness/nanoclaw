use super::network_proxy::{
    DomainAllowlist, ProxyBindTarget, ProxyConfig, ProxyEndpoint, start_retained_proxy,
};
use super::policy::{
    ExecRequest, HostProcessExecutor, NetworkPolicy, ProcessExecutor, SandboxMode, SandboxPolicy,
};
use crate::{Result, SandboxError};
#[cfg(target_os = "linux")]
use std::collections::hash_map::DefaultHasher;
#[cfg(target_os = "linux")]
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BackendAvailability {
    macos_seatbelt: Option<PathBuf>,
    linux_bwrap: Option<PathBuf>,
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
        return availability.macos_seatbelt.is_some();
    }
    #[cfg(target_os = "linux")]
    {
        return availability.linux_bwrap.is_some();
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
        NetworkPolicy::AllowDomains(domains) => {
            format!("network allowlist({})", domains.join(","))
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
    if matches!(
        request.sandbox_policy.network,
        NetworkPolicy::AllowDomains(_)
    ) && !allow_domains_backend_available(&availability)
    {
        return Err(SandboxError::invalid_state(
            "domain-scoped network policy requires a compatible enforcing sandbox backend",
        ));
    }
    attach_allow_domains_proxy_support(&mut request)?;

    #[cfg(target_os = "macos")]
    if let Some(path) = availability.macos_seatbelt.as_deref() {
        return super::platform::macos::prepare_macos_seatbelt_command(request, path);
    }
    #[cfg(target_os = "linux")]
    if let Some(path) = availability.linux_bwrap.as_deref() {
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

pub(crate) fn sandbox_backend_status_with_availability(
    policy: &SandboxPolicy,
    availability: BackendAvailability,
) -> SandboxBackendStatus {
    if !policy.requires_enforcement() {
        return SandboxBackendStatus::NotRequired;
    }

    #[cfg(target_os = "macos")]
    {
        return availability
            .macos_seatbelt
            .map(|_| SandboxBackendStatus::Available {
                kind: SandboxBackendKind::MacOsSeatbelt,
            })
            .unwrap_or_else(|| SandboxBackendStatus::Unavailable {
                reason: "`sandbox-exec` is unavailable on this host".to_string(),
            });
    }

    #[cfg(target_os = "linux")]
    {
        if availability.linux_bwrap.is_some() {
            return SandboxBackendStatus::Available {
                kind: SandboxBackendKind::LinuxBubblewrap,
            };
        }
        return match super::platform::linux::linux_bwrap_status() {
            super::platform::linux::LinuxBubblewrapStatus::Available(_) => {
                SandboxBackendStatus::Available {
                    kind: SandboxBackendKind::LinuxBubblewrap,
                }
            }
            super::platform::linux::LinuxBubblewrapStatus::Unavailable { reason } => {
                SandboxBackendStatus::Unavailable { reason }
            }
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
    if super::platform::macos::has_allow_domains_proxy_config(&request.env) {
        return Ok(());
    }
    let allowlist = DomainAllowlist::new(domains.clone())
        .map_err(|error| SandboxError::invalid_state(error.to_string()))?;
    let endpoint = start_retained_proxy(ProxyConfig::localhost(allowlist))
        .map_err(|error| SandboxError::invalid_state(error.to_string()))?;
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
        .contains_key(super::platform::linux::LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV)
    {
        return Ok(());
    }

    let allowlist = DomainAllowlist::new(domains.clone())
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
        availability.macos_seatbelt = super::platform::macos::sandbox_exec_path();
    }
    #[cfg(target_os = "linux")]
    {
        availability.linux_bwrap = super::platform::linux::find_bwrap_executable();
    }
    availability
}

#[cfg(test)]
mod tests {
    use super::{
        BackendAvailability, ManagedPolicyProcessExecutor, SandboxBackendStatus,
        describe_sandbox_policy, prepare_with_available_backends,
        sandbox_backend_status_with_availability,
    };
    use crate::{
        ExecRequest, ExecutionOrigin, FilesystemPolicy, HostEscapePolicy, NetworkPolicy,
        ProcessExecutor, ProcessStdio, RuntimeScope, SandboxMode, SandboxPolicy,
    };
    use std::collections::BTreeMap;
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
