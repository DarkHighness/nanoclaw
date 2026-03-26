use super::super::{
    SearchBackendResponse, SearchResultItem, WebSearchBackend, WebSearchBackendCapabilities,
    WebSearchRequest, canonicalize_result_url, send_search_request,
};
use crate::web::common::{WebToolPolicy, summarize_remote_body};
use crate::{Result, ToolError};
use async_trait::async_trait;
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub(crate) struct DuckDuckGoHtmlSearchBackend {
    endpoint: Url,
}

impl DuckDuckGoHtmlSearchBackend {
    pub(crate) fn new(endpoint: Url) -> Self {
        Self { endpoint }
    }

    fn build_request_url(&self, request: &WebSearchRequest) -> Result<Url> {
        let mut request_url = self.endpoint.clone();
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .append_pair("q", &request.query)
            .append_pair("kd", "-1");
        if request.offset > 0 {
            request_url
                .query_pairs_mut()
                .append_pair("s", &request.offset.to_string());
        }
        Ok(request_url)
    }
}

#[async_trait]
impl WebSearchBackend for DuckDuckGoHtmlSearchBackend {
    fn backend_name(&self) -> &'static str {
        "duckduckgo_html"
    }

    fn retrieval_mode(&self) -> &'static str {
        "html_scrape"
    }

    fn capabilities(&self) -> WebSearchBackendCapabilities {
        WebSearchBackendCapabilities {
            locale: false,
            freshness: false,
            source_mode: false,
            pagination: true,
            extra_snippets: false,
        }
    }

    async fn search(
        &self,
        client: &Client,
        policy: &WebToolPolicy,
        request: &WebSearchRequest,
    ) -> Result<SearchBackendResponse> {
        let request_url = self.build_request_url(request)?;
        let (final_url, status, content_type, body) =
            send_search_request(client, policy, request_url.clone(), None).await?;
        let results = if (200..300).contains(&status) {
            parse_duckduckgo_results(&body)?
        } else {
            Vec::new()
        };
        Ok(SearchBackendResponse {
            request_urls: vec![request_url],
            final_urls: vec![final_url],
            offset_base: request.offset,
            status,
            content_type,
            body,
            more_results_available: duckduckgo_more_results_available(&results, request),
            results,
        })
    }
}

fn parse_duckduckgo_results(body: &str) -> Result<Vec<SearchResultItem>> {
    if duckduckgo_challenge_present(body) {
        // Returning an explicit error keeps the runtime from mistaking a bot wall
        // for an empty but successful result set.
        return Err(ToolError::invalid(
            "DuckDuckGo HTML search returned a bot challenge instead of search results",
        ));
    }

    let document = Html::parse_document(body);
    let mut results = Vec::new();
    for container in document.select(result_selector()) {
        let Some(link) = container.select(result_link_selector()).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let title = summarize_remote_body(&link.text().collect::<Vec<_>>().join(" "), None);
        if title.is_empty() {
            continue;
        }
        let (url, raw_url) = canonicalize_result_url(href);
        let snippet = container
            .select(result_snippet_selector())
            .next()
            .map(extract_element_text)
            .filter(|value| !value.is_empty());
        let source_name = container
            .select(result_source_selector())
            .next()
            .map(extract_element_text)
            .filter(|value| !value.is_empty());

        results.push(SearchResultItem {
            title,
            url,
            raw_url,
            snippet,
            extra_snippets: Vec::new(),
            published_at: None,
            source_name,
        });
    }

    Ok(results)
}

fn duckduckgo_more_results_available(
    results: &[SearchResultItem],
    request: &WebSearchRequest,
) -> Option<bool> {
    (!results.is_empty() && results.len() >= request.limit).then_some(true)
}

fn duckduckgo_challenge_present(body: &str) -> bool {
    let normalized = body.to_ascii_lowercase();
    normalized.contains("anomaly-modal")
        || normalized.contains("bots use duckduckgo too")
        || normalized.contains("please complete the following challenge")
}

fn result_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| {
        Selector::parse("div.result, div.result.results_links, div.web-result").expect("selector")
    })
}

fn result_link_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| Selector::parse("a.result__a").expect("selector"))
}

fn result_snippet_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| {
        Selector::parse(".result__snippet, a.result__snippet, div.result__snippet")
            .expect("selector")
    })
}

fn result_source_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| {
        Selector::parse(".result__extras__url, .result__url, span.result__url").expect("selector")
    })
}

fn extract_element_text(element: ElementRef<'_>) -> String {
    summarize_remote_body(&element.text().collect::<Vec<_>>().join(" "), None)
}
