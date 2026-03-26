use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use crate::web::common::{
    DEFAULT_HTTP_TIMEOUT_MS, WebToolPolicy, clamped_search_limit, decode_html_entities,
    default_http_client, summarize_remote_body, truncate_text,
};
use crate::{Result, ToolExecutionContext};
use agent_env::vars;
use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Url};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_SEARCH_ENDPOINT: &str = "https://www.bing.com/search";
const DEFAULT_RESULT_SNIPPET_MAX_CHARS: usize = 280;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WebSearchToolInput {
    pub query: String,
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub domains: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchResultItem {
    title: String,
    url: String,
    snippet: Option<String>,
    published_at: Option<String>,
}

#[derive(Clone, Debug)]
struct SearchResultRecord {
    id: String,
    rank: usize,
    domain: Option<String>,
    title: String,
    url: String,
    snippet: Option<String>,
    published_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WebSearchTool {
    client: Client,
    policy: WebToolPolicy,
    endpoint: Url,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_settings(
            WebToolPolicy::default(),
            DEFAULT_HTTP_TIMEOUT_MS,
            agent_env::get_non_empty(vars::AGENT_CORE_WEB_SEARCH_ENDPOINT),
        )
        .expect("web search client")
    }

    pub(crate) fn with_settings(
        policy: WebToolPolicy,
        timeout_ms: u64,
        endpoint: Option<String>,
    ) -> Result<Self> {
        let endpoint = endpoint.unwrap_or_else(|| DEFAULT_SEARCH_ENDPOINT.to_string());
        Ok(Self {
            client: default_http_client(timeout_ms)?,
            policy,
            endpoint: Url::parse(&endpoint)?,
        })
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".to_string(),
            description: "Search the public web and return result titles, URLs, and snippets. Supports per-call domain filtering before follow-up web_fetch calls.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WebSearchToolInput))
                .expect("web_search schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Search Web", true, false, false, true),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: WebSearchToolInput = serde_json::from_value(arguments)?;
        let query = input.query.trim();
        if query.is_empty() {
            return Ok(ToolResult::error(
                call_id,
                "web_search",
                "Query must not be empty",
            ));
        }

        let domains = normalize_domains(input.domains);
        let limit = clamped_search_limit(input.limit);
        let offset = input.offset.unwrap_or(0);
        let request_query = augment_query_with_domains(query, &domains);

        let mut request_url = self.endpoint.clone();
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .append_pair("format", "rss")
            .append_pair("cc", "us")
            .append_pair("setlang", "en-US")
            .append_pair("q", &request_query);

        if let Err(error) = self.policy.validate_transport_url(&request_url) {
            return Ok(ToolResult::error(call_id, "web_search", error.to_string()));
        }

        let response = match self.client.get(request_url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ToolResult::error(
                    call_id,
                    "web_search",
                    format!("Failed to search the web for `{query}`: {error}"),
                ));
            }
        };
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                return Ok(ToolResult::error(
                    call_id,
                    "web_search",
                    format!("Failed to read search results for `{query}`: {error}"),
                ));
            }
        };

        if !status.is_success() {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.clone(),
                tool_name: "web_search".to_string(),
                parts: vec![MessagePart::text(format!(
                    "query> {query}\nstatus> {status}\n\n{}",
                    summarize_remote_body(&body, content_type.as_deref())
                ))],
                metadata: Some(serde_json::json!({
                    "query": query,
                    "status": status.as_u16(),
                    "content_type": content_type,
                    "request_url": request_url.as_str(),
                    "final_url": final_url.as_str(),
                })),
                is_error: true,
            });
        }

        let filtered_results = parse_rss_results(&body)
            .into_iter()
            .filter(|item| matches_policy(item, &self.policy))
            .filter(|item| matches_domains(item, &domains))
            .collect::<Vec<_>>();
        let filtered_total = filtered_results.len();
        let offset = offset.min(filtered_total);
        let paged_results = filtered_results
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let result_records = paged_results
            .iter()
            .enumerate()
            .map(|(index, item)| SearchResultRecord {
                id: stable_result_id(item),
                rank: offset + index + 1,
                domain: result_domain(&item.url),
                title: item.title.clone(),
                url: item.url.clone(),
                snippet: item.snippet.clone(),
                published_at: item.published_at.clone(),
            })
            .collect::<Vec<_>>();
        let unique_domains = unique_domains(&result_records);
        let next_offset = (offset + result_records.len() < filtered_total)
            .then_some(offset + result_records.len());
        let retrieved_at_unix_s = unix_timestamp_s();

        let mut sections = vec![
            format!("query> {query}"),
            format!("engine> {}", self.endpoint.host_str().unwrap_or("custom")),
            format!("limit> {limit}"),
            format!("offset> {offset}"),
        ];
        if !domains.is_empty() {
            sections.push(format!("domains> {}", domains.join(", ")));
        }
        sections.push(format!("results> {}", result_records.len()));
        sections.push(format!("total_matches> {filtered_total}"));
        if result_records.is_empty() {
            sections.push(String::new());
            sections.push("No search results matched the current filters.".to_string());
        } else {
            sections.push(String::new());
            sections.extend(result_records.iter().map(format_result_entry));
            if let Some(next_offset) = next_offset {
                sections.push(format!(
                    "\n[more results available; continue with offset={next_offset}]"
                ));
            }
        }

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "web_search".to_string(),
            parts: vec![MessagePart::text(sections.join("\n"))],
            metadata: Some(serde_json::json!({
                "query": query,
                "request_query": request_query,
                "request_url": request_url.as_str(),
                "final_url": final_url.as_str(),
                "status": status.as_u16(),
                "content_type": content_type,
                "limit": limit,
                "offset": offset,
                "next_offset": next_offset,
                "domains": domains,
                "result_count": result_records.len(),
                "total_matches": filtered_total,
                "result_domains": unique_domains,
                "retrieved_at_unix_s": retrieved_at_unix_s,
                "policy": {
                    "allow_private_hosts": self.policy.allow_private_hosts,
                    "allowed_domains": self.policy.allowed_domains.iter().cloned().collect::<Vec<_>>(),
                    "blocked_domains": self.policy.blocked_domains.iter().cloned().collect::<Vec<_>>(),
                },
                "results": result_records.iter().map(|item| serde_json::json!({
                    "id": item.id,
                    "rank": item.rank,
                    "domain": item.domain,
                    "title": item.title,
                    "url": item.url,
                    "snippet": item.snippet,
                    "published_at": item.published_at,
                })).collect::<Vec<_>>(),
            })),
            is_error: false,
        })
    }
}

fn parse_rss_results(xml: &str) -> Vec<SearchResultItem> {
    static ITEM_RE: OnceLock<Regex> = OnceLock::new();
    ITEM_RE
        .get_or_init(|| Regex::new(r"(?is)<item>(.*?)</item>").expect("item regex"))
        .captures_iter(xml)
        .filter_map(|captures| captures.get(1))
        .filter_map(|item| {
            let raw = item.as_str();
            let title = extract_xml_field(raw, "title")?;
            let url = extract_xml_field(raw, "link")?;
            let snippet = extract_xml_field(raw, "description");
            let published_at = extract_xml_field(raw, "pubDate");
            Some(SearchResultItem {
                title: summarize_remote_body(&title, None),
                url: summarize_remote_body(&url, None),
                snippet: snippet
                    .map(|value| summarize_remote_body(&decode_html_entities(&value), None))
                    .filter(|value| !value.is_empty()),
                published_at: published_at
                    .map(|value| summarize_remote_body(&value, None))
                    .filter(|value| !value.is_empty()),
            })
        })
        .collect()
}

fn extract_xml_field(xml: &str, field: &str) -> Option<String> {
    let pattern = format!(r"(?is)<{field}>(.*?)</{field}>");
    Regex::new(&pattern)
        .ok()?
        .captures(xml)
        .and_then(|captures| captures.get(1))
        .map(|value| strip_cdata(value.as_str()).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn strip_cdata(value: &str) -> String {
    value
        .trim()
        .strip_prefix("<![CDATA[")
        .and_then(|inner| inner.strip_suffix("]]>"))
        .unwrap_or(value)
        .to_string()
}

fn normalize_domains(domains: Option<Vec<String>>) -> Vec<String> {
    let mut normalized = domains
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn augment_query_with_domains(query: &str, domains: &[String]) -> String {
    if domains.is_empty() {
        return query.to_string();
    }
    let filters = domains
        .iter()
        .map(|domain| format!("site:{domain}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("{query} ({filters})")
}

fn matches_policy(item: &SearchResultItem, policy: &WebToolPolicy) -> bool {
    Url::parse(&item.url)
        .ok()
        .and_then(|url| policy.validate_target_url(&url).ok())
        .is_some()
}

fn matches_domains(item: &SearchResultItem, domains: &[String]) -> bool {
    if domains.is_empty() {
        return true;
    }
    let Ok(url) = Url::parse(&item.url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    domains.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

fn unique_domains(results: &[SearchResultRecord]) -> Vec<String> {
    let mut domains = BTreeSet::new();
    for item in results {
        if let Some(host) = &item.domain {
            domains.insert(host.clone());
        }
    }
    domains.into_iter().collect()
}

fn format_result_entry(item: &SearchResultRecord) -> String {
    let mut entry = vec![
        format!("{}. {}", item.rank, item.title),
        format!("id: {}", item.id),
        format!("url: {}", item.url),
    ];
    if let Some(domain) = &item.domain {
        entry.push(format!("domain: {domain}"));
    }
    if let Some(snippet) = &item.snippet {
        let (snippet, truncated) = truncate_text(snippet, DEFAULT_RESULT_SNIPPET_MAX_CHARS);
        entry.push(if truncated {
            format!("snippet: {snippet}...")
        } else {
            format!("snippet: {snippet}")
        });
    }
    if let Some(published_at) = &item.published_at {
        entry.push(format!("published_at: {published_at}"));
    }
    entry.push(format!("fetch_hint: web_fetch url={}", item.url));
    entry.join("\n")
}

fn result_domain(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_ascii_lowercase()))
}

fn stable_result_id(item: &SearchResultItem) -> String {
    let mut hasher = Sha256::new();
    hasher.update(item.url.as_bytes());
    hasher.update(b"\n");
    hasher.update(item.title.as_bytes());
    let digest = hasher.finalize();
    let mut output = String::from("wsr_");
    for byte in digest.iter().take(8) {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn unix_timestamp_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{WebSearchTool, WebSearchToolInput, parse_rss_results};
    use crate::web::common::WebToolPolicy;
    use crate::{Tool, ToolExecutionContext};
    use std::collections::BTreeSet;
    use types::ToolCallId;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parse_rss_results_extracts_items() {
        let xml = r#"
            <rss><channel>
                <item>
                    <title>Example One</title>
                    <link>https://example.com/one</link>
                    <description><![CDATA[alpha &amp; beta]]></description>
                    <pubDate>Tue, 25 Mar 2026 09:00:00 GMT</pubDate>
                </item>
                <item>
                    <title>Example Two</title>
                    <link>https://example.com/two</link>
                </item>
            </channel></rss>
        "#;
        let results = parse_rss_results(xml);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example One");
        assert_eq!(results[0].snippet.as_deref(), Some("alpha & beta"));
    }

    #[tokio::test]
    async fn web_search_filters_by_domains() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(
                        r#"
                    <rss><channel>
                        <item>
                            <title>Wanted</title>
                            <link>https://allowed.example.com/article</link>
                            <description>keep this</description>
                        </item>
                        <item>
                            <title>Blocked</title>
                            <link>https://other.example.org/post</link>
                            <description>drop this</description>
                        </item>
                    </channel></rss>
                "#,
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebSearchTool::with_settings(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            Some(format!("{}/search", server.uri())),
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "example".to_string(),
                    limit: Some(5),
                    offset: None,
                    domains: Some(vec!["allowed.example.com".to_string()]),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("allowed.example.com/article"));
        assert!(!text.contains("other.example.org/post"));
        assert_eq!(
            result.metadata.unwrap()["domains"][0],
            "allowed.example.com"
        );
    }

    #[tokio::test]
    async fn web_search_supports_offset_pagination() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(
                        r#"
                    <rss><channel>
                        <item><title>One</title><link>https://example.com/1</link></item>
                        <item><title>Two</title><link>https://example.com/2</link></item>
                        <item><title>Three</title><link>https://example.com/3</link></item>
                    </channel></rss>
                "#,
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebSearchTool::with_settings(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            Some(format!("{}/search", server.uri())),
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "example".to_string(),
                    limit: Some(1),
                    offset: Some(1),
                    domains: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("Two"));
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["offset"], 1);
        assert_eq!(metadata["next_offset"], 2);
        assert!(
            metadata["results"][0]["id"]
                .as_str()
                .unwrap()
                .starts_with("wsr_")
        );
    }
}
