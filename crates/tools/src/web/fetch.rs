use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use crate::web::common::{
    DEFAULT_HTTP_TIMEOUT_MS, RedirectValidationScope, WebToolPolicy, clamped_fetch_max_chars,
    default_http_client, extract_html_title, is_text_content_type, summarize_remote_body,
    truncate_text,
};
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::{CACHE_CONTROL, CONTENT_LANGUAGE, ETAG, LAST_MODIFIED};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WebFetchToolInput {
    pub url: String,
    #[serde(default)]
    pub start_index: Option<usize>,
    pub max_chars: Option<usize>,
    #[serde(default)]
    pub expected_document_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WebFetchTool {
    client: Client,
    policy: WebToolPolicy,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WebFetchToolOutput {
    url: String,
    final_url: String,
    status: u16,
    content_type: Option<String>,
    document_id: String,
    title: Option<String>,
    start_index: usize,
    end_index: usize,
    returned_chars: usize,
    remaining_chars: usize,
    total_chars: usize,
    truncated: bool,
    max_chars: usize,
    next_start_index: Option<usize>,
    preview_text: String,
    retrieved_at_unix_s: u64,
    etag: Option<String>,
    last_modified: Option<String>,
    cache_control: Option<String>,
    content_language: Option<String>,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_policy(WebToolPolicy::default(), DEFAULT_HTTP_TIMEOUT_MS)
            .expect("web fetch client")
    }

    pub(crate) fn with_policy(policy: WebToolPolicy, timeout_ms: u64) -> Result<Self> {
        Ok(Self {
            client: default_http_client(
                timeout_ms,
                policy.clone(),
                RedirectValidationScope::Target,
            )?,
            policy,
        })
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch".to_string(),
            description: "Fetch a web page over HTTP(S), extract readable text, and return a paged text window plus metadata for continuation.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WebFetchToolInput))
                .expect("web_fetch schema"),
            output_mode: ToolOutputMode::Text,
            output_schema: Some(
                serde_json::to_value(schema_for!(WebFetchToolOutput))
                    .expect("web_fetch output schema"),
            ),
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Fetch Web Page", true, false, false, true),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: WebFetchToolInput = serde_json::from_value(arguments)?;
        let url = match reqwest::Url::parse(input.url.trim()) {
            Ok(url) => url,
            Err(error) => {
                return Ok(ToolResult::error(
                    call_id,
                    "web_fetch",
                    format!("Invalid URL: {error}"),
                ));
            }
        };
        if let Err(error) = self.policy.validate_target_url(&url) {
            return Ok(ToolResult::error(call_id, "web_fetch", error.to_string()));
        }

        let max_chars = clamped_fetch_max_chars(input.max_chars);
        let response = match self.client.get(url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ToolResult::error(
                    call_id,
                    "web_fetch",
                    format!("Failed to fetch {url}: {error}"),
                ));
            }
        };
        let status = response.status();
        let final_url = response.url().clone();
        let etag = header_to_string(response.headers(), ETAG);
        let last_modified = header_to_string(response.headers(), LAST_MODIFIED);
        let cache_control = header_to_string(response.headers(), CACHE_CONTROL);
        let content_language = header_to_string(response.headers(), CONTENT_LANGUAGE);
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
                    "web_fetch",
                    format!("Failed to read response body from {final_url}: {error}"),
                ));
            }
        };

        if !status.is_success() {
            let summary = summarize_remote_body(&body, content_type.as_deref());
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.clone(),
                tool_name: "web_fetch".to_string(),
                parts: vec![MessagePart::text(format!(
                    "url> {url}\nfinal_url> {final_url}\nstatus> {}\n\n{}",
                    status,
                    if summary.is_empty() {
                        "Remote server returned a non-success status with no readable body."
                            .to_string()
                    } else {
                        summary
                    }
                ))],
                structured_content: None,
                metadata: Some(serde_json::json!({
                    "url": url.as_str(),
                    "final_url": final_url.as_str(),
                    "status": status.as_u16(),
                    "content_type": content_type,
                    "etag": etag,
                    "last_modified": last_modified,
                    "cache_control": cache_control,
                    "content_language": content_language,
                })),
                is_error: true,
            });
        }

        if !is_text_content_type(content_type.as_deref()) {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.clone(),
                tool_name: "web_fetch".to_string(),
                parts: vec![MessagePart::text(format!(
                    "url> {url}\nfinal_url> {final_url}\nstatus> {}\ncontent_type> {}\n\nFetched a non-text response. Text extraction is currently limited to text, HTML, JSON, XML, and similar content types.",
                    status,
                    content_type.as_deref().unwrap_or("unknown"),
                ))],
                structured_content: None,
                metadata: Some(serde_json::json!({
                    "url": url.as_str(),
                    "final_url": final_url.as_str(),
                    "status": status.as_u16(),
                    "content_type": content_type,
                    "etag": etag,
                    "last_modified": last_modified,
                    "cache_control": cache_control,
                    "content_language": content_language,
                    "unsupported_content_type": true,
                })),
                is_error: true,
            });
        }

        let title = extract_html_title(&body);
        let extracted_text = summarize_remote_body(&body, content_type.as_deref());
        let extracted_text = trim_trailing_whitespace(&extracted_text);
        let document_id = stable_document_id(final_url.as_str(), &extracted_text);
        if let Some(expected_document_id) = input.expected_document_id.as_deref()
            && expected_document_id != document_id
        {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "web_fetch".to_string(),
                parts: vec![MessagePart::text(format!(
                    "url> {url}\nfinal_url> {final_url}\nstatus> {status}\nexpected_document_id> {expected_document_id}\nactual_document_id> {document_id}\n\nDocument id mismatch. The page content changed or a different resource was returned."
                ))],
                structured_content: None,
                metadata: Some(serde_json::json!({
                    "url": url.as_str(),
                    "final_url": final_url.as_str(),
                    "status": status.as_u16(),
                    "content_type": content_type,
                    "expected_document_id": expected_document_id,
                    "document_id": document_id,
                    "etag": etag,
                    "last_modified": last_modified,
                })),
                is_error: true,
            });
        }

        let total_chars = extracted_text.chars().count();
        let start_index = input.start_index.unwrap_or(0).min(total_chars);
        let skipped = extracted_text.chars().skip(start_index).collect::<String>();
        let (preview, truncated) = truncate_text(&skipped, max_chars);
        let returned_chars = preview.chars().count();
        let end_index = start_index + returned_chars;
        let next_start_index = truncated.then_some(end_index);
        let remaining_chars = total_chars.saturating_sub(end_index);
        let retrieved_at_unix_s = unix_timestamp_s();
        let structured_output = WebFetchToolOutput {
            url: url.as_str().to_string(),
            final_url: final_url.as_str().to_string(),
            status: status.as_u16(),
            content_type: content_type.clone(),
            document_id: document_id.clone(),
            title: title.clone(),
            start_index,
            end_index,
            returned_chars,
            remaining_chars,
            total_chars,
            truncated,
            max_chars,
            next_start_index,
            preview_text: preview.clone(),
            retrieved_at_unix_s,
            etag: etag.clone(),
            last_modified: last_modified.clone(),
            cache_control: cache_control.clone(),
            content_language: content_language.clone(),
        };

        let mut sections = vec![
            format!("url> {url}"),
            format!("final_url> {final_url}"),
            format!("status> {status}"),
            format!("document_id> {document_id}"),
        ];
        if let Some(content_type) = &content_type {
            sections.push(format!("content_type> {content_type}"));
        }
        if let Some(title) = &title {
            sections.push(format!("title> {title}"));
        }
        sections.push(format!("start_index> {start_index}"));
        sections.push(format!("end_index> {end_index}"));
        sections.push(format!("total_chars> {total_chars}"));
        sections.push(format!("max_chars> {max_chars}"));
        sections.push(String::new());
        sections.push(if preview.is_empty() {
            "[No readable text extracted from page]".to_string()
        } else {
            preview
        });
        if let Some(next_start_index) = next_start_index {
            sections.push(format!(
                "\n[truncated to {max_chars} characters; continue with start_index={next_start_index}]"
            ));
        }

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "web_fetch".to_string(),
            parts: vec![MessagePart::text(sections.join("\n"))],
            structured_content: Some(
                serde_json::to_value(&structured_output).expect("web_fetch structured output"),
            ),
            metadata: Some(serde_json::json!({
                "url": url.as_str(),
                "final_url": final_url.as_str(),
                "status": status.as_u16(),
                "content_type": content_type,
                "document_id": document_id,
                "etag": etag,
                "last_modified": last_modified,
                "cache_control": cache_control,
                "content_language": content_language,
                "title": title,
                "start_index": start_index,
                "end_index": end_index,
                "returned_chars": returned_chars,
                "remaining_chars": remaining_chars,
                "total_chars": total_chars,
                "truncated": truncated,
                "max_chars": max_chars,
                "next_start_index": next_start_index,
                "retrieved_at_unix_s": retrieved_at_unix_s,
            })),
            is_error: false,
        })
    }
}

fn trim_trailing_whitespace(text: &str) -> String {
    text.trim_end().to_string()
}

fn header_to_string(
    headers: &reqwest::header::HeaderMap,
    key: reqwest::header::HeaderName,
) -> Option<String> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn stable_document_id(url: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    hasher.update(b"\n");
    hasher.update(body.as_bytes());
    let digest = hasher.finalize();
    let mut output = String::from("doc_");
    for byte in digest.iter().take(10) {
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
    use super::{WebFetchTool, WebFetchToolInput};
    use crate::web::common::WebToolPolicy;
    use crate::{Tool, ToolExecutionContext};
    use std::collections::BTreeSet;
    use types::ToolCallId;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn web_fetch_extracts_readable_html() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/html").set_body_string(
                r#"<html><head><title>Example Page</title><script>bad()</script></head><body><h1>Hello</h1><p>World &amp; friends.</p></body></html>"#,
            ))
            .mount(&server)
            .await;

        let tool = WebFetchTool::with_policy(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebFetchToolInput {
                    url: format!("{}/page", server.uri()),
                    start_index: None,
                    max_chars: Some(200),
                    expected_document_id: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = result.text_content();
        assert!(text.contains("title> Example Page"));
        assert!(text.contains("Hello"));
        assert!(text.contains("World & friends."));
        assert!(!text.contains("bad()"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["title"], "Example Page");
        assert_eq!(
            structured["preview_text"].as_str().unwrap(),
            "Hello\nWorld & friends."
        );
    }

    #[tokio::test]
    async fn web_fetch_supports_continuation_with_start_index() {
        let server = MockServer::start().await;
        let body = "abcdefghij".repeat(40);
        Mock::given(method("GET"))
            .and(path("/long"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string(body.clone()),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::with_policy(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
        )
        .unwrap();
        let first = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebFetchToolInput {
                    url: format!("{}/long", server.uri()),
                    start_index: Some(0),
                    max_chars: Some(4),
                    expected_document_id: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let metadata = first.metadata.clone().unwrap();
        assert_eq!(metadata["next_start_index"], 256);
        assert!(
            metadata["document_id"]
                .as_str()
                .unwrap()
                .starts_with("doc_")
        );
        let structured = first.structured_content.clone().unwrap();
        assert_eq!(structured["next_start_index"], 256);
        assert_eq!(structured["returned_chars"], 256);

        let second = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebFetchToolInput {
                    url: format!("{}/long", server.uri()),
                    start_index: Some(4),
                    max_chars: Some(4),
                    expected_document_id: metadata["document_id"].as_str().map(ToString::to_string),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(second.text_content().contains("ghij"));
    }

    #[tokio::test]
    async fn web_fetch_rejects_private_hosts_by_default() {
        let tool = WebFetchTool::with_policy(
            WebToolPolicy {
                allow_private_hosts: false,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
        )
        .unwrap();

        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebFetchToolInput {
                    url: "http://127.0.0.1/private".to_string(),
                    start_index: None,
                    max_chars: Some(128),
                    expected_document_id: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.text_content().contains("private host"));
    }

    #[tokio::test]
    async fn web_fetch_rejects_redirects_to_blocked_hosts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/blocked", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/blocked"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("should not be reached"),
            )
            .mount(&server)
            .await;

        let start_url = format!("{}/start", server.uri()).replace("127.0.0.1", "localhost");
        let tool = WebFetchTool::with_policy(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::from(["127.0.0.1".to_string()]),
            },
            5_000,
        )
        .unwrap();

        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebFetchToolInput {
                    url: start_url,
                    start_index: None,
                    max_chars: Some(128),
                    expected_document_id: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.text_content().contains("error following redirect"));
        let received_requests = server.received_requests().await.unwrap();
        assert_eq!(received_requests.len(), 1);
        assert_eq!(received_requests[0].url.path(), "/start");
    }

    #[tokio::test]
    async fn web_fetch_detects_document_id_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/doc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("stable body"),
            )
            .mount(&server)
            .await;

        let tool = WebFetchTool::with_policy(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
        )
        .unwrap();

        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebFetchToolInput {
                    url: format!("{}/doc", server.uri()),
                    start_index: None,
                    max_chars: Some(200),
                    expected_document_id: Some("doc_wrong".to_string()),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("Document id mismatch"));
    }
}
