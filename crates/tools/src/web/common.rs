use crate::{Result, ToolError};
use agent_env::{self, EnvVar, vars};
use regex::Regex;
use reqwest::{Client, Url, redirect::Policy};
use schemars::JsonSchema;
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
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

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub(crate) struct ExtractedWebDocument {
    pub blocks: Vec<WebDocumentBlockRecord>,
    pub links: Vec<WebDocumentLink>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct WebDocumentLink {
    pub id: String,
    pub href: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct WebDocumentBlockRecord {
    pub id: String,
    pub start_index: usize,
    pub end_index: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citation_ids: Vec<String>,
    pub content: WebDocumentBlock,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WebDocumentBlock {
    Heading { level: u8, text: String },
    Paragraph { text: String },
    List { ordered: bool, items: Vec<String> },
    CodeBlock { text: String },
    Table { rows: Vec<Vec<String>> },
}

#[derive(Clone, Debug)]
struct CollectedDocumentBlock {
    content: WebDocumentBlock,
    link_hrefs: Vec<String>,
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
    extract_html_document(html, None).text
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

#[must_use]
pub(crate) fn extract_html_document(html: &str, base_url: Option<&Url>) -> ExtractedWebDocument {
    // Keep extraction deterministic and auditable: we derive a block-oriented
    // document model directly from the DOM instead of running opaque readability
    // scoring. That preserves structure for agents and hosts without hiding why
    // a given heading, list, code block, or table survived extraction.
    let document = Html::parse_document(html);
    let root = document
        .select(body_selector())
        .next()
        .unwrap_or_else(|| document.root_element());

    let mut collected_blocks = Vec::new();
    collect_document_blocks(&root, base_url, &mut collected_blocks);

    let mut links = Vec::new();
    let mut seen_links = BTreeSet::new();
    collect_document_links(&root, base_url, &mut seen_links, &mut links);
    let link_ids_by_href = links
        .iter()
        .map(|link| (link.href.clone(), link.id.clone()))
        .collect::<BTreeMap<_, _>>();
    let (text, blocks) = render_document_blocks(&collected_blocks, &link_ids_by_href);

    ExtractedWebDocument {
        text,
        blocks,
        links,
    }
}

fn body_selector() -> &'static Selector {
    static BODY_SELECTOR: OnceLock<Selector> = OnceLock::new();
    BODY_SELECTOR.get_or_init(|| Selector::parse("body").expect("body selector"))
}

fn link_selector() -> &'static Selector {
    static LINK_SELECTOR: OnceLock<Selector> = OnceLock::new();
    LINK_SELECTOR.get_or_init(|| Selector::parse("a[href]").expect("link selector"))
}

fn table_row_selector() -> &'static Selector {
    static TABLE_ROW_SELECTOR: OnceLock<Selector> = OnceLock::new();
    TABLE_ROW_SELECTOR.get_or_init(|| Selector::parse("tr").expect("table row selector"))
}

fn table_cell_selector() -> &'static Selector {
    static TABLE_CELL_SELECTOR: OnceLock<Selector> = OnceLock::new();
    TABLE_CELL_SELECTOR.get_or_init(|| Selector::parse("th, td").expect("table cell selector"))
}

fn collect_document_blocks(
    root: &ElementRef<'_>,
    base_url: Option<&Url>,
    blocks: &mut Vec<CollectedDocumentBlock>,
) {
    for child in root.children() {
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        push_element_blocks(&element, base_url, blocks);
    }
}

fn push_element_blocks(
    element: &ElementRef<'_>,
    base_url: Option<&Url>,
    blocks: &mut Vec<CollectedDocumentBlock>,
) {
    let name = element.value().name();
    if is_hidden_html_tag(name) {
        return;
    }

    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let text = normalize_descendant_text(element);
            if !text.is_empty() {
                let level = name
                    .strip_prefix('h')
                    .and_then(|value| value.parse::<u8>().ok())
                    .unwrap_or(1);
                blocks.push(CollectedDocumentBlock {
                    content: WebDocumentBlock::Heading { level, text },
                    link_hrefs: collect_element_link_hrefs(element, base_url),
                });
            }
        }
        "p" | "blockquote" => {
            let text = normalize_descendant_text(element);
            if !text.is_empty() {
                blocks.push(CollectedDocumentBlock {
                    content: WebDocumentBlock::Paragraph { text },
                    link_hrefs: collect_element_link_hrefs(element, base_url),
                });
            }
        }
        "ul" | "ol" => {
            let items = element
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|child| child.value().name() == "li")
                .map(|child| normalize_descendant_text(&child))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>();
            if !items.is_empty() {
                blocks.push(CollectedDocumentBlock {
                    content: WebDocumentBlock::List {
                        ordered: name == "ol",
                        items,
                    },
                    link_hrefs: collect_element_link_hrefs(element, base_url),
                });
            }
        }
        "pre" => {
            let text = extract_preformatted_text(element);
            if !text.is_empty() {
                blocks.push(CollectedDocumentBlock {
                    content: WebDocumentBlock::CodeBlock { text },
                    link_hrefs: collect_element_link_hrefs(element, base_url),
                });
            }
        }
        "table" => {
            let rows = element
                .select(table_row_selector())
                .map(|row| {
                    row.select(table_cell_selector())
                        .map(|cell| normalize_descendant_text(&cell))
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                })
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                blocks.push(CollectedDocumentBlock {
                    content: WebDocumentBlock::Table { rows },
                    link_hrefs: collect_element_link_hrefs(element, base_url),
                });
            }
        }
        _ => collect_document_blocks(element, base_url, blocks),
    }
}

fn collect_document_links(
    root: &ElementRef<'_>,
    base_url: Option<&Url>,
    seen_links: &mut BTreeSet<String>,
    links: &mut Vec<WebDocumentLink>,
) {
    for link in root.select(link_selector()) {
        let Some(raw_href) = link.value().attr("href") else {
            continue;
        };
        let href = resolve_document_href(raw_href, base_url);
        if href.is_empty() || !seen_links.insert(href.clone()) {
            continue;
        }
        let text = normalize_descendant_text(&link);
        links.push(WebDocumentLink {
            id: stable_document_link_id(&href),
            href,
            text: (!text.is_empty()).then_some(text),
        });
    }
}

fn collect_element_link_hrefs(element: &ElementRef<'_>, base_url: Option<&Url>) -> Vec<String> {
    let mut hrefs = Vec::new();
    let mut seen = BTreeSet::new();
    for link in element.select(link_selector()) {
        let Some(raw_href) = link.value().attr("href") else {
            continue;
        };
        let href = resolve_document_href(raw_href, base_url);
        if href.is_empty() || !seen.insert(href.clone()) {
            continue;
        }
        hrefs.push(href);
    }
    hrefs
}

fn resolve_document_href(href: &str, base_url: Option<&Url>) -> String {
    let href = href.trim();
    if href.is_empty() {
        return String::new();
    }
    if let Some(base_url) = base_url
        && let Ok(resolved) = base_url.join(href)
    {
        return resolved.to_string();
    }
    Url::parse(href)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| href.to_string())
}

fn normalize_descendant_text(element: &ElementRef<'_>) -> String {
    normalize_inline_text(&element.text().collect::<Vec<_>>().join(" "))
}

fn extract_preformatted_text(element: &ElementRef<'_>) -> String {
    let normalized = element
        .text()
        .collect::<Vec<_>>()
        .join("")
        .replace("\r\n", "\n");
    let trimmed = normalized.trim_matches('\n');
    let lines = trimmed
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    if lines.trim().is_empty() {
        String::new()
    } else {
        lines
    }
}

fn render_document_blocks(
    blocks: &[CollectedDocumentBlock],
    link_ids_by_href: &BTreeMap<String, String>,
) -> (String, Vec<WebDocumentBlockRecord>) {
    let mut rendered = String::new();
    let mut rendered_chars = 0usize;
    let mut records = Vec::new();

    for (index, block) in blocks.iter().enumerate() {
        let block_text = render_document_block(&block.content);
        if block_text.is_empty() {
            continue;
        }
        if !rendered.is_empty() {
            rendered.push_str("\n\n");
            rendered_chars += 2;
        }

        let start_index = rendered_chars;
        rendered.push_str(&block_text);
        rendered_chars += block_text.chars().count();
        let end_index = rendered_chars;
        let citation_ids = block
            .link_hrefs
            .iter()
            .filter_map(|href| link_ids_by_href.get(href).cloned())
            .collect::<Vec<_>>();

        records.push(WebDocumentBlockRecord {
            id: format!("blk_{:04}", index + 1),
            start_index,
            end_index,
            citation_ids,
            content: block.content.clone(),
        });
    }

    (rendered, records)
}

fn render_document_block(block: &WebDocumentBlock) -> String {
    match block {
        WebDocumentBlock::Heading { level, text } => {
            format!("{} {text}", "#".repeat((*level).into()))
        }
        WebDocumentBlock::Paragraph { text } => text.clone(),
        WebDocumentBlock::List { ordered, items } => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                if *ordered {
                    format!("{}. {item}", index + 1)
                } else {
                    format!("- {item}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        WebDocumentBlock::CodeBlock { text } => format!("```text\n{text}\n```"),
        WebDocumentBlock::Table { rows } => rows
            .iter()
            .map(|row| row.join(" | "))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn stable_document_link_id(href: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(href.as_bytes());
    let digest = hasher.finalize();
    let mut output = String::from("wlnk_");
    for byte in digest.iter().take(8) {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn is_hidden_html_tag(name: &str) -> bool {
    matches!(
        name,
        "head" | "script" | "style" | "noscript" | "svg" | "canvas" | "iframe" | "template"
    )
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

fn normalize_inline_text(value: &str) -> String {
    static TRAILING_SPACE_BEFORE_PUNCT_RE: OnceLock<Regex> = OnceLock::new();
    static LEADING_SPACE_AFTER_OPEN_RE: OnceLock<Regex> = OnceLock::new();

    let normalized = normalize_whitespace(value);
    let without_trailing_space = TRAILING_SPACE_BEFORE_PUNCT_RE
        .get_or_init(|| Regex::new(r"\s+([,.;:!?)\]])").expect("punctuation spacing regex"))
        .replace_all(&normalized, "$1");
    LEADING_SPACE_AFTER_OPEN_RE
        .get_or_init(|| Regex::new(r"([(\[])\s+").expect("opening punctuation spacing regex"))
        .replace_all(&without_trailing_space, "$1")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        RedirectValidationScope, WebDocumentBlock, WebToolPolicy, decode_html_entities,
        extract_html_document, html_to_text, validate_redirect_attempt,
    };
    use reqwest::Url;
    use std::collections::BTreeSet;

    #[test]
    fn html_to_text_strips_markup_and_scripts() {
        let html = r#"
            <html>
              <head><title>ignored</title><script>alert(1)</script></head>
              <body><h1>Hello&nbsp;world</h1><p>alpha <b>beta</b></p><ul><li>item</li></ul><pre>let x = 1;</pre></body>
            </html>
        "#;

        let text = html_to_text(html);
        assert!(text.contains("# Hello world"));
        assert!(text.contains("alpha beta"));
        assert!(text.contains("- item"));
        assert!(text.contains("```text\nlet x = 1;\n```"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn extract_html_document_preserves_links_and_block_types() {
        let html = r#"
            <html>
              <body>
                <h2>Overview</h2>
                <p>See <a href="/docs">the docs</a>.</p>
                <table><tr><th>Name</th><th>Value</th></tr><tr><td>alpha</td><td>1</td></tr></table>
              </body>
            </html>
        "#;

        let document = extract_html_document(
            html,
            Some(&Url::parse("https://example.com/guide").unwrap()),
        );

        assert!(document.links[0].id.starts_with("wlnk_"));
        assert_eq!(document.links[0].href, "https://example.com/docs");
        assert_eq!(document.links[0].text.as_deref(), Some("the docs"));
        assert!(matches!(
            document.blocks[0].content,
            WebDocumentBlock::Heading { level: 2, .. }
        ));
        assert_eq!(
            document.blocks[1].citation_ids,
            vec![document.links[0].id.clone()]
        );
        assert!(matches!(
            document.blocks[2].content,
            WebDocumentBlock::Table { .. }
        ));
        assert_eq!(document.blocks[0].start_index, 0);
        assert!(document.blocks[1].start_index >= document.blocks[0].end_index);
        assert!(document.text.contains("Name | Value"));
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
