use crate::{Result, SandboxError};
use std::collections::BTreeMap;

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
pub(super) struct AllowDomainsProxyConfig {
    pub(super) loopback_ports: Vec<u16>,
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

pub(super) fn configure_allow_domains_proxy_env(
    env: &mut BTreeMap<String, String>,
    domains: &[String],
) -> Result<AllowDomainsProxyConfig> {
    if domains.is_empty() {
        return Err(SandboxError::invalid_state(
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
            return Err(SandboxError::invalid_state(format!(
                "domain-scoped network policy on macOS requires parseable proxy env; got `{key}`={value}",
            )));
        };
        if !is_loopback_host(&host) {
            return Err(SandboxError::invalid_state(format!(
                "domain-scoped network policy on macOS requires loopback proxy endpoint; got `{key}` host `{host}`",
            )));
        }
        let Some(port) = port else {
            return Err(SandboxError::invalid_state(format!(
                "domain-scoped network policy on macOS requires proxy env `{key}` to include an explicit loopback port",
            )));
        };
        loopback_ports.push(port);
    }
    if !has_proxy_env {
        return Err(SandboxError::invalid_state(
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
        return Err(SandboxError::invalid_state(format!(
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
        SandboxError::invalid_state(format!("invalid proxy port in `{key}`: {error}"))
    })?;
    if port == 0 {
        return Err(SandboxError::invalid_state(format!(
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
