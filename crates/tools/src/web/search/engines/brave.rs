use super::super::{
    BRAVE_MAX_PAGE_OFFSET, BRAVE_NEWS_PAGE_SIZE, BRAVE_WEB_PAGE_SIZE, SearchBackendResponse,
    SearchResultItem, WebSearchBackend, WebSearchBackendCapabilities, WebSearchFreshness,
    WebSearchRequest, WebSearchSourceMode, canonicalize_result_url, send_search_request,
};
use crate::web::common::{WebToolPolicy, summarize_remote_body};
use crate::{Result, ToolError};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde_json::Value;

#[derive(Clone, Debug)]
pub(crate) struct BraveApiSearchBackend {
    base_url: Url,
    api_key: String,
}

impl BraveApiSearchBackend {
    pub(crate) fn new(base_url: Url, api_key: String) -> Self {
        Self { base_url, api_key }
    }

    fn page_size(&self, source_mode: &WebSearchSourceMode) -> usize {
        match source_mode {
            WebSearchSourceMode::General => BRAVE_WEB_PAGE_SIZE,
            WebSearchSourceMode::News => BRAVE_NEWS_PAGE_SIZE,
        }
    }

    fn build_request_url(&self, request: &WebSearchRequest, page_index: usize) -> Result<Url> {
        let path = match request.source_mode {
            WebSearchSourceMode::General => "/res/v1/web/search",
            WebSearchSourceMode::News => "/res/v1/news/search",
        };
        let mut request_url = self
            .base_url
            .join(path)
            .map_err(|error| ToolError::invalid(format!("invalid Brave API endpoint: {error}")))?;
        let search_lang = request
            .locale
            .language
            .split('-')
            .next()
            .unwrap_or("en")
            .to_ascii_lowercase();
        let country = request.locale.country.to_ascii_uppercase();
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .append_pair("q", &request.query)
            .append_pair("country", &country)
            .append_pair("search_lang", &search_lang)
            .append_pair("count", &self.page_size(&request.source_mode).to_string())
            .append_pair("offset", &page_index.to_string())
            .append_pair("extra_snippets", "true");
        if let Some(freshness) = brave_freshness_value(&request.freshness) {
            request_url
                .query_pairs_mut()
                .append_pair("freshness", freshness);
        }
        Ok(request_url)
    }
}

#[async_trait]
impl WebSearchBackend for BraveApiSearchBackend {
    fn backend_name(&self) -> &'static str {
        "brave_api"
    }

    fn retrieval_mode(&self) -> &'static str {
        "json_api"
    }

    fn capabilities(&self) -> WebSearchBackendCapabilities {
        WebSearchBackendCapabilities {
            locale: true,
            freshness: true,
            source_mode: true,
            pagination: true,
            extra_snippets: true,
        }
    }

    async fn search(
        &self,
        client: &Client,
        policy: &WebToolPolicy,
        request: &WebSearchRequest,
    ) -> Result<SearchBackendResponse> {
        let page_size = self.page_size(&request.source_mode);
        let first_page = request.offset / page_size;
        if first_page > BRAVE_MAX_PAGE_OFFSET {
            return Err(ToolError::invalid(format!(
                "requested offset {} exceeds the hosted search backend limit of {} results",
                request.offset,
                (BRAVE_MAX_PAGE_OFFSET + 1) * page_size - 1
            )));
        }

        let required_results =
            request.offset.saturating_sub(first_page * page_size) + request.limit;
        let mut request_urls = Vec::new();
        let mut final_urls = Vec::new();
        let mut aggregated_results = Vec::new();
        let mut last_status = 200u16;
        let mut last_content_type = None;
        let mut last_body = String::new();
        let mut more_results_available = None;

        // Brave paginates by page index rather than row offset. We translate the
        // substrate's row-based contract into as many page fetches as required to
        // satisfy the requested window, while preserving the original model-facing
        // `offset`/`limit` semantics at the tool boundary.
        for page_index in first_page..=BRAVE_MAX_PAGE_OFFSET {
            if aggregated_results.len() >= required_results {
                break;
            }

            let request_url = self.build_request_url(request, page_index)?;
            let (final_url, status, content_type, body) =
                send_search_request(client, policy, request_url.clone(), Some(&self.api_key))
                    .await?;
            request_urls.push(request_url);
            final_urls.push(final_url);
            last_status = status;
            last_content_type = content_type.clone();
            last_body = body.clone();

            if !(200..300).contains(&status) {
                return Ok(SearchBackendResponse {
                    request_urls,
                    final_urls,
                    offset_base: first_page * page_size,
                    status,
                    content_type,
                    body,
                    results: Vec::new(),
                    more_results_available: Some(false),
                });
            }

            let parsed_page = parse_brave_results(&body, &request.source_mode)?;
            aggregated_results.extend(parsed_page.results);
            more_results_available = parsed_page.more_results_available;
            if !more_results_available.unwrap_or(false) {
                break;
            }
        }

        Ok(SearchBackendResponse {
            request_urls,
            final_urls,
            offset_base: first_page * page_size,
            status: last_status,
            content_type: last_content_type,
            body: last_body,
            results: aggregated_results,
            more_results_available,
        })
    }
}

#[derive(Clone, Debug, Default)]
struct BraveParsedPage {
    results: Vec<SearchResultItem>,
    more_results_available: Option<bool>,
}

fn parse_brave_results(body: &str, source_mode: &WebSearchSourceMode) -> Result<BraveParsedPage> {
    let payload: Value = serde_json::from_str(body)
        .map_err(|error| ToolError::invalid(format!("invalid Brave search response: {error}")))?;

    // Brave exposes different top-level containers between general web and news
    // endpoints. We intentionally parse by stable field families so backend
    // upgrades do not force the substrate contract to chase one exact envelope.
    let results = brave_result_items(&payload, source_mode)
        .iter()
        .filter_map(parse_brave_result_item)
        .collect::<Vec<_>>();
    let more_results_available = payload
        .pointer("/query/more_results_available")
        .and_then(Value::as_bool);

    Ok(BraveParsedPage {
        results,
        more_results_available,
    })
}

fn brave_result_items<'a>(payload: &'a Value, source_mode: &WebSearchSourceMode) -> &'a [Value] {
    let candidates = match source_mode {
        WebSearchSourceMode::General => ["/web/results", "/results", "/news/results"].as_slice(),
        WebSearchSourceMode::News => ["/results", "/news/results", "/web/results"].as_slice(),
    };

    for pointer in candidates {
        if let Some(results) = payload.pointer(pointer).and_then(Value::as_array) {
            return results;
        }
    }
    &[]
}

fn parse_brave_result_item(item: &Value) -> Option<SearchResultItem> {
    let title = json_string(item, &["/title"])?;
    let original_url = json_string(item, &["/url"])?;
    let (url, raw_url) = canonicalize_result_url(&original_url);
    let mut extra_snippets = json_string_array(item, &["/extra_snippets"]);
    let snippet = json_string(item, &["/description", "/snippet"])
        .map(|value| summarize_remote_body(&value, None));
    extra_snippets = extra_snippets
        .into_iter()
        .map(|value| summarize_remote_body(&value, None))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let published_at = json_string(
        item,
        &["/page_age", "/age", "/published", "/date", "/published_at"],
    );
    let source_name = json_string(
        item,
        &[
            "/source",
            "/publisher",
            "/profile/name",
            "/meta_url/hostname",
            "/meta_url/netloc",
        ],
    );

    Some(SearchResultItem {
        title: summarize_remote_body(&title, None),
        url,
        raw_url,
        snippet,
        extra_snippets,
        published_at,
        source_name,
    })
}

fn json_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value.pointer(pointer).and_then(|candidate| {
            candidate
                .as_str()
                .map(str::trim)
                .filter(|candidate| !candidate.is_empty())
                .map(ToOwned::to_owned)
        })
    })
}

fn json_string_array(value: &Value, pointers: &[&str]) -> Vec<String> {
    pointers
        .iter()
        .find_map(|pointer| {
            value.pointer(pointer).and_then(|candidate| {
                candidate.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
            })
        })
        .unwrap_or_default()
}

fn brave_freshness_value(freshness: &WebSearchFreshness) -> Option<&'static str> {
    match freshness {
        WebSearchFreshness::AnyTime => None,
        WebSearchFreshness::PastDay => Some("pd"),
        WebSearchFreshness::PastWeek => Some("pw"),
        WebSearchFreshness::PastMonth => Some("pm"),
        WebSearchFreshness::PastYear => Some("py"),
    }
}
