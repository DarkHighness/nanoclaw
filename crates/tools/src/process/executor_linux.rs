use super::executor::{
    ExecRequest, NetworkPolicy, SandboxPolicy, canonicalize_filesystem_policy,
    canonicalize_optional_path, resolve_effective_cwd,
};
use crate::{Result, ToolError};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio as StdStdio};
use std::sync::OnceLock;
use tempfile::tempfile;
use tokio::process::Command;

const LINUX_BWRAP_CANDIDATES: &[&str] = &["bwrap", "bubblewrap"];
const LINUX_SOCAT_CANDIDATES: &[&str] = &["socat"];
pub(super) const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV: &str =
    "NANOCLAW_SANDBOX_PROXY_SOCKET_PATH";
pub(super) const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV: &str =
    "NANOCLAW_SANDBOX_PROXY_SOCKET_SANDBOX_PATH";
pub(super) const LINUX_ALLOW_DOMAINS_PROXY_URL_ENV: &str = "NANOCLAW_SANDBOX_PROXY_URL";
pub(super) const LINUX_ALLOW_DOMAINS_PROXY_BRIDGE_PORT: u16 = 18080;
const LINUX_SYSTEM_READONLY_ROOTS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/etc",
    "/opt",
    "/nix/store",
    "/run/current-system/sw",
];
static LINUX_BWRAP_STATUS: OnceLock<LinuxBubblewrapStatus> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LinuxBubblewrapStatus {
    Available(PathBuf),
    Unavailable { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxSeccompProfile {
    FullNetwork,
    NoNetwork,
    ProxyOnlyUnixSockets,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AllowDomainsProxyBridge {
    host_socket_path: PathBuf,
    sandbox_socket_path: PathBuf,
    proxy_url: String,
}

pub(super) fn find_bwrap_executable() -> Option<PathBuf> {
    match linux_bwrap_status() {
        LinuxBubblewrapStatus::Available(path) => Some(path),
        LinuxBubblewrapStatus::Unavailable { .. } => None,
    }
}

fn find_socat_executable() -> Option<PathBuf> {
    find_first_usable_executable(LINUX_SOCAT_CANDIDATES, |_| true)
}

pub(super) fn linux_bwrap_status() -> LinuxBubblewrapStatus {
    LINUX_BWRAP_STATUS.get_or_init(detect_bwrap_status).clone()
}

fn find_first_usable_executable(
    candidates: &[&str],
    is_usable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for candidate in candidates {
            let resolved = directory.join(candidate);
            if resolved.is_file() && is_usable(&resolved) {
                return Some(resolved);
            }
        }
    }
    None
}

fn detect_bwrap_status() -> LinuxBubblewrapStatus {
    let Some(path) = std::env::var_os("PATH") else {
        return LinuxBubblewrapStatus::Unavailable {
            reason: "PATH is unset, so no `bwrap` executable can be discovered".to_string(),
        };
    };
    let mut first_failure = None;
    for directory in std::env::split_paths(&path) {
        for candidate in LINUX_BWRAP_CANDIDATES {
            let resolved = directory.join(candidate);
            if !resolved.is_file() {
                continue;
            }
            match probe_bwrap_runtime(&resolved) {
                Ok(()) => return LinuxBubblewrapStatus::Available(resolved),
                Err(reason) => {
                    if first_failure.is_none() {
                        first_failure = Some(format!(
                            "`{}` exists but cannot create an unprivileged sandbox: {reason}",
                            resolved.display()
                        ));
                    }
                }
            }
        }
    }
    LinuxBubblewrapStatus::Unavailable {
        reason: first_failure.unwrap_or_else(|| {
            "no `bwrap` or `bubblewrap` executable was found in PATH".to_string()
        }),
    }
}

fn probe_bwrap_runtime(bwrap_path: &Path) -> std::result::Result<(), String> {
    // Linux distributions often ship `bwrap` even when the surrounding
    // container/kernel forbids unprivileged user namespaces. Treating binary
    // presence as availability makes fail-closed policies report a later child
    // process crash instead of "no backend is available", so probe the actual
    // namespace boundary once before selecting Bubblewrap.
    match StdCommand::new(bwrap_path)
        .arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--")
        .arg("/bin/sh")
        .arg("-lc")
        .arg("true")
        .stdin(StdStdio::null())
        .stdout(StdStdio::piped())
        .stderr(StdStdio::piped())
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("exit status {}", output.status)
            };
            Err(detail)
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn prepare_linux_bwrap_command(
    request: ExecRequest,
    bwrap_path: &Path,
) -> Result<Command> {
    let cwd = canonicalize_optional_path(request.cwd.as_deref())?;
    let filesystem = canonicalize_filesystem_policy(&request.sandbox_policy.filesystem)?;
    let effective_policy = SandboxPolicy {
        filesystem,
        ..request.sandbox_policy.clone()
    };
    let proxy_bridge = match &effective_policy.network {
        NetworkPolicy::AllowDomains(domains) => {
            Some(resolve_allow_domains_proxy_bridge(domains, &request.env)?)
        }
        _ => None,
    };
    let socat_path = if proxy_bridge.is_some() {
        Some(find_socat_executable().ok_or_else(|| {
            ToolError::invalid_state(
                "Linux allow-domains policy requires `socat` to bridge loopback proxy traffic inside the sandbox",
            )
        })?)
    } else {
        None
    };
    let seccomp_profile = select_seccomp_profile(&effective_policy.network);
    let effective_cwd = resolve_effective_cwd(cwd, &effective_policy)?;
    let mut effective_env = request.env;
    apply_allow_domains_proxy_env(proxy_bridge.as_ref(), &mut effective_env);

    let mut command = Command::new(bwrap_path);
    command
        .arg("--die-with-parent")
        .arg("--new-session")
        .arg("--unshare-pid")
        .arg("--unshare-ipc")
        .arg("--unshare-uts")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--tmpfs")
        .arg("/tmp")
        .arg("--tmpfs")
        .arg("/var/tmp");

    if requires_network_namespace_isolation(&effective_policy.network) {
        command.arg("--unshare-net");
    }

    for root in LINUX_SYSTEM_READONLY_ROOTS {
        command.arg("--ro-bind-try").arg(root).arg(root);
    }

    // Bubblewrap applies mounts in argument order. Read-only roots land first,
    // writable roots override them where needed, and protected paths are
    // rebound read-only last so control directories such as `.git` stay
    // immutable even inside otherwise writable workspaces.
    for root in &effective_policy.filesystem.readable_roots {
        ensure_policy_mount_source_exists(root, "readable root")?;
        add_bind_mount(&mut command, "--ro-bind", root, root);
    }
    for root in &effective_policy.filesystem.writable_roots {
        ensure_policy_mount_source_exists(root, "writable root")?;
        add_bind_mount(&mut command, "--bind", root, root);
    }
    for path in &effective_policy.filesystem.protected_paths {
        if path.exists() {
            add_bind_mount(&mut command, "--ro-bind", path, path);
        }
    }

    if let Some(bridge) = proxy_bridge.as_ref() {
        bind_allow_domains_proxy_bridge(&mut command, bridge)?;
    }

    attach_seccomp_filter(&mut command, &build_linux_seccomp_program(seccomp_profile)?)?;

    if let Some(cwd) = effective_cwd.as_ref() {
        command.arg("--chdir").arg(cwd);
        command.current_dir(cwd);
    }

    command.arg("--");
    if let (Some(bridge), Some(socat_path)) = (proxy_bridge.as_ref(), socat_path.as_ref()) {
        append_allow_domains_bridge_wrapper(
            &mut command,
            bridge,
            socat_path,
            &request.program,
            &request.args,
        )?;
    } else {
        command.arg(&request.program).args(&request.args);
    }
    command
        .stdin(request.stdin.into_stdio())
        .stdout(request.stdout.into_stdio())
        .stderr(request.stderr.into_stdio())
        .kill_on_drop(request.kill_on_drop);

    if !effective_env.is_empty() {
        command.envs(effective_env);
    }

    let _ = request.origin;
    let _ = request.runtime_scope;

    Ok(command)
}

fn requires_network_namespace_isolation(network: &NetworkPolicy) -> bool {
    matches!(network, NetworkPolicy::Off | NetworkPolicy::AllowDomains(_))
}

fn select_seccomp_profile(network: &NetworkPolicy) -> LinuxSeccompProfile {
    match network {
        NetworkPolicy::Full => LinuxSeccompProfile::FullNetwork,
        NetworkPolicy::Off => LinuxSeccompProfile::NoNetwork,
        NetworkPolicy::AllowDomains(_) => LinuxSeccompProfile::ProxyOnlyUnixSockets,
    }
}

fn resolve_allow_domains_proxy_bridge(
    domains: &[String],
    env: &BTreeMap<String, String>,
) -> Result<AllowDomainsProxyBridge> {
    // Linux `AllowDomains` is enforced by isolating the network namespace and
    // bridging a host-managed Unix-socket proxy into the sandbox. The host is
    // expected to inject this bridge metadata (and run the proxy) before the
    // command is prepared.
    let host_socket_path = required_env_path(
        env,
        LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV,
        "proxy socket path",
    )?;
    let sandbox_socket_path =
        optional_env_path(env, LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV)?
            .unwrap_or_else(|| host_socket_path.clone());
    let proxy_url =
        required_env_value(env, LINUX_ALLOW_DOMAINS_PROXY_URL_ENV, "proxy URL")?.to_string();

    if !host_socket_path.is_absolute() || !sandbox_socket_path.is_absolute() {
        return Err(ToolError::invalid_state(
            "Linux allow-domains proxy bridge paths must be absolute",
        ));
    }

    if !host_socket_path.exists() {
        return Err(ToolError::invalid_state(format!(
            "Linux allow-domains proxy socket {} does not exist; host must start the proxy bridge first",
            host_socket_path.display()
        )));
    }

    if domains.is_empty() {
        return Err(ToolError::invalid_state(
            "AllowDomains policy requires at least one domain entry",
        ));
    }

    Ok(AllowDomainsProxyBridge {
        host_socket_path,
        sandbox_socket_path,
        proxy_url,
    })
}

fn bind_allow_domains_proxy_bridge(
    command: &mut Command,
    bridge: &AllowDomainsProxyBridge,
) -> Result<()> {
    // Bubblewrap cannot mount a non-existent file destination directly. Mounting
    // the parent directory keeps the Unix-socket endpoint visible inside the
    // sandbox even if the host proxy rotates the socket file.
    let host_dir = bridge.host_socket_path.parent().ok_or_else(|| {
        ToolError::invalid_state(format!(
            "Linux allow-domains proxy socket path {} has no parent directory",
            bridge.host_socket_path.display()
        ))
    })?;
    let sandbox_dir = bridge.sandbox_socket_path.parent().ok_or_else(|| {
        ToolError::invalid_state(format!(
            "Linux allow-domains proxy sandbox socket path {} has no parent directory",
            bridge.sandbox_socket_path.display()
        ))
    })?;
    ensure_policy_mount_source_exists(host_dir, "allow-domains proxy directory")?;
    add_bind_mount(command, "--ro-bind", host_dir, sandbox_dir);
    Ok(())
}

fn apply_allow_domains_proxy_env(
    bridge: Option<&AllowDomainsProxyBridge>,
    env: &mut BTreeMap<String, String>,
) {
    let Some(bridge) = bridge else {
        return;
    };
    // Child tools vary in which proxy variable they honor. Populate the common
    // set only when absent so host-provided command env can still override.
    set_if_missing(env, "ALL_PROXY", &bridge.proxy_url);
    set_if_missing(env, "all_proxy", &bridge.proxy_url);
    if bridge.proxy_url.starts_with("http://") || bridge.proxy_url.starts_with("https://") {
        set_if_missing(env, "HTTP_PROXY", &bridge.proxy_url);
        set_if_missing(env, "http_proxy", &bridge.proxy_url);
        set_if_missing(env, "HTTPS_PROXY", &bridge.proxy_url);
        set_if_missing(env, "https_proxy", &bridge.proxy_url);
    }
}

fn set_if_missing(env: &mut BTreeMap<String, String>, key: &str, value: &str) {
    env.entry(key.to_string())
        .or_insert_with(|| value.to_string());
}

fn required_env_value<'a>(
    env: &'a BTreeMap<String, String>,
    key: &str,
    label: &str,
) -> Result<&'a str> {
    env.get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ToolError::invalid_state(format!(
                "Linux allow-domains policy requires {label}; host must inject {key}"
            ))
        })
}

fn optional_env_path(env: &BTreeMap<String, String>, key: &str) -> Result<Option<PathBuf>> {
    let Some(value) = env.get(key) else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(value)))
}

fn required_env_path(env: &BTreeMap<String, String>, key: &str, label: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(required_env_value(env, key, label)?))
}

fn build_linux_seccomp_program(profile: LinuxSeccompProfile) -> Result<BpfProgram> {
    let mut rules = BTreeMap::new();
    add_unconditional_allow_rules(&mut rules, &baseline_allowed_syscalls());

    match profile {
        LinuxSeccompProfile::FullNetwork => {
            add_unconditional_allow_rules(&mut rules, &network_allowed_syscalls())
        }
        LinuxSeccompProfile::NoNetwork => {}
        LinuxSeccompProfile::ProxyOnlyUnixSockets => {
            add_unconditional_allow_rules(&mut rules, &proxy_flow_syscalls());
            add_proxy_bridge_socket_rules(&mut rules, libc::SYS_socket)?;
            add_proxy_bridge_socket_rules(&mut rules, libc::SYS_socketpair)?;
        }
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        std::env::consts::ARCH.try_into().map_err(|error| {
            ToolError::invalid_state(format!(
                "unsupported Linux seccomp architecture {}: {error}",
                std::env::consts::ARCH
            ))
        })?,
    )
    .map_err(|error| {
        ToolError::invalid_state(format!("failed to build seccomp filter: {error}"))
    })?;

    filter.try_into().map_err(|error| {
        ToolError::invalid_state(format!("failed to compile seccomp filter: {error}"))
    })
}

fn baseline_allowed_syscalls() -> Vec<i64> {
    let mut syscalls = vec![
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_openat,
        libc::SYS_close,
        libc::SYS_statx,
        libc::SYS_newfstatat,
        libc::SYS_fstat,
        libc::SYS_lseek,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_ioctl,
        libc::SYS_access,
        libc::SYS_faccessat,
        libc::SYS_pipe,
        libc::SYS_pipe2,
        libc::SYS_dup,
        libc::SYS_dup2,
        libc::SYS_dup3,
        libc::SYS_fcntl,
        libc::SYS_getdents64,
        libc::SYS_getcwd,
        libc::SYS_chdir,
        libc::SYS_fchdir,
        libc::SYS_readlink,
        libc::SYS_readlinkat,
        libc::SYS_unlinkat,
        libc::SYS_mkdirat,
        libc::SYS_renameat,
        libc::SYS_renameat2,
        libc::SYS_linkat,
        libc::SYS_symlinkat,
        libc::SYS_umask,
        libc::SYS_ftruncate,
        libc::SYS_fsync,
        libc::SYS_fdatasync,
        libc::SYS_fallocate,
        libc::SYS_clock_gettime,
        libc::SYS_clock_getres,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_getrandom,
        libc::SYS_uname,
        libc::SYS_getpid,
        libc::SYS_getppid,
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_prlimit64,
        libc::SYS_clone,
        libc::SYS_clone3,
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_wait4,
        libc::SYS_waitid,
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_sched_getaffinity,
        libc::SYS_prctl,
        libc::SYS_getrusage,
        libc::SYS_kill,
        libc::SYS_tgkill,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_poll,
        libc::SYS_ppoll,
        libc::SYS_select,
        libc::SYS_pselect6,
        libc::SYS_epoll_create1,
        libc::SYS_epoll_ctl,
        libc::SYS_epoll_wait,
        libc::SYS_epoll_pwait,
        libc::SYS_eventfd2,
    ];
    #[cfg(target_arch = "x86_64")]
    {
        // x86_64 dynamic loader initialization requires TLS arch setup.
        syscalls.push(libc::SYS_arch_prctl);
    }
    syscalls
}

fn network_allowed_syscalls() -> Vec<i64> {
    vec![
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_shutdown,
    ]
}

fn proxy_flow_syscalls() -> Vec<i64> {
    // `socket` and `socketpair` are constrained to AF_UNIX by argument-matched
    // rules. The remaining calls operate on already-created descriptors.
    vec![
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_shutdown,
    ]
}

fn add_unconditional_allow_rules(rules: &mut BTreeMap<i64, Vec<SeccompRule>>, syscalls: &[i64]) {
    for syscall in syscalls {
        rules.entry(*syscall).or_default();
    }
}

fn add_proxy_bridge_socket_rules(
    rules: &mut BTreeMap<i64, Vec<SeccompRule>>,
    syscall: i64,
) -> Result<()> {
    let mut socket_rules = Vec::new();
    for family in [libc::AF_UNIX, libc::AF_INET, libc::AF_INET6] {
        let domain_condition =
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, family as u64)
                .map_err(|error| {
                    ToolError::invalid_state(format!(
                        "failed to build seccomp socket-domain condition: {error}"
                    ))
                })?;
        let rule = SeccompRule::new(vec![domain_condition]).map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to build seccomp socket-domain rule: {error}"
            ))
        })?;
        socket_rules.push(rule);
    }
    rules.insert(syscall, socket_rules);
    Ok(())
}

fn append_allow_domains_bridge_wrapper(
    command: &mut Command,
    bridge: &AllowDomainsProxyBridge,
    socat_path: &Path,
    program: &str,
    args: &[String],
) -> Result<()> {
    let proxy_port = parse_loopback_proxy_port(&bridge.proxy_url)?;
    let bridge_script = r#"ip link set lo up >/dev/null 2>&1 || true
"$1" "TCP-LISTEN:${2},bind=127.0.0.1,reuseaddr,fork" "UNIX-CONNECT:${3}" >/dev/null 2>&1 &
bridge_pid=$!
trap 'kill "$bridge_pid"' EXIT
shift 3
exec "$@""#;

    command
        .arg("/bin/sh")
        .arg("-lc")
        .arg(bridge_script)
        .arg("sh")
        .arg(socat_path)
        .arg(proxy_port.to_string())
        .arg(&bridge.sandbox_socket_path)
        .arg(program)
        .args(args);
    Ok(())
}

fn parse_loopback_proxy_port(proxy_url: &str) -> Result<u16> {
    let trimmed = proxy_url.trim();
    let authority = trimmed
        .split("://")
        .nth(1)
        .unwrap_or(trimmed)
        .split('/')
        .next()
        .unwrap_or(trimmed)
        .rsplit('@')
        .next()
        .unwrap_or(trimmed)
        .trim();
    if authority.is_empty() {
        return Err(ToolError::invalid_state(format!(
            "invalid Linux allow-domains proxy URL `{proxy_url}`"
        )));
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            ToolError::invalid_state(format!(
                "invalid Linux allow-domains proxy URL `{proxy_url}`"
            ))
        })?;
        let host = &rest[..end];
        let tail = &rest[end + 1..];
        let port = tail.strip_prefix(':').ok_or_else(|| {
            ToolError::invalid_state(format!(
                "Linux allow-domains proxy URL `{proxy_url}` must include an explicit port"
            ))
        })?;
        (host.to_string(), port.to_string())
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        (host.to_string(), port.to_string())
    } else {
        return Err(ToolError::invalid_state(format!(
            "Linux allow-domains proxy URL `{proxy_url}` must include an explicit port"
        )));
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Err(ToolError::invalid_state(format!(
            "Linux allow-domains proxy URL must target loopback, got `{host}`"
        )));
    }
    let port = port.parse::<u16>().map_err(|error| {
        ToolError::invalid_state(format!(
            "invalid Linux allow-domains proxy port in `{proxy_url}`: {error}"
        ))
    })?;
    if port == 0 {
        return Err(ToolError::invalid_state(
            "Linux allow-domains proxy port must be non-zero",
        ));
    }
    Ok(port)
}

fn attach_seccomp_filter(command: &mut Command, program: &BpfProgram) -> Result<()> {
    let mut seccomp_file = tempfile().map_err(|error| {
        ToolError::invalid_state(format!("failed to create seccomp tempfile: {error}"))
    })?;
    write_bpf_program(&mut seccomp_file, program)?;
    seccomp_file.rewind().map_err(|error| {
        ToolError::invalid_state(format!("failed to rewind seccomp tempfile: {error}"))
    })?;
    let seccomp_fd = seccomp_file.as_raw_fd();
    command.arg("--seccomp").arg(seccomp_fd.to_string());

    // Bubblewrap wants a raw inherited fd rather than a file path. Keeping the
    // tempfile inside the pre-exec closure preserves the descriptor until
    // `execve`, and clearing `FD_CLOEXEC` lets the target `bwrap` process read
    // the compiled BPF program without widening the ProcessExecutor trait.
    unsafe {
        command.pre_exec(move || {
            let _keep_alive = &seccomp_file;
            clear_close_on_exec(seccomp_fd)?;
            Ok(())
        });
    }

    Ok(())
}

fn write_bpf_program(file: &mut File, program: &BpfProgram) -> Result<()> {
    let bytes = unsafe {
        std::slice::from_raw_parts(
            program.as_ptr().cast::<u8>(),
            program.len() * size_of::<seccompiler::sock_filter>(),
        )
    };
    file.write_all(bytes).map_err(|error| {
        ToolError::invalid_state(format!("failed to write seccomp program: {error}"))
    })?;
    Ok(())
}

fn clear_close_on_exec(fd: i32) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn ensure_policy_mount_source_exists(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    Err(ToolError::invalid_state(format!(
        "sandbox {label} {} does not exist on the host",
        path.display()
    )))
}

fn add_bind_mount(command: &mut Command, flag: &str, source: &Path, dest: &Path) {
    command.arg(flag).arg(source).arg(dest);
}

#[cfg(test)]
mod tests {
    use super::probe_bwrap_runtime;

    #[test]
    fn bwrap_probe_accepts_zero_exit_status() {
        assert!(probe_bwrap_runtime(std::path::Path::new("/bin/true")).is_ok());
    }

    #[test]
    fn bwrap_probe_rejects_non_zero_exit_status() {
        assert!(probe_bwrap_runtime(std::path::Path::new("/bin/false")).is_err());
    }
}
