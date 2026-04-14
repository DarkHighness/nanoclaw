use super::super::{
    SearchBackendResponse, SearchResultItem, TAVILY_MAX_RESULTS, WebSearchBackend,
    WebSearchBackendCapabilities, WebSearchFreshness, WebSearchRequest, WebSearchSourceMode,
    canonicalize_result_url, result_domain, send_search_json_request,
};
use crate::web::common::WebToolPolicy;
use crate::{Result, ToolError};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub(crate) struct TavilyApiSearchBackend {
    base_url: Url,
    api_key: String,
}

impl TavilyApiSearchBackend {
    pub(crate) fn new(base_url: Url, api_key: String) -> Self {
        Self { base_url, api_key }
    }

    fn request_url(&self) -> Result<Url> {
        self.base_url
            .join("/search")
            .map_err(|error| ToolError::invalid(format!("invalid Tavily API endpoint: {error}")))
    }

    fn request_result_count(&self, request: &WebSearchRequest) -> usize {
        request
            .offset
            .saturating_add(request.limit)
            .clamp(1, TAVILY_MAX_RESULTS)
    }

    fn request_body(&self, request: &WebSearchRequest) -> Value {
        let mut body = json!({
            "query": request.query,
            "topic": match request.source_mode {
                WebSearchSourceMode::General => "general",
                WebSearchSourceMode::News => "news",
            },
            "search_depth": "basic",
            "max_results": self.request_result_count(request),
            "include_answer": false,
            "include_raw_content": false,
            "include_favicon": false
        });
        if let Some(time_range) = tavily_time_range(&request.freshness) {
            body["time_range"] = Value::String(time_range.to_string());
        }
        body
    }
}

#[async_trait]
impl WebSearchBackend for TavilyApiSearchBackend {
    fn backend_name(&self) -> &'static str {
        "tavily_api"
    }

    fn retrieval_mode(&self) -> &'static str {
        "json_api"
    }

    fn capabilities(&self) -> WebSearchBackendCapabilities {
        WebSearchBackendCapabilities {
            locale: false,
            freshness: true,
            source_mode: true,
            pagination: false,
            extra_snippets: false,
        }
    }

    async fn search(
        &self,
        client: &Client,
        policy: &WebToolPolicy,
        request: &WebSearchRequest,
    ) -> Result<SearchBackendResponse> {
        let request_url = self.request_url()?;
        let authorization = format!("Bearer {}", self.api_key);
        let requested_result_count = self.request_result_count(request);
        let (final_url, status, content_type, body) = send_search_json_request(
            client,
            policy,
            request_url.clone(),
            ("Authorization", authorization.as_str()),
            self.request_body(request),
        )
        .await?;
        let results = if (200..300).contains(&status) {
            parse_tavily_results(&body)?
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
                results.len() >= requested_result_count
                    && requested_result_count < TAVILY_MAX_RESULTS,
            ),
            results,
        })
    }
}

fn parse_tavily_results(body: &str) -> Result<Vec<SearchResultItem>> {
    let payload: Value = serde_json::from_str(body)
        .map_err(|error| ToolError::invalid(format!("invalid Tavily search response: {error}")))?;
    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    Ok(results
        .iter()
        .filter_map(parse_tavily_result_item)
        .collect())
}

fn parse_tavily_result_item(item: &Value) -> Option<SearchResultItem> {
    let title = item.get("title")?.as_str()?.trim();
    let original_url = item.get("url")?.as_str()?.trim();
    if title.is_empty() || original_url.is_empty() {
        return None;
    }

    let (url, raw_url) = canonicalize_result_url(original_url);
    Some(SearchResultItem {
        title: title.to_string(),
        url: url.clone(),
        raw_url,
        snippet: item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        extra_snippets: Vec::new(),
        published_at: item
            .get("published_date")
            .or_else(|| item.get("published_at"))
            .and_then(Value::as_str)
            .map(str::to_string),
        source_name: result_domain(&url),
    })
}

fn tavily_time_range(freshness: &WebSearchFreshness) -> Option<&'static str> {
    match freshness {
        WebSearchFreshness::AnyTime => None,
        WebSearchFreshness::PastDay => Some("day"),
        WebSearchFreshness::PastWeek => Some("week"),
        WebSearchFreshness::PastMonth => Some("month"),
        WebSearchFreshness::PastYear => Some("year"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_tavily_results;

    #[test]
    fn parses_tavily_results() {
        let results = parse_tavily_results(
            r#"{
                "results": [
                    {
                        "title": "Tavily result",
                        "url": "https://example.com/post",
                        "content": "Result summary",
                        "published_date": "2026-04-14"
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Tavily result");
        assert_eq!(results[0].url, "https://example.com/post");
        assert_eq!(results[0].snippet.as_deref(), Some("Result summary"));
        assert_eq!(results[0].published_at.as_deref(), Some("2026-04-14"));
    }
}
