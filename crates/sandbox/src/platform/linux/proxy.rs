use crate::{Result, SandboxError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(crate) const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV: &str =
    "NANOCLAW_SANDBOX_PROXY_SOCKET_PATH";
pub(crate) const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV: &str =
    "NANOCLAW_SANDBOX_PROXY_SOCKET_SANDBOX_PATH";
pub(crate) const LINUX_ALLOW_DOMAINS_PROXY_URL_ENV: &str = "NANOCLAW_SANDBOX_PROXY_URL";
pub(crate) const LINUX_ALLOW_DOMAINS_PROXY_BRIDGE_PORT: u16 = 18080;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AllowDomainsProxyBridge {
    pub(crate) host_socket_path: PathBuf,
    pub(crate) sandbox_socket_path: PathBuf,
    pub(crate) proxy_url: String,
}

pub(crate) fn resolve_allow_domains_proxy_bridge(
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
        return Err(SandboxError::invalid_state(
            "Linux allow-domains proxy bridge paths must be absolute",
        ));
    }

    if !host_socket_path.exists() {
        return Err(SandboxError::invalid_state(format!(
            "Linux allow-domains proxy socket {} does not exist; host must start the proxy bridge first",
            host_socket_path.display()
        )));
    }

    if domains.is_empty() {
        return Err(SandboxError::invalid_state(
            "AllowDomains policy requires at least one domain entry",
        ));
    }

    Ok(AllowDomainsProxyBridge {
        host_socket_path,
        sandbox_socket_path,
        proxy_url,
    })
}

pub(crate) fn bind_allow_domains_proxy_bridge(
    command: &mut Command,
    bridge: &AllowDomainsProxyBridge,
) -> Result<()> {
    // Bubblewrap cannot mount a non-existent file destination directly. Mounting
    // the parent directory keeps the Unix-socket endpoint visible inside the
    // sandbox even if the host proxy rotates the socket file.
    let host_dir = bridge.host_socket_path.parent().ok_or_else(|| {
        SandboxError::invalid_state(format!(
            "Linux allow-domains proxy socket path {} has no parent directory",
            bridge.host_socket_path.display()
        ))
    })?;
    let sandbox_dir = bridge.sandbox_socket_path.parent().ok_or_else(|| {
        SandboxError::invalid_state(format!(
            "Linux allow-domains proxy sandbox socket path {} has no parent directory",
            bridge.sandbox_socket_path.display()
        ))
    })?;
    ensure_existing_path(host_dir, "allow-domains proxy directory")?;
    add_bind_mount(command, "--ro-bind", host_dir, sandbox_dir);
    Ok(())
}

pub(crate) fn apply_allow_domains_proxy_env(
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

pub(crate) fn append_allow_domains_bridge_wrapper(
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
            SandboxError::invalid_state(format!(
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
        return Err(SandboxError::invalid_state(format!(
            "invalid Linux allow-domains proxy URL `{proxy_url}`"
        )));
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| {
            SandboxError::invalid_state(format!(
                "invalid Linux allow-domains proxy URL `{proxy_url}`"
            ))
        })?;
        let host = &rest[..end];
        let tail = &rest[end + 1..];
        let port = tail.strip_prefix(':').ok_or_else(|| {
            SandboxError::invalid_state(format!(
                "Linux allow-domains proxy URL `{proxy_url}` must include an explicit port"
            ))
        })?;
        (host.to_string(), port.to_string())
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        (host.to_string(), port.to_string())
    } else {
        return Err(SandboxError::invalid_state(format!(
            "Linux allow-domains proxy URL `{proxy_url}` must include an explicit port"
        )));
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Err(SandboxError::invalid_state(format!(
            "Linux allow-domains proxy URL must target loopback, got `{host}`"
        )));
    }
    let port = port.parse::<u16>().map_err(|error| {
        SandboxError::invalid_state(format!(
            "invalid Linux allow-domains proxy port in `{proxy_url}`: {error}"
        ))
    })?;
    if port == 0 {
        return Err(SandboxError::invalid_state(
            "Linux allow-domains proxy port must be non-zero",
        ));
    }
    Ok(port)
}

fn ensure_existing_path(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    Err(SandboxError::invalid_state(format!(
        "sandbox {label} {} does not exist on the host",
        path.display()
    )))
}

fn add_bind_mount(command: &mut Command, flag: &str, source: &Path, dest: &Path) {
    command.arg(flag).arg(source).arg(dest);
}
