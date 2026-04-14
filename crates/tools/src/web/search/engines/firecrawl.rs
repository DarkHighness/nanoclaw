use super::super::{
    FIRECRAWL_MAX_RESULTS, SearchBackendResponse, SearchResultItem, WebSearchBackend,
    WebSearchBackendCapabilities, WebSearchFreshness, WebSearchRequest, WebSearchSourceMode,
    canonicalize_result_url, result_domain, send_search_json_request,
};
use crate::web::common::WebToolPolicy;
use crate::{Result, ToolError};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub(crate) struct FirecrawlApiSearchBackend {
    base_url: Url,
    api_key: String,
}

impl FirecrawlApiSearchBackend {
    pub(crate) fn new(base_url: Url, api_key: String) -> Self {
        Self { base_url, api_key }
    }

    fn request_url(&self) -> Result<Url> {
        self.base_url
            .join("/v2/search")
            .map_err(|error| ToolError::invalid(format!("invalid Firecrawl API endpoint: {error}")))
    }

    fn request_result_count(&self, request: &WebSearchRequest) -> usize {
        request
            .offset
            .saturating_add(request.limit)
            .clamp(1, FIRECRAWL_MAX_RESULTS)
    }

    fn request_body(&self, request: &WebSearchRequest) -> Value {
        let mut body = json!({
            "query": request.query,
            "limit": self.request_result_count(request),
            "sources": [match request.source_mode {
                WebSearchSourceMode::General => "web",
                WebSearchSourceMode::News => "news",
            }],
            "country": request.locale.country.to_ascii_uppercase(),
            "ignoreInvalidURLs": true
        });
        if let Some(tbs) = firecrawl_tbs(&request.freshness) {
            body["tbs"] = Value::String(tbs.to_string());
        }
        body
    }
}

#[async_trait]
impl WebSearchBackend for FirecrawlApiSearchBackend {
    fn backend_name(&self) -> &'static str {
        "firecrawl_api"
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
            parse_firecrawl_results(&body, &request.source_mode)?
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
                    && requested_result_count < FIRECRAWL_MAX_RESULTS,
            ),
            results,
        })
    }
}

fn parse_firecrawl_results(
    body: &str,
    source_mode: &WebSearchSourceMode,
) -> Result<Vec<SearchResultItem>> {
    let payload: Value = serde_json::from_str(body).map_err(|error| {
        ToolError::invalid(format!("invalid Firecrawl search response: {error}"))
    })?;
    let Some(data) = payload.get("data") else {
        return Ok(Vec::new());
    };
    let primary_key = match source_mode {
        WebSearchSourceMode::General => "web",
        WebSearchSourceMode::News => "news",
    };
    let results = data
        .get(primary_key)
        .and_then(Value::as_array)
        .or_else(|| data.get("web").and_then(Value::as_array))
        .or_else(|| data.get("news").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();

    Ok(results
        .iter()
        .filter_map(parse_firecrawl_result_item)
        .collect())
}

fn parse_firecrawl_result_item(item: &Value) -> Option<SearchResultItem> {
    let title = item
        .get("title")
        .or_else(|| item.pointer("/metadata/title"))
        .and_then(Value::as_str)?
        .trim();
    let original_url = item
        .get("url")
        .or_else(|| item.pointer("/metadata/sourceURL"))
        .and_then(Value::as_str)?
        .trim();
    if title.is_empty() || original_url.is_empty() {
        return None;
    }

    let (url, raw_url) = canonicalize_result_url(original_url);
    Some(SearchResultItem {
        title: title.to_string(),
        url: url.clone(),
        raw_url,
        snippet: item
            .get("description")
            .or_else(|| item.get("markdown"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        extra_snippets: Vec::new(),
        published_at: item
            .pointer("/metadata/publishedDate")
            .or_else(|| item.pointer("/metadata/date"))
            .and_then(Value::as_str)
            .map(str::to_string),
        source_name: result_domain(&url),
    })
}

fn firecrawl_tbs(freshness: &WebSearchFreshness) -> Option<&'static str> {
    match freshness {
        WebSearchFreshness::AnyTime => None,
        WebSearchFreshness::PastDay => Some("qdr:d"),
        WebSearchFreshness::PastWeek => Some("qdr:w"),
        WebSearchFreshness::PastMonth => Some("qdr:m"),
        WebSearchFreshness::PastYear => Some("qdr:y"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_firecrawl_results;
    use crate::web::search::WebSearchSourceMode;

    #[test]
    fn parses_firecrawl_web_results() {
        let results = parse_firecrawl_results(
            r#"{
                "data": {
                    "web": [
                        {
                            "title": "Firecrawl result",
                            "description": "Result snippet",
                            "url": "https://example.com/story",
                            "metadata": {
                                "publishedDate": "2026-04-14"
                            }
                        }
                    ]
                }
            }"#,
            &WebSearchSourceMode::General,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Firecrawl result");
        assert_eq!(results[0].url, "https://example.com/story");
        assert_eq!(results[0].snippet.as_deref(), Some("Result snippet"));
        assert_eq!(results[0].published_at.as_deref(), Some("2026-04-14"));
    }
}
