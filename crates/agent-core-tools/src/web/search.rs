use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use crate::web::common::{
    DEFAULT_HTTP_TIMEOUT_MS, WebToolPolicy, clamped_search_limit, decode_html_entities,
    default_http_client, summarize_remote_body, truncate_text,
};
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use reqwest::{Client, Url};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

const DEFAULT_SEARCH_ENDPOINT: &str = "https://www.bing.com/search";
const DEFAULT_RESULT_SNIPPET_MAX_CHARS: usize = 280;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WebSearchToolInput {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchResultItem {
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
            std::env::var("AGENT_CORE_WEB_SEARCH_ENDPOINT").ok(),
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
            description: "Search the public web and return result titles, URLs, and snippets. Use this to find candidate sources before calling web_fetch.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WebSearchToolInput)).expect("web_search schema"),
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
        let external_call_id = call_id.0.clone();
        let input: WebSearchToolInput = serde_json::from_value(arguments)?;
        let query = input.query.trim();
        if query.is_empty() {
            return Ok(ToolResult::error(
                call_id,
                "web_search",
                "Query must not be empty",
            ));
        }

        let mut request_url = self.endpoint.clone();
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .append_pair("format", "rss")
            .append_pair("cc", "us")
            .append_pair("setlang", "en-US")
            .append_pair("q", query);

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

        let limit = clamped_search_limit(input.limit);
        let results = parse_rss_results(&body)
            .into_iter()
            .filter(|item| {
                Url::parse(&item.url)
                    .ok()
                    .and_then(|url| self.policy.validate_target_url(&url).ok())
                    .is_some()
            })
            .take(limit)
            .collect::<Vec<_>>();

        let mut sections = vec![
            format!("query> {query}"),
            format!("engine> {}", self.endpoint.host_str().unwrap_or("custom")),
            format!("results> {}", results.len()),
        ];
        if results.is_empty() {
            sections.push(String::new());
            sections.push("No search results matched the current policy filters.".to_string());
        } else {
            sections.push(String::new());
            sections.extend(results.iter().enumerate().map(format_result_entry));
        }

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "web_search".to_string(),
            parts: vec![MessagePart::text(sections.join("\n"))],
            metadata: Some(serde_json::json!({
                "query": query,
                "request_url": request_url.as_str(),
                "final_url": final_url.as_str(),
                "status": status.as_u16(),
                "content_type": content_type,
                "results": results.iter().map(|item| serde_json::json!({
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
        .map(|value| value.as_str().trim().to_string())
        .filter(|value| !value.is_empty())
}

fn format_result_entry((index, item): (usize, &SearchResultItem)) -> String {
    let mut entry = vec![
        format!("{}. {}", index + 1, item.title),
        format!("url: {}", item.url),
    ];
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
    entry.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{WebSearchTool, WebSearchToolInput, parse_rss_results};
    use crate::web::common::WebToolPolicy;
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;
    use std::collections::BTreeSet;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parse_rss_results_extracts_items() {
        let xml = r#"
            <rss><channel>
              <item>
                <title>Rust Programming Language</title>
                <link>https://rust-lang.org/</link>
                <description>Reliable &amp; efficient software.</description>
                <pubDate>Tue, 24 Mar 2026 00:00:00 GMT</pubDate>
              </item>
            </channel></rss>
        "#;
        let items = parse_rss_results(xml);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Rust Programming Language");
        assert_eq!(items[0].url, "https://rust-lang.org/");
        assert_eq!(
            items[0].snippet.as_deref(),
            Some("Reliable & efficient software.")
        );
    }

    #[tokio::test]
    async fn web_search_formats_rss_results() {
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
                                <title>Rust Programming Language</title>
                                <link>https://rust-lang.org/</link>
                                <description>Reliable &amp; efficient software.</description>
                              </item>
                              <item>
                                <title>Rust Book</title>
                                <link>https://doc.rust-lang.org/book/</link>
                                <description>The Rust Programming Language book.</description>
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
                    query: "rust programming language".to_string(),
                    limit: Some(1),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = result.text_content();
        assert!(text.contains("query> rust programming language"));
        assert!(text.contains("1. Rust Programming Language"));
        assert!(text.contains("url: https://rust-lang.org/"));
        assert!(!text.contains("Rust Book"));
    }
}
