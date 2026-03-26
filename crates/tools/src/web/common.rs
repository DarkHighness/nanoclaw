use crate::{Result, ToolError};
use agent_env::{self, EnvVar, vars};
use regex::Regex;
use reqwest::{Client, Url, redirect::Policy};
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) const DEFAULT_HTTP_TIMEOUT_MS: u64 = 20_000;
pub(crate) const DEFAULT_FETCH_MAX_CHARS: usize = 20_000;
pub(crate) const MAX_FETCH_MAX_CHARS: usize = 200_000;
pub(crate) const DEFAULT_SEARCH_LIMIT: usize = 5;
pub(crate) const MAX_SEARCH_LIMIT: usize = 10;
pub(crate) const DEFAULT_HTTP_REDIRECT_LIMIT: usize = 5;
const DEFAULT_WEB_USER_AGENT: &str = "nanoclaw/0.1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RedirectValidationScope {
    Transport,
    Target,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WebToolPolicy {
    pub allow_private_hosts: bool,
    pub allowed_domains: BTreeSet<String>,
    pub blocked_domains: BTreeSet<String>,
}

impl Default for WebToolPolicy {
    fn default() -> Self {
        Self::from_env()
    }
}

impl WebToolPolicy {
    #[must_use]
    pub(crate) fn from_env() -> Self {
        Self {
            allow_private_hosts: agent_env::read_bool_flag(
                vars::AGENT_CORE_WEB_ALLOW_PRIVATE_HOSTS,
            ),
            allowed_domains: parse_domain_list(vars::AGENT_CORE_WEB_ALLOWED_DOMAINS),
            blocked_domains: parse_domain_list(vars::AGENT_CORE_WEB_BLOCKED_DOMAINS),
        }
    }

    pub(crate) fn validate_transport_url(&self, url: &Url) -> Result<()> {
        match url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(ToolError::invalid(format!(
                    "unsupported URL scheme `{scheme}`; expected http or https"
                )));
            }
        }

        let host = url
            .host_str()
            .ok_or_else(|| ToolError::invalid("URL is missing a host"))?;
        if !self.allow_private_hosts && is_private_or_local_host(host) {
            return Err(ToolError::invalid(format!(
                "refusing to access local or private host `{host}`"
            )));
        }
        Ok(())
    }

    pub(crate) fn validate_target_url(&self, url: &Url) -> Result<()> {
        self.validate_transport_url(url)?;
        let host = url
            .host_str()
            .ok_or_else(|| ToolError::invalid("URL is missing a host"))?;
        if !domain_allowed(host, &self.allowed_domains) {
            return Err(ToolError::invalid(format!(
                "host `{host}` is outside the configured allowlist"
            )));
        }
        if domain_blocked(host, &self.blocked_domains) {
            return Err(ToolError::invalid(format!(
                "host `{host}` is blocked by policy"
            )));
        }
        Ok(())
    }
}

pub(crate) fn default_http_client(
    timeout_ms: u64,
    policy: WebToolPolicy,
    redirect_scope: RedirectValidationScope,
) -> Result<Client> {
    // Redirect destinations have to be validated before reqwest follows them.
    // Checking only the eventual response URL would still allow intermediate
    // hops to reach private or disallowed hosts.
    let redirect_policy = Policy::custom(move |attempt| {
        match validate_redirect_attempt(
            &policy,
            redirect_scope,
            attempt.url(),
            attempt.previous().len(),
        ) {
            Ok(()) => attempt.follow(),
            Err(error) => attempt.error(error),
        }
    });

    Ok(Client::builder()
        .redirect(redirect_policy)
        .timeout(Duration::from_millis(timeout_ms.max(1)))
        .user_agent(DEFAULT_WEB_USER_AGENT)
        .build()?)
}

#[must_use]
pub(crate) fn clamped_fetch_max_chars(value: Option<usize>) -> usize {
    value
        .unwrap_or(DEFAULT_FETCH_MAX_CHARS)
        .clamp(256, MAX_FETCH_MAX_CHARS)
}

#[must_use]
pub(crate) fn clamped_search_limit(value: Option<usize>) -> usize {
    value
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT)
}

#[must_use]
pub(crate) fn is_html_content_type(content_type: Option<&str>) -> bool {
    content_type
        .map(|value| value.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false)
}

#[must_use]
pub(crate) fn is_text_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let normalized = content_type.to_ascii_lowercase();
    normalized.starts_with("text/")
        || normalized.contains("json")
        || normalized.contains("xml")
        || normalized.contains("javascript")
        || normalized.contains("yaml")
        || normalized.contains("markdown")
}

#[must_use]
pub(crate) fn extract_html_title(html: &str) -> Option<String> {
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    TITLE_RE
        .get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("title regex"))
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|value| normalize_whitespace(&decode_html_entities(value.as_str())))
        .filter(|value| !value.is_empty())
}

#[must_use]
pub(crate) fn html_to_text(html: &str) -> String {
    static COMMENT_RE: OnceLock<Regex> = OnceLock::new();
    static STRIP_BLOCK_RE: OnceLock<Regex> = OnceLock::new();
    static BREAK_RE: OnceLock<Regex> = OnceLock::new();
    static LI_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();

    let without_comments = COMMENT_RE
        .get_or_init(|| Regex::new(r"(?is)<!--.*?-->").expect("comment regex"))
        .replace_all(html, " ");
    let without_hidden = STRIP_BLOCK_RE
        .get_or_init(|| {
            Regex::new(
                r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>|<noscript[^>]*>.*?</noscript>|<svg[^>]*>.*?</svg>|<canvas[^>]*>.*?</canvas>|<iframe[^>]*>.*?</iframe>|<template[^>]*>.*?</template>|<head[^>]*>.*?</head>",
            )
            .expect("strip regex")
        })
        .replace_all(&without_comments, " ");
    let with_breaks = BREAK_RE
        .get_or_init(|| {
            Regex::new(r"(?is)</?(p|div|section|article|header|footer|main|aside|nav|table|tr|h[1-6]|br)[^>]*>")
                .expect("break regex")
        })
        .replace_all(&without_hidden, "\n");
    let with_list_markers = LI_RE
        .get_or_init(|| Regex::new(r"(?is)<li[^>]*>").expect("li regex"))
        .replace_all(&with_breaks, "\n- ");
    let stripped = TAG_RE
        .get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("tag regex"))
        .replace_all(&with_list_markers, " ");

    normalize_whitespace(&decode_html_entities(&stripped))
}

#[must_use]
pub(crate) fn decode_html_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let chars: Vec<char> = value.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] != '&' {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        let mut end = index + 1;
        while end < chars.len() && end.saturating_sub(index) <= 12 && chars[end] != ';' {
            end += 1;
        }
        if end >= chars.len() || chars[end] != ';' {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        let entity: String = chars[index + 1..end].iter().collect();
        if let Some(decoded) = decode_single_entity(&entity) {
            output.push(decoded);
            index = end + 1;
            continue;
        }

        output.push(chars[index]);
        index += 1;
    }

    output
}

#[must_use]
pub(crate) fn truncate_text(value: &str, max_chars: usize) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return (value.to_string(), false);
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    while truncated.ends_with(char::is_whitespace) {
        truncated.pop();
    }
    (truncated, true)
}

#[must_use]
pub(crate) fn summarize_remote_body(body: &str, content_type: Option<&str>) -> String {
    let text = if is_html_content_type(content_type) || looks_like_html_document(body) {
        html_to_text(body)
    } else {
        normalize_whitespace(&decode_html_entities(body))
    };
    text.trim().to_string()
}

#[must_use]
pub(crate) fn looks_like_html_document(body: &str) -> bool {
    let prefix = body.trim_start().chars().take(256).collect::<String>();
    let normalized = prefix.to_ascii_lowercase();
    normalized.starts_with("<!doctype html")
        || normalized.starts_with("<html")
        || normalized.starts_with("<head")
        || normalized.starts_with("<body")
        || (normalized.contains("<title") && normalized.contains("<p"))
}

pub(crate) fn validate_redirect_attempt(
    policy: &WebToolPolicy,
    scope: RedirectValidationScope,
    next_url: &Url,
    previous_len: usize,
) -> Result<()> {
    if previous_len > DEFAULT_HTTP_REDIRECT_LIMIT {
        return Err(ToolError::invalid(format!(
            "too many redirects; limit is {DEFAULT_HTTP_REDIRECT_LIMIT}"
        )));
    }

    match scope {
        RedirectValidationScope::Transport => policy.validate_transport_url(next_url),
        RedirectValidationScope::Target => policy.validate_target_url(next_url),
    }
}

fn parse_domain_list(variable: EnvVar) -> BTreeSet<String> {
    agent_env::get_non_empty(variable)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .collect()
}

fn domain_allowed(host: &str, allowed_domains: &BTreeSet<String>) -> bool {
    if allowed_domains.is_empty() {
        return true;
    }
    let host = host.to_ascii_lowercase();
    allowed_domains
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn domain_blocked(host: &str, blocked_domains: &BTreeSet<String>) -> bool {
    let host = host.to_ascii_lowercase();
    blocked_domains
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn is_private_or_local_host(host: &str) -> bool {
    let normalized = host.trim().trim_matches('.').to_ascii_lowercase();
    if normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
    {
        return true;
    }
    if let Ok(ip) = normalized.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(ip) => {
                ip.is_loopback()
                    || ip.is_private()
                    || ip.is_link_local()
                    || ip.is_multicast()
                    || ip.is_unspecified()
            }
            IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.is_multicast()
                    || ip.is_unspecified()
            }
        };
    }
    false
}

fn decode_single_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "#39" | "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let number = if let Some(hex) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok()
            } else if let Some(decimal) = entity.strip_prefix('#') {
                decimal.parse::<u32>().ok()
            } else {
                None
            }?;
            char::from_u32(number)
        }
    }
}

fn normalize_whitespace(value: &str) -> String {
    value
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        RedirectValidationScope, WebToolPolicy, decode_html_entities, html_to_text,
        validate_redirect_attempt,
    };
    use reqwest::Url;
    use std::collections::BTreeSet;

    #[test]
    fn html_to_text_strips_markup_and_scripts() {
        let html = r#"
            <html>
              <head><title>ignored</title><script>alert(1)</script></head>
              <body><h1>Hello&nbsp;world</h1><p>alpha <b>beta</b></p><ul><li>item</li></ul></body>
            </html>
        "#;

        let text = html_to_text(html);
        assert!(text.contains("Hello world"));
        assert!(text.contains("alpha beta"));
        assert!(text.contains("- item"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn decode_html_entities_handles_named_and_numeric_values() {
        assert_eq!(decode_html_entities("&amp;&#39;&#x41;"), "&'A");
    }

    #[test]
    fn policy_blocks_private_hosts_and_respects_allowlists() {
        let policy = WebToolPolicy {
            allow_private_hosts: false,
            allowed_domains: BTreeSet::from(["example.com".to_string()]),
            blocked_domains: BTreeSet::new(),
        };

        assert!(
            policy
                .validate_target_url(&Url::parse("https://docs.example.com/page").unwrap())
                .is_ok()
        );
        assert!(
            policy
                .validate_target_url(&Url::parse("http://127.0.0.1/test").unwrap())
                .is_err()
        );
        assert!(
            policy
                .validate_target_url(&Url::parse("https://other.test").unwrap())
                .is_err()
        );
    }

    #[test]
    fn redirect_transport_scope_ignores_target_allowlists() {
        let policy = WebToolPolicy {
            allow_private_hosts: false,
            allowed_domains: BTreeSet::from(["example.com".to_string()]),
            blocked_domains: BTreeSet::from(["blocked.example.com".to_string()]),
        };

        assert!(
            validate_redirect_attempt(
                &policy,
                RedirectValidationScope::Transport,
                &Url::parse("https://search.example.net/redirect").unwrap(),
                1,
            )
            .is_ok()
        );
        assert!(
            validate_redirect_attempt(
                &policy,
                RedirectValidationScope::Transport,
                &Url::parse("http://127.0.0.1/internal").unwrap(),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn redirect_target_scope_reuses_target_policy_checks() {
        let policy = WebToolPolicy {
            allow_private_hosts: true,
            allowed_domains: BTreeSet::from(["example.com".to_string()]),
            blocked_domains: BTreeSet::from(["blocked.example.com".to_string()]),
        };

        assert!(
            validate_redirect_attempt(
                &policy,
                RedirectValidationScope::Target,
                &Url::parse("https://docs.example.com/page").unwrap(),
                1,
            )
            .is_ok()
        );
        assert!(
            validate_redirect_attempt(
                &policy,
                RedirectValidationScope::Target,
                &Url::parse("https://blocked.example.com/page").unwrap(),
                1,
            )
            .is_err()
        );
        assert!(
            validate_redirect_attempt(
                &policy,
                RedirectValidationScope::Target,
                &Url::parse("https://other.test/page").unwrap(),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn redirect_validation_enforces_hop_limit() {
        let policy = WebToolPolicy {
            allow_private_hosts: true,
            allowed_domains: BTreeSet::new(),
            blocked_domains: BTreeSet::new(),
        };

        assert!(
            validate_redirect_attempt(
                &policy,
                RedirectValidationScope::Transport,
                &Url::parse("https://example.com/next").unwrap(),
                super::DEFAULT_HTTP_REDIRECT_LIMIT + 1,
            )
            .is_err()
        );
    }
}
