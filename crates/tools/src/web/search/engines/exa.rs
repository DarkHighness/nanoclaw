use super::super::{
    EXA_MAX_RESULTS, SearchBackendResponse, SearchResultItem, WebSearchBackend,
    WebSearchBackendCapabilities, WebSearchFreshness, WebSearchRequest, WebSearchSourceMode,
    canonicalize_result_url, result_domain, send_search_json_request,
};
use crate::web::common::{WebToolPolicy, summarize_remote_body};
use crate::{Result, ToolError};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Clone, Debug)]
pub(crate) struct ExaApiSearchBackend {
    base_url: Url,
    api_key: String,
}

impl ExaApiSearchBackend {
    pub(crate) fn new(base_url: Url, api_key: String) -> Self {
        Self { base_url, api_key }
    }

    fn request_url(&self) -> Result<Url> {
        self.base_url
            .join("/search")
            .map_err(|error| ToolError::invalid(format!("invalid Exa API endpoint: {error}")))
    }

    fn request_result_count(&self, request: &WebSearchRequest) -> usize {
        // Exa exposes a requested result count but no stable row-offset cursor.
        // We therefore overfetch up to the requested window and let the shared
        // tool layer apply the final deterministic `offset` slice.
        request
            .offset
            .saturating_add(request.limit)
            .clamp(1, EXA_MAX_RESULTS)
    }

    fn request_body(&self, request: &WebSearchRequest) -> Value {
        let mut body = json!({
            "query": request.query,
            "type": "auto",
            "numResults": self.request_result_count(request),
            "text": false,
            "summary": true,
            "highlights": true,
            "userLocation": {
                "country": request.locale.country.to_ascii_uppercase(),
            }
        });
        if matches!(request.source_mode, WebSearchSourceMode::News) {
            body["category"] = Value::String("news".to_string());
        }
        if let Some(start_published_date) = exa_start_published_date(&request.freshness) {
            body["startPublishedDate"] = Value::String(start_published_date);
        }
        body
    }
}

#[async_trait]
impl WebSearchBackend for ExaApiSearchBackend {
    fn backend_name(&self) -> &'static str {
        "exa_api"
    }

    fn backend_type(&self) -> super::super::WebSearchBackendType {
        super::super::WebSearchBackendType::HostedApi
    }

    fn retrieval_mode(&self) -> &'static str {
        "json_api"
    }

    fn capabilities(&self) -> WebSearchBackendCapabilities {
        WebSearchBackendCapabilities {
            locale: true,
            freshness: true,
            source_mode: true,
            pagination: false,
            extra_snippets: true,
        }
    }

    async fn search(
        &self,
        client: &Client,
        policy: &WebToolPolicy,
        request: &WebSearchRequest,
    ) -> Result<SearchBackendResponse> {
        let request_url = self.request_url()?;
        let request_body = self.request_body(request);
        let requested_result_count = self.request_result_count(request);
        let (final_url, status, content_type, body) = send_search_json_request(
            client,
            policy,
            request_url.clone(),
            ("x-api-key", &self.api_key),
            request_body,
        )
        .await?;
        let results = if (200..300).contains(&status) {
            parse_exa_results(&body)?
        } else {
            Vec::new()
        };
        Ok(SearchBackendResponse {
            request_urls: vec![request_url],
            final_urls: vec![final_url],
            offset_base: 0,
            status,
            content_type,
            body,
            more_results_available: Some(
                results.len() >= requested_result_count && requested_result_count < EXA_MAX_RESULTS,
            ),
            results,
        })
    }
}

fn parse_exa_results(body: &str) -> Result<Vec<SearchResultItem>> {
    let payload: Value = serde_json::from_str(body)
        .map_err(|error| ToolError::invalid(format!("invalid Exa search response: {error}")))?;
    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    Ok(results.iter().filter_map(parse_exa_result_item).collect())
}

fn parse_exa_result_item(item: &Value) -> Option<SearchResultItem> {
    let title = item.get("title")?.as_str()?.trim();
    let original_url = item.get("url")?.as_str()?.trim();
    if title.is_empty() || original_url.is_empty() {
        return None;
    }

    let (url, raw_url) = canonicalize_result_url(original_url);
    let mut extra_snippets = item
        .get("highlights")
        .and_then(Value::as_array)
        .map(|highlights| {
            highlights
                .iter()
                .filter_map(Value::as_str)
                .map(|value| summarize_remote_body(value, None))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let snippet = item
        .get("summary")
        .and_then(Value::as_str)
        .map(|value| summarize_remote_body(value, None))
        .filter(|value| !value.is_empty())
        .or_else(|| {
            extra_snippets
                .first()
                .cloned()
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            item.get("text")
                .and_then(Value::as_str)
                .map(|value| summarize_remote_body(value, None))
                .filter(|value| !value.is_empty())
        });
    if snippet.is_some() && !extra_snippets.is_empty() {
        extra_snippets.remove(0);
    }

    Some(SearchResultItem {
        title: summarize_remote_body(title, None),
        url: url.clone(),
        raw_url,
        snippet,
        extra_snippets,
        published_at: item
            .get("publishedDate")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_name: result_domain(&url),
    })
}

fn exa_start_published_date(freshness: &WebSearchFreshness) -> Option<String> {
    let now = OffsetDateTime::now_utc();
    let cutoff = match freshness {
        WebSearchFreshness::AnyTime => return None,
        WebSearchFreshness::PastDay => now - time::Duration::days(1),
        WebSearchFreshness::PastWeek => now - time::Duration::weeks(1),
        WebSearchFreshness::PastMonth => now - time::Duration::days(30),
        WebSearchFreshness::PastYear => now - time::Duration::days(365),
    };
    cutoff.format(&Rfc3339).ok()
}
