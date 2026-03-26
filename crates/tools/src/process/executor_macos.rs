use super::executor::{
    ExecRequest, NetworkPolicy, SandboxMode, SandboxPolicy, accessible_roots,
    canonicalize_filesystem_policy, canonicalize_optional_path, resolve_effective_cwd,
};
use crate::{Result, ToolError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(super) const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
const PROXY_ENV_KEYS: &[&str] = &[
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];
const PROXY_HOST_ENV_KEY: &str = "NANOCLAW_SANDBOX_PROXY_HOST";
const HTTP_PROXY_PORT_ENV_KEY: &str = "NANOCLAW_SANDBOX_HTTP_PROXY_PORT";
const SOCKS_PROXY_PORT_ENV_KEY: &str = "NANOCLAW_SANDBOX_SOCKS_PROXY_PORT";

#[derive(Clone, Debug, Default)]
struct AllowDomainsProxyConfig {
    loopback_ports: Vec<u16>,
}

pub(super) fn sandbox_exec_path() -> Option<PathBuf> {
    Path::new(MACOS_SANDBOX_EXEC)
        .exists()
        .then(|| PathBuf::from(MACOS_SANDBOX_EXEC))
}

pub(super) fn has_allow_domains_proxy_config(env: &BTreeMap<String, String>) -> bool {
    PROXY_ENV_KEYS.iter().any(|key| {
        env.get(*key)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }) || env
        .get(HTTP_PROXY_PORT_ENV_KEY)
        .or_else(|| env.get(SOCKS_PROXY_PORT_ENV_KEY))
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

pub(super) fn prepare_macos_seatbelt_command(
    mut request: ExecRequest,
    sandbox_exec_path: &Path,
) -> Result<Command> {
    let proxy_config = match &request.sandbox_policy.network {
        NetworkPolicy::AllowDomains(domains) => {
            configure_allow_domains_proxy_env(&mut request.env, domains)?
        }
        _ => AllowDomainsProxyConfig::default(),
    };

    let cwd = canonicalize_optional_path(request.cwd.as_deref())?;
    let filesystem = canonicalize_filesystem_policy(&request.sandbox_policy.filesystem)?;
    let effective_policy = SandboxPolicy {
        filesystem,
        ..request.sandbox_policy.clone()
    };
    let effective_cwd = resolve_effective_cwd(cwd, &effective_policy)?;
    let profile = build_macos_seatbelt_profile(&effective_policy, &proxy_config)?;

    let mut command = Command::new(sandbox_exec_path);
    command
        .arg("-p")
        .arg(profile)
        .arg(&request.program)
        .args(&request.args)
        .stdin(request.stdin.into_stdio())
        .stdout(request.stdout.into_stdio())
        .stderr(request.stderr.into_stdio())
        .kill_on_drop(request.kill_on_drop);

    if let Some(cwd) = effective_cwd {
        command.current_dir(cwd);
    }
    if !request.env.is_empty() {
        command.envs(request.env);
    }

    let _ = request.origin;
    let _ = request.runtime_scope;

    Ok(command)
}

fn build_macos_seatbelt_profile(
    policy: &SandboxPolicy,
    proxy_config: &AllowDomainsProxyConfig,
) -> Result<String> {
    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        // `system.sb` is the stable Apple-provided base profile that keeps
        // dyld, sysctl, mach, and standard system-path access coherent. The
        // generated rules below then narrow host-visible workspace roots on top
        // of that baseline instead of trying to hand-maintain a fragile clone
        // of Apple's system allowances.
        "(import \"system.sb\")".to_string(),
        "(allow process*)".to_string(),
    ];

    if !policy.filesystem.readable_roots.is_empty()
        || !policy.filesystem.writable_roots.is_empty()
        || !policy.filesystem.protected_paths.is_empty()
    {
        // Seatbelt evaluates real paths rather than the user-facing `/var`
        // aliases the shell often exposes, so every host path is canonicalized
        // before it is embedded into the generated profile.
        lines.push("(allow file-read-metadata)".to_string());
    }

    for ancestor in policy_path_ancestors(policy) {
        lines.push(format!(
            "(allow file-read-metadata (literal \"{}\"))",
            escape_sbpl_path(&ancestor)
        ));
    }

    match policy.mode {
        SandboxMode::DangerFullAccess => lines.push("(allow file*)".to_string()),
        SandboxMode::ReadOnly | SandboxMode::WorkspaceWrite => {
            for root in &policy.filesystem.readable_roots {
                lines.push(format!(
                    "(allow file-read* file-map-executable file-test-existence (subpath \"{}\"))",
                    escape_sbpl_path(root)
                ));
            }
            if matches!(policy.mode, SandboxMode::WorkspaceWrite) {
                for root in &policy.filesystem.writable_roots {
                    lines.push(format!(
                        "(allow file* (subpath \"{}\"))",
                        escape_sbpl_path(root)
                    ));
                }
            }
        }
    }

    for protected in &policy.filesystem.protected_paths {
        lines.push(format!(
            "(deny file-write* (subpath \"{}\"))",
            escape_sbpl_path(protected)
        ));
    }

    match &policy.network {
        NetworkPolicy::Off => {}
        NetworkPolicy::Full => lines.push("(allow network*)".to_string()),
        NetworkPolicy::AllowDomains(domains) => {
            if domains.is_empty() {
                return Err(ToolError::invalid_state(
                    "domain-scoped network policy requires at least one allowed domain",
                ));
            }
            // Seatbelt cannot natively filter arbitrary hostnames. The most
            // defensible boundary it can provide here is "proxy-only egress":
            // deny generic networking and then allow loopback so a host-owned
            // localhost proxy can enforce the domain allowlist out of process.
            lines.push("(deny network*)".to_string());
            if proxy_config.loopback_ports.is_empty() {
                lines.push("(allow network* (local ip \"localhost:*\"))".to_string());
                lines.push("(allow network* (remote ip \"localhost:*\"))".to_string());
            } else {
                for port in &proxy_config.loopback_ports {
                    lines.push(format!(
                        "(allow network* (local ip \"localhost:{}\"))",
                        port
                    ));
                    lines.push(format!(
                        "(allow network* (remote ip \"localhost:{}\"))",
                        port
                    ));
                }
            }
        }
    }

    Ok(lines.join(" "))
}

fn configure_allow_domains_proxy_env(
    env: &mut BTreeMap<String, String>,
    domains: &[String],
) -> Result<AllowDomainsProxyConfig> {
    if domains.is_empty() {
        return Err(ToolError::invalid_state(
            "domain-scoped network policy requires at least one allowed domain",
        ));
    }
    inject_proxy_env_from_port_hints(env)?;
    let mut loopback_ports = Vec::new();
    let mut has_proxy_env = false;
    for key in PROXY_ENV_KEYS {
        let Some(value) = env.get(*key).map(|value| value.trim()) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        has_proxy_env = true;
        let Some((host, port)) = parse_proxy_endpoint(value) else {
            return Err(ToolError::invalid_state(format!(
                "domain-scoped network policy on macOS requires parseable proxy env; got `{key}`={value}",
            )));
        };
        if !is_loopback_host(&host) {
            return Err(ToolError::invalid_state(format!(
                "domain-scoped network policy on macOS requires loopback proxy endpoint; got `{key}` host `{host}`",
            )));
        }
        if let Some(port) = port {
            loopback_ports.push(port);
        }
    }
    if !has_proxy_env {
        return Err(ToolError::invalid_state(
            "domain-scoped network policy on macOS requires proxy env (ALL_PROXY/HTTPS_PROXY/HTTP_PROXY) or NANOCLAW_SANDBOX_*_PROXY_PORT hints",
        ));
    }
    loopback_ports.sort_unstable();
    loopback_ports.dedup();
    Ok(AllowDomainsProxyConfig { loopback_ports })
}

fn inject_proxy_env_from_port_hints(env: &mut BTreeMap<String, String>) -> Result<()> {
    let proxy_host = env
        .get(PROXY_HOST_ENV_KEY)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1")
        .to_string();
    if !is_loopback_host(&proxy_host) {
        return Err(ToolError::invalid_state(format!(
            "{PROXY_HOST_ENV_KEY} must resolve to localhost/127.0.0.1/[::1], got `{proxy_host}`",
        )));
    }

    if let Some(http_port) = parse_proxy_port(env, HTTP_PROXY_PORT_ENV_KEY)? {
        let endpoint = format!("http://{}:{http_port}", proxy_host_for_url(&proxy_host));
        env.entry("HTTP_PROXY".to_string())
            .or_insert_with(|| endpoint.clone());
        env.entry("HTTPS_PROXY".to_string())
            .or_insert_with(|| endpoint.clone());
        env.entry("http_proxy".to_string())
            .or_insert_with(|| endpoint.clone());
        env.entry("https_proxy".to_string()).or_insert(endpoint);
    }

    if let Some(socks_port) = parse_proxy_port(env, SOCKS_PROXY_PORT_ENV_KEY)? {
        let endpoint = format!("socks5h://{}:{socks_port}", proxy_host_for_url(&proxy_host));
        env.entry("ALL_PROXY".to_string())
            .or_insert_with(|| endpoint.clone());
        env.entry("all_proxy".to_string()).or_insert(endpoint);
    }
    Ok(())
}

fn parse_proxy_port(env: &BTreeMap<String, String>, key: &str) -> Result<Option<u16>> {
    let Some(raw) = env.get(key).map(|value| value.trim()) else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let port = raw.parse::<u16>().map_err(|error| {
        ToolError::invalid_state(format!("invalid proxy port in `{key}`: {error}"))
    })?;
    if port == 0 {
        return Err(ToolError::invalid_state(format!(
            "invalid proxy port in `{key}`: port 0 is not allowed"
        )));
    }
    Ok(Some(port))
}

fn parse_proxy_endpoint(value: &str) -> Option<(String, Option<u16>)> {
    let trimmed = value.trim();
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
        return None;
    }

    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let tail = &rest[end + 1..];
        let port = tail
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok());
        return Some((host.to_string(), port));
    }

    if let Some((host, port)) = authority.rsplit_once(':')
        && !host.contains(':')
    {
        let port = port.parse::<u16>().ok();
        return Some((host.to_string(), port));
    }

    Some((authority.to_string(), None))
}

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().trim_matches('.').to_ascii_lowercase();
    normalized == "localhost" || normalized == "127.0.0.1" || normalized == "::1"
}

fn proxy_host_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn policy_path_ancestors(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut ancestors = BTreeSet::new();
    for path in policy
        .filesystem
        .readable_roots
        .iter()
        .chain(policy.filesystem.writable_roots.iter())
        .chain(policy.filesystem.protected_paths.iter())
    {
        for ancestor in path.ancestors() {
            ancestors.insert(ancestor.to_path_buf());
        }
    }
    // `resolve_effective_cwd` already guarantees cwd stays inside configured
    // roots, so granting metadata access to root ancestors is sufficient for
    // startup path traversal without widening real file contents beyond the
    // declared read/write roots.
    ancestors.extend(accessible_roots(policy));
    ancestors.into_iter().collect()
}

fn escape_sbpl_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}
