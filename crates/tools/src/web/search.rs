use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use crate::web::common::{
    DEFAULT_HTTP_TIMEOUT_MS, RedirectValidationScope, WebToolPolicy, clamped_search_limit,
    default_http_client, summarize_remote_body, truncate_text,
};
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use reqwest::{Client, Url};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::{Rfc2822, Rfc3339};
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

mod engines;

use engines::bing::BingRssSearchBackend;
#[cfg(test)]
use engines::bing::parse_feed_results;
use engines::brave::BraveApiSearchBackend;
use engines::duckduckgo::DuckDuckGoHtmlSearchBackend;
use engines::exa::ExaApiSearchBackend;
use engines::{WebSearchBackendKind, WebSearchBackendRegistry};

const DEFAULT_SEARCH_ENDPOINT: &str = "https://www.bing.com/search";
const DEFAULT_BRAVE_API_BASE_URL: &str = "https://api.search.brave.com";
const DEFAULT_EXA_API_BASE_URL: &str = "https://api.exa.ai";
const DEFAULT_DUCKDUCKGO_HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const DEFAULT_RESULT_SNIPPET_MAX_CHARS: usize = 280;
const BRAVE_WEB_PAGE_SIZE: usize = 20;
const BRAVE_NEWS_PAGE_SIZE: usize = 50;
const BRAVE_MAX_PAGE_OFFSET: usize = 9;
const EXA_MAX_RESULTS: usize = 100;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WebSearchToolInput {
    pub query: String,
    #[serde(default)]
    pub backend: Option<String>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub domains: Option<Vec<String>>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub freshness: Option<WebSearchFreshness>,
    #[serde(default)]
    pub source_mode: Option<WebSearchSourceMode>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchFreshness {
    AnyTime,
    PastDay,
    PastWeek,
    PastMonth,
    PastYear,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchSourceMode {
    General,
    News,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchResultItem {
    title: String,
    url: String,
    raw_url: Option<String>,
    snippet: Option<String>,
    extra_snippets: Vec<String>,
    published_at: Option<String>,
    source_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct SearchResultRecord {
    id: String,
    citation_id: String,
    rank: usize,
    domain: Option<String>,
    title: String,
    url: String,
    raw_url: Option<String>,
    snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extra_snippets: Vec<String>,
    published_at: Option<String>,
    source_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct SearchSourceRecord {
    citation_id: String,
    result_id: String,
    rank: usize,
    domain: Option<String>,
    title: String,
    url: String,
    raw_url: Option<String>,
    snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extra_snippets: Vec<String>,
    published_at: Option<String>,
    source_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WebSearchPolicyOutput {
    allow_private_hosts: bool,
    allowed_domains: Vec<String>,
    blocked_domains: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WebSearchBackendType {
    HostedApi,
    RssFeed,
    HtmlScrape,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WebSearchFreshnessMode {
    NotRequested,
    BestEffort,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WebSearchFreshnessOutput {
    requested: WebSearchFreshness,
    mode: WebSearchFreshnessMode,
    cutoff_unix_s: Option<i64>,
    dropped_results: usize,
    kept_without_timestamp: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct WebSearchBackendsToolInput {
    #[serde(default = "default_true")]
    pub include_unconfigured: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WebSearchBackendCatalogRecord {
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    selector_aliases: Vec<String>,
    backend_type: WebSearchBackendType,
    configured: bool,
    selected_by_default: bool,
    auto_priority: usize,
    retrieval_mode: String,
    capabilities: WebSearchBackendCapabilities,
    missing_requirement: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WebSearchBackendsToolOutput {
    default_selector: String,
    resolved_default_backend: Option<String>,
    available_backends: Vec<String>,
    backends: Vec<WebSearchBackendCatalogRecord>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WebSearchToolOutput {
    query: String,
    request_query: String,
    locale: String,
    freshness: WebSearchFreshness,
    source_mode: WebSearchSourceMode,
    requested_backend: String,
    backend: String,
    available_backends: Vec<String>,
    retrieval_mode: String,
    backend_capabilities: WebSearchBackendCapabilities,
    engine: String,
    request_url: String,
    final_url: String,
    request_urls: Vec<String>,
    final_urls: Vec<String>,
    response_pages: usize,
    backend_offset_base: usize,
    status: u16,
    content_type: Option<String>,
    limit: usize,
    offset: usize,
    next_offset: Option<usize>,
    more_results_available: Option<bool>,
    domains: Vec<String>,
    result_count: usize,
    total_matches: usize,
    result_domains: Vec<String>,
    citation_ids: Vec<String>,
    retrieved_at_unix_s: u64,
    policy: WebSearchPolicyOutput,
    freshness_filter: WebSearchFreshnessOutput,
    results: Vec<SearchResultRecord>,
    sources: Vec<SearchSourceRecord>,
}

#[derive(Clone, Debug)]
struct SearchLocale {
    language: String,
    country: String,
}

#[derive(Clone, Debug)]
struct WebSearchRequest {
    query: String,
    locale: SearchLocale,
    freshness: WebSearchFreshness,
    source_mode: WebSearchSourceMode,
    limit: usize,
    offset: usize,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema, PartialEq, Eq)]
struct WebSearchBackendCapabilities {
    locale: bool,
    freshness: bool,
    source_mode: bool,
    pagination: bool,
    extra_snippets: bool,
}

#[derive(Clone, Debug)]
struct SearchBackendResponse {
    request_urls: Vec<Url>,
    final_urls: Vec<Url>,
    offset_base: usize,
    status: u16,
    content_type: Option<String>,
    body: String,
    results: Vec<SearchResultItem>,
    more_results_available: Option<bool>,
}

#[async_trait]
trait WebSearchBackend: Send + Sync {
    fn backend_name(&self) -> &'static str;
    fn retrieval_mode(&self) -> &'static str;
    // The request contract is intentionally richer than the bundled RSS fallback.
    // Callers can ask for freshness/source modes today, while result metadata
    // exposes which knobs the active backend can actually honor.
    fn capabilities(&self) -> WebSearchBackendCapabilities;
    async fn search(
        &self,
        client: &Client,
        policy: &WebToolPolicy,
        request: &WebSearchRequest,
    ) -> Result<SearchBackendResponse>;
}

#[derive(Clone)]
pub struct WebSearchTool {
    client: Client,
    policy: WebToolPolicy,
    backend_registry: WebSearchBackendRegistry,
}

#[derive(Clone)]
pub struct WebSearchBackendsTool {
    backend_registry: WebSearchBackendRegistry,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for WebSearchBackendsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_env_settings(WebToolPolicy::default(), DEFAULT_HTTP_TIMEOUT_MS)
            .expect("web search client")
    }

    pub(crate) fn with_env_settings(policy: WebToolPolicy, timeout_ms: u64) -> Result<Self> {
        Self::with_backend_registry(policy, timeout_ms, WebSearchBackendRegistry::from_env()?)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_settings(
        policy: WebToolPolicy,
        timeout_ms: u64,
        endpoint: Option<String>,
    ) -> Result<Self> {
        let endpoint = endpoint.unwrap_or_else(|| DEFAULT_SEARCH_ENDPOINT.to_string());
        Self::with_backend_registry(
            policy,
            timeout_ms,
            WebSearchBackendRegistry::single(
                WebSearchBackendKind::BingRss,
                Arc::new(BingRssSearchBackend::new(Url::parse(&endpoint).map_err(
                    |error| crate::ToolError::invalid(format!("invalid search endpoint: {error}")),
                )?)),
            ),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_brave_backend(
        policy: WebToolPolicy,
        timeout_ms: u64,
        endpoint: Option<String>,
        api_key: String,
    ) -> Result<Self> {
        let endpoint = endpoint.unwrap_or_else(|| DEFAULT_BRAVE_API_BASE_URL.to_string());
        Self::with_backend_registry(
            policy,
            timeout_ms,
            WebSearchBackendRegistry::single(
                WebSearchBackendKind::BraveApi,
                Arc::new(BraveApiSearchBackend::new(
                    Url::parse(&endpoint).map_err(|error| {
                        crate::ToolError::invalid(format!("invalid Brave API endpoint: {error}"))
                    })?,
                    api_key,
                )),
            ),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_exa_backend(
        policy: WebToolPolicy,
        timeout_ms: u64,
        endpoint: Option<String>,
        api_key: String,
    ) -> Result<Self> {
        let endpoint = endpoint.unwrap_or_else(|| DEFAULT_EXA_API_BASE_URL.to_string());
        Self::with_backend_registry(
            policy,
            timeout_ms,
            WebSearchBackendRegistry::single(
                WebSearchBackendKind::ExaApi,
                Arc::new(ExaApiSearchBackend::new(
                    Url::parse(&endpoint).map_err(|error| {
                        crate::ToolError::invalid(format!("invalid Exa API endpoint: {error}"))
                    })?,
                    api_key,
                )),
            ),
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_duckduckgo_backend(
        policy: WebToolPolicy,
        timeout_ms: u64,
        endpoint: Option<String>,
    ) -> Result<Self> {
        let endpoint = endpoint.unwrap_or_else(|| DEFAULT_DUCKDUCKGO_HTML_ENDPOINT.to_string());
        Self::with_backend_registry(
            policy,
            timeout_ms,
            WebSearchBackendRegistry::single(
                WebSearchBackendKind::DuckDuckGoHtml,
                Arc::new(DuckDuckGoHtmlSearchBackend::new(
                    Url::parse(&endpoint).map_err(|error| {
                        crate::ToolError::invalid(format!(
                            "invalid DuckDuckGo HTML endpoint: {error}"
                        ))
                    })?,
                )),
            ),
        )
    }

    fn with_backend_registry(
        policy: WebToolPolicy,
        timeout_ms: u64,
        backend_registry: WebSearchBackendRegistry,
    ) -> Result<Self> {
        Ok(Self {
            // Search result allowlists apply to returned links, not to the configured
            // search backend. Redirects still need transport checks so the engine
            // cannot bounce the request into private network space.
            client: default_http_client(
                timeout_ms,
                policy.clone(),
                RedirectValidationScope::Transport,
            )?,
            policy,
            backend_registry,
        })
    }
}

impl WebSearchBackendsTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_env_settings().expect("web search backend catalog")
    }

    pub(crate) fn with_env_settings() -> Result<Self> {
        Ok(Self {
            backend_registry: WebSearchBackendRegistry::from_env()?,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn with_backend_registry(backend_registry: WebSearchBackendRegistry) -> Self {
        Self { backend_registry }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".into(),
            description: "Search the public web and return result titles, URLs, and snippets. Supports optional backend selection plus per-call domain filtering before follow-up web_fetch calls.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WebSearchToolInput))
                .expect("web_search schema"),
            output_mode: ToolOutputMode::Text,
            output_schema: Some(
                serde_json::to_value(schema_for!(WebSearchToolOutput))
                    .expect("web_search output schema"),
            ),
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
        let available_backends = self.backend_registry.available_backend_names();
        let (requested_backend, backend) =
            match self.backend_registry.resolve(input.backend.as_deref()) {
                Ok((requested_backend, backend)) => (requested_backend, backend),
                Err(error) => {
                    return Ok(ToolResult::error(
                        call_id,
                        "web_search",
                        format!("Failed to resolve a web search backend for `{query}`: {error}"),
                    ));
                }
            };

        let domains = normalize_domains(input.domains);
        let request = WebSearchRequest {
            query: augment_query_with_domains(query, &domains),
            locale: normalize_locale(input.locale),
            freshness: normalize_freshness(input.freshness),
            source_mode: normalize_source_mode(input.source_mode),
            limit: clamped_search_limit(input.limit),
            offset: input.offset.unwrap_or(0),
        };

        let response = match backend.search(&self.client, &self.policy, &request).await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ToolResult::error(
                    call_id,
                    "web_search",
                    format!("Failed to search the web for `{query}`: {error}"),
                ));
            }
        };
        let SearchBackendResponse {
            request_urls,
            final_urls,
            offset_base,
            status,
            content_type,
            body,
            results,
            more_results_available,
        } = response;
        let backend_capabilities = backend.capabilities();
        let request_url = request_urls
            .first()
            .cloned()
            .unwrap_or_else(|| Url::parse("https://example.invalid/search").expect("static url"));
        let final_url = final_urls
            .last()
            .cloned()
            .unwrap_or_else(|| request_url.clone());
        let limit = request.limit;
        let offset = request.offset;

        if !(200..300).contains(&status) {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.clone(),
                tool_name: "web_search".into(),
                parts: vec![MessagePart::text(format!(
                    "query> {query}\nstatus> {status}\n\n{}",
                    summarize_remote_body(&body, content_type.as_deref())
                ))],
                structured_content: None,
                metadata: Some(serde_json::json!({
                    "query": query,
                    "request_query": request.query,
                    "locale": request.locale.language,
                    "freshness": request.freshness,
                    "source_mode": request.source_mode,
                    "requested_backend": requested_backend.name(),
                    "backend": backend.backend_name(),
                    "available_backends": available_backends,
                    "retrieval_mode": backend.retrieval_mode(),
                    "backend_capabilities": backend_capabilities,
                    "status": status,
                    "content_type": content_type,
                    "request_url": request_url.as_str(),
                    "final_url": final_url.as_str(),
                    "request_urls": request_urls.iter().map(Url::as_str).collect::<Vec<_>>(),
                    "final_urls": final_urls.iter().map(Url::as_str).collect::<Vec<_>>(),
                    "response_pages": request_urls.len(),
                    "backend_offset_base": offset_base,
                })),
                is_error: true,
            });
        }

        let filtered_results = results
            .into_iter()
            .filter(|item| matches_policy(item, &self.policy))
            .filter(|item| matches_domains(item, &domains))
            .collect::<Vec<_>>();
        let (filtered_results, freshness_filter) =
            apply_freshness_filter(filtered_results, &request.freshness);
        let filtered_total = filtered_results.len();
        let total_matches = offset_base + filtered_total;
        let relative_offset = offset.saturating_sub(offset_base).min(filtered_total);
        let paged_results = filtered_results
            .iter()
            .skip(relative_offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let result_records = paged_results
            .iter()
            .enumerate()
            .map(|(index, item)| SearchResultRecord {
                id: stable_result_id(item),
                citation_id: stable_result_citation_id(&item.url),
                rank: offset + index + 1,
                domain: result_domain(&item.url),
                title: item.title.clone(),
                url: item.url.clone(),
                raw_url: item.raw_url.clone(),
                snippet: item.snippet.clone(),
                extra_snippets: item.extra_snippets.clone(),
                published_at: item.published_at.clone(),
                source_name: item.source_name.clone(),
            })
            .collect::<Vec<_>>();
        let unique_domains = unique_domains(&result_records);
        let sources = build_search_sources(&result_records);
        let citation_ids = sources
            .iter()
            .map(|source| source.citation_id.clone())
            .collect::<Vec<_>>();
        let consumed_results = offset + result_records.len();
        let next_offset = (consumed_results < total_matches
            || more_results_available.unwrap_or(false))
        .then_some(consumed_results);
        let retrieved_at_unix_s = unix_timestamp_s();
        let policy_output = WebSearchPolicyOutput {
            allow_private_hosts: self.policy.allow_private_hosts,
            allowed_domains: self.policy.allowed_domains.iter().cloned().collect(),
            blocked_domains: self.policy.blocked_domains.iter().cloned().collect(),
        };
        let structured_output = WebSearchToolOutput {
            query: query.to_string(),
            request_query: request.query.clone(),
            locale: request.locale.language.clone(),
            freshness: request.freshness.clone(),
            source_mode: request.source_mode.clone(),
            requested_backend: requested_backend.name().to_string(),
            backend: backend.backend_name().to_string(),
            available_backends: available_backends.clone(),
            retrieval_mode: backend.retrieval_mode().to_string(),
            backend_capabilities: backend_capabilities.clone(),
            engine: request_url.host_str().unwrap_or("custom").to_string(),
            request_url: request_url.as_str().to_string(),
            final_url: final_url.as_str().to_string(),
            request_urls: request_urls
                .iter()
                .map(|url| url.as_str().to_string())
                .collect(),
            final_urls: final_urls
                .iter()
                .map(|url| url.as_str().to_string())
                .collect(),
            response_pages: request_urls.len(),
            backend_offset_base: offset_base,
            status,
            content_type: content_type.clone(),
            limit,
            offset,
            next_offset,
            more_results_available,
            domains: domains.clone(),
            result_count: result_records.len(),
            total_matches,
            result_domains: unique_domains.clone(),
            citation_ids: citation_ids.clone(),
            retrieved_at_unix_s,
            policy: policy_output,
            freshness_filter: freshness_filter.clone(),
            results: result_records.clone(),
            sources: sources.clone(),
        };
        let structured_output_value =
            serde_json::to_value(&structured_output).expect("web_search structured output");

        let mut sections = vec![
            format!("query> {query}"),
            format!("requested_backend> {}", requested_backend.name()),
            format!("backend> {}", backend.backend_name()),
            format!("retrieval_mode> {}", backend.retrieval_mode()),
            format!("locale> {}", request.locale.language),
            format!("freshness> {}", format_freshness(&request.freshness)),
            format!("source_mode> {}", format_source_mode(&request.source_mode)),
            format!("engine> {}", request_url.host_str().unwrap_or("custom")),
            format!("limit> {limit}"),
            format!("offset> {offset}"),
        ];
        sections.push(format!(
            "available_backends> {}",
            available_backends.join(", ")
        ));
        if !domains.is_empty() {
            sections.push(format!("domains> {}", domains.join(", ")));
        }
        sections.push(format!("results> {}", result_records.len()));
        sections.push(format!("total_matches> {total_matches}"));
        sections.push(format!("citations> {}", citation_ids.len()));
        sections.push(format!(
            "freshness_mode> {}",
            format_freshness_mode(&freshness_filter.mode)
        ));
        if let Some(cutoff_unix_s) = freshness_filter.cutoff_unix_s {
            sections.push(format!("freshness_cutoff_unix_s> {cutoff_unix_s}"));
        }
        sections.push(format!(
            "freshness_dropped> {}",
            freshness_filter.dropped_results
        ));
        sections.push(format!(
            "freshness_unknown> {}",
            freshness_filter.kept_without_timestamp
        ));
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
            tool_name: "web_search".into(),
            parts: vec![MessagePart::text(sections.join("\n"))],
            structured_content: Some(structured_output_value.clone()),
            metadata: Some(structured_output_value),
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for WebSearchBackendsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search_backends".into(),
            description: "Inspect known web_search backends, configured availability, capability coverage, and default selection order.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WebSearchBackendsToolInput))
                .expect("web_search_backends schema"),
            output_mode: ToolOutputMode::Text,
            output_schema: Some(
                serde_json::to_value(schema_for!(WebSearchBackendsToolOutput))
                    .expect("web_search_backends output schema"),
            ),
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("List Search Backends", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: WebSearchBackendsToolInput = serde_json::from_value(arguments)?;
        let default_selector = self.backend_registry.default_selector();
        let resolved_default_backend = self
            .backend_registry
            .resolved_default_kind()
            .map(|kind| kind.backend_name().to_string());
        let available_backends = self.backend_registry.available_backend_names();
        let backends = self
            .backend_registry
            .known_backend_kinds(input.include_unconfigured)
            .into_iter()
            .map(|kind| {
                // Catalog data must exist even for backends that are not configured
                // in the current runtime, so the registry combines static backend
                // declarations with live instances when available.
                let live_backend = self.backend_registry.get(kind);
                let retrieval_mode = live_backend
                    .as_ref()
                    .map(|backend| backend.retrieval_mode())
                    .unwrap_or_else(|| kind.retrieval_mode())
                    .to_string();
                let capabilities = live_backend
                    .as_ref()
                    .map(|backend| backend.capabilities())
                    .unwrap_or_else(|| kind.capabilities());

                WebSearchBackendCatalogRecord {
                    name: kind.backend_name().to_string(),
                    selector_aliases: kind
                        .selector_aliases()
                        .iter()
                        .map(|alias| (*alias).to_string())
                        .collect(),
                    backend_type: kind.backend_type(),
                    configured: self.backend_registry.contains(kind),
                    selected_by_default: resolved_default_backend
                        .as_deref()
                        .is_some_and(|name| name == kind.backend_name()),
                    auto_priority: WebSearchBackendRegistry::auto_priority_rank(kind),
                    retrieval_mode,
                    capabilities,
                    missing_requirement: (!self.backend_registry.contains(kind))
                        .then(|| kind.missing_requirement().map(str::to_string))
                        .flatten(),
                }
            })
            .collect::<Vec<_>>();
        let structured_output = WebSearchBackendsToolOutput {
            default_selector: default_selector.name().to_string(),
            resolved_default_backend: resolved_default_backend.clone(),
            available_backends: available_backends.clone(),
            backends: backends.clone(),
        };
        let structured_output_value = serde_json::to_value(&structured_output)
            .expect("web_search_backends structured output");

        let mut sections = vec![
            format!("default_selector> {}", default_selector.name()),
            format!(
                "resolved_default_backend> {}",
                resolved_default_backend.as_deref().unwrap_or("none")
            ),
            format!("available_backends> {}", available_backends.join(", ")),
            String::new(),
        ];
        for backend in &backends {
            sections.push(format!("backend> {}", backend.name));
            sections.push(format!("configured> {}", backend.configured));
            sections.push(format!(
                "selected_by_default> {}",
                backend.selected_by_default
            ));
            sections.push(format!("auto_priority> {}", backend.auto_priority));
            sections.push(format!(
                "selector_aliases> {}",
                backend.selector_aliases.join(", ")
            ));
            sections.push(format!(
                "type> {}",
                format_backend_type(&backend.backend_type)
            ));
            sections.push(format!("retrieval_mode> {}", backend.retrieval_mode));
            sections.push(format!(
                "capabilities> {}",
                format_backend_capabilities(&backend.capabilities)
            ));
            if let Some(missing_requirement) = &backend.missing_requirement {
                sections.push(format!("missing_requirement> {missing_requirement}"));
            }
            sections.push(String::new());
        }

        Ok(ToolResult {
            id: call_id.clone(),
            call_id: types::CallId::from(&call_id),
            tool_name: "web_search_backends".into(),
            parts: vec![MessagePart::text(
                sections.join("\n").trim_end().to_string(),
            )],
            structured_content: Some(structured_output_value.clone()),
            metadata: Some(structured_output_value),
            is_error: false,
        })
    }
}

fn looks_like_markup_fragment(value: &str) -> bool {
    value.contains('<') && value.contains('>')
}

const fn default_true() -> bool {
    true
}

async fn send_search_request(
    client: &Client,
    policy: &WebToolPolicy,
    request_url: Url,
    api_key: Option<&str>,
) -> Result<(Url, u16, Option<String>, String)> {
    policy.validate_transport_url(&request_url)?;
    let mut request = client.get(request_url.clone());
    if let Some(api_key) = api_key {
        request = request.header("X-Subscription-Token", api_key);
    }
    let response = request.send().await?;
    let final_url = response.url().clone();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.text().await?;
    Ok((final_url, status, content_type, body))
}

async fn send_search_json_request(
    client: &Client,
    policy: &WebToolPolicy,
    request_url: Url,
    api_key_header: (&str, &str),
    body_json: Value,
) -> Result<(Url, u16, Option<String>, String)> {
    policy.validate_transport_url(&request_url)?;
    let response = client
        .post(request_url.clone())
        .header(api_key_header.0, api_key_header.1)
        .json(&body_json)
        .send()
        .await?;
    let final_url = response.url().clone();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.text().await?;
    Ok((final_url, status, content_type, body))
}

fn canonicalize_result_url(url: &str) -> (String, Option<String>) {
    let Ok(parsed) = Url::parse(url) else {
        return (url.to_string(), None);
    };
    let Some(host) = parsed.host_str() else {
        return (url.to_string(), None);
    };
    let host = host.to_ascii_lowercase();
    let path = parsed.path().to_ascii_lowercase();
    let target = if host.ends_with("bing.com") && path.contains("apiclick") {
        parsed
            .query_pairs()
            .find_map(|(key, value)| (key == "url").then_some(value.into_owned()))
    } else if host.ends_with("duckduckgo.com") && path == "/l/" {
        parsed
            .query_pairs()
            .find_map(|(key, value)| (key == "uddg").then_some(value.into_owned()))
    } else {
        None
    }
    .filter(|value| Url::parse(value).is_ok());

    match target {
        Some(target) if target != url => (target, Some(url.to_string())),
        _ => (url.to_string(), None),
    }
}

fn normalize_locale(locale: Option<String>) -> SearchLocale {
    let raw = locale.unwrap_or_else(|| "en-US".to_string());
    let normalized = raw.trim().replace('_', "-");
    let mut parts = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return SearchLocale {
            language: "en-US".to_string(),
            country: "us".to_string(),
        };
    }

    let language = parts.remove(0).to_ascii_lowercase();
    let country = parts
        .first()
        .map(|part| part.to_ascii_uppercase())
        .unwrap_or_else(|| "US".to_string());

    SearchLocale {
        language: format!("{language}-{country}"),
        country: country.to_ascii_lowercase(),
    }
}

fn normalize_freshness(freshness: Option<WebSearchFreshness>) -> WebSearchFreshness {
    freshness.unwrap_or(WebSearchFreshness::AnyTime)
}

fn normalize_source_mode(source_mode: Option<WebSearchSourceMode>) -> WebSearchSourceMode {
    source_mode.unwrap_or(WebSearchSourceMode::General)
}

fn format_freshness(freshness: &WebSearchFreshness) -> &'static str {
    match freshness {
        WebSearchFreshness::AnyTime => "any_time",
        WebSearchFreshness::PastDay => "past_day",
        WebSearchFreshness::PastWeek => "past_week",
        WebSearchFreshness::PastMonth => "past_month",
        WebSearchFreshness::PastYear => "past_year",
    }
}

fn format_source_mode(source_mode: &WebSearchSourceMode) -> &'static str {
    match source_mode {
        WebSearchSourceMode::General => "general",
        WebSearchSourceMode::News => "news",
    }
}

fn format_freshness_mode(mode: &WebSearchFreshnessMode) -> &'static str {
    match mode {
        WebSearchFreshnessMode::NotRequested => "not_requested",
        WebSearchFreshnessMode::BestEffort => "best_effort",
    }
}

fn format_backend_type(backend_type: &WebSearchBackendType) -> &'static str {
    match backend_type {
        WebSearchBackendType::HostedApi => "hosted_api",
        WebSearchBackendType::RssFeed => "rss_feed",
        WebSearchBackendType::HtmlScrape => "html_scrape",
    }
}

fn format_backend_capabilities(capabilities: &WebSearchBackendCapabilities) -> String {
    let mut supported = Vec::new();
    if capabilities.locale {
        supported.push("locale");
    }
    if capabilities.freshness {
        supported.push("freshness");
    }
    if capabilities.source_mode {
        supported.push("source_mode");
    }
    if capabilities.pagination {
        supported.push("pagination");
    }
    if capabilities.extra_snippets {
        supported.push("extra_snippets");
    }
    if supported.is_empty() {
        "none".to_string()
    } else {
        supported.join(", ")
    }
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

fn apply_freshness_filter(
    results: Vec<SearchResultItem>,
    requested: &WebSearchFreshness,
) -> (Vec<SearchResultItem>, WebSearchFreshnessOutput) {
    if matches!(requested, WebSearchFreshness::AnyTime) {
        return (
            results,
            WebSearchFreshnessOutput {
                requested: requested.clone(),
                mode: WebSearchFreshnessMode::NotRequested,
                cutoff_unix_s: None,
                dropped_results: 0,
                kept_without_timestamp: 0,
            },
        );
    }

    let now = OffsetDateTime::now_utc();
    let Some(cutoff) = freshness_cutoff(now, requested) else {
        return (
            results,
            WebSearchFreshnessOutput {
                requested: requested.clone(),
                mode: WebSearchFreshnessMode::BestEffort,
                cutoff_unix_s: None,
                dropped_results: 0,
                kept_without_timestamp: 0,
            },
        );
    };

    let mut dropped_results = 0usize;
    let mut kept_without_timestamp = 0usize;
    let filtered = results
        .into_iter()
        .filter(
            |item| match item.published_at.as_deref().and_then(parse_published_at) {
                Some(timestamp) => {
                    let keep = timestamp >= cutoff;
                    if !keep {
                        dropped_results += 1;
                    }
                    keep
                }
                None => {
                    kept_without_timestamp += 1;
                    true
                }
            },
        )
        .collect::<Vec<_>>();

    (
        filtered,
        WebSearchFreshnessOutput {
            requested: requested.clone(),
            mode: WebSearchFreshnessMode::BestEffort,
            cutoff_unix_s: Some(cutoff.unix_timestamp()),
            dropped_results,
            kept_without_timestamp,
        },
    )
}

fn freshness_cutoff(now: OffsetDateTime, requested: &WebSearchFreshness) -> Option<OffsetDateTime> {
    match requested {
        WebSearchFreshness::AnyTime => None,
        WebSearchFreshness::PastDay => Some(now - time::Duration::days(1)),
        WebSearchFreshness::PastWeek => Some(now - time::Duration::weeks(1)),
        WebSearchFreshness::PastMonth => Some(now - time::Duration::days(30)),
        WebSearchFreshness::PastYear => Some(now - time::Duration::days(365)),
    }
}

fn parse_published_at(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc2822)
        .ok()
        .or_else(|| OffsetDateTime::parse(value, &Rfc3339).ok())
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

fn build_search_sources(results: &[SearchResultRecord]) -> Vec<SearchSourceRecord> {
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();

    for item in results {
        // Citations identify underlying sources, not list positions. Multiple
        // ranked results can converge on one URL, so source annotations dedupe
        // by stable citation id while result rows keep their own rank/id pair.
        if !seen.insert(item.citation_id.clone()) {
            continue;
        }
        sources.push(SearchSourceRecord {
            citation_id: item.citation_id.clone(),
            result_id: item.id.clone(),
            rank: item.rank,
            domain: item.domain.clone(),
            title: item.title.clone(),
            url: item.url.clone(),
            raw_url: item.raw_url.clone(),
            snippet: item.snippet.clone(),
            extra_snippets: item.extra_snippets.clone(),
            published_at: item.published_at.clone(),
            source_name: item.source_name.clone(),
        });
    }

    sources
}

fn format_result_entry(item: &SearchResultRecord) -> String {
    let mut entry = vec![
        format!("{}. {}", item.rank, item.title),
        format!("id: {}", item.id),
        format!("citation: {}", item.citation_id),
        format!("url: {}", item.url),
    ];
    if let Some(raw_url) = &item.raw_url {
        entry.push(format!("raw_url: {raw_url}"));
    }
    if let Some(domain) = &item.domain {
        entry.push(format!("domain: {domain}"));
    }
    if let Some(source_name) = &item.source_name {
        entry.push(format!("source: {source_name}"));
    }
    if let Some(snippet) = &item.snippet {
        let (snippet, truncated) = truncate_text(snippet, DEFAULT_RESULT_SNIPPET_MAX_CHARS);
        entry.push(if truncated {
            format!("snippet: {snippet}...")
        } else {
            format!("snippet: {snippet}")
        });
    }
    if !item.extra_snippets.is_empty() {
        entry.push(format!(
            "extra_snippets: {}",
            item.extra_snippets.join(" | ")
        ));
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

fn stable_result_citation_id(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let digest = hasher.finalize();
    let mut output = String::from("wsrc_");
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
    use super::engines::{
        WebSearchBackendKind, WebSearchBackendRegistry, WebSearchBackendSelector,
        parse_backend_selector,
    };
    use super::{
        BingRssSearchBackend, ExaApiSearchBackend, SearchLocale, WebSearchBackendsTool,
        WebSearchFreshness, WebSearchRequest, WebSearchSourceMode, WebSearchTool,
        WebSearchToolInput, parse_feed_results,
    };
    use crate::web::common::WebToolPolicy;
    use crate::{Tool, ToolExecutionContext};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use types::ToolCallId;
    use wiremock::matchers::{body_partial_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parse_feed_results_extracts_rss_items() {
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
        let results = parse_feed_results(xml);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example One");
        assert_eq!(results[0].snippet.as_deref(), Some("alpha & beta"));
    }

    #[test]
    fn parse_feed_results_extracts_atom_entries() {
        let xml = r#"
            <feed xmlns="http://www.w3.org/2005/Atom">
              <entry>
                <title>Atom Example</title>
                <link rel="alternate" href="https://example.com/atom"/>
                <summary><![CDATA[See <b>details</b>.]]></summary>
                <updated>2026-03-25T09:00:00Z</updated>
              </entry>
            </feed>
        "#;

        let results = parse_feed_results(xml);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Atom Example");
        assert_eq!(results[0].url, "https://example.com/atom");
        assert_eq!(results[0].snippet.as_deref(), Some("See details."));
        assert_eq!(
            results[0].published_at.as_deref(),
            Some("2026-03-25T09:00:00Z")
        );
    }

    #[test]
    fn parse_feed_results_extracts_news_source_name() {
        let xml = r#"
            <rss xmlns:News="https://www.bing.com/news/search?q=openai&amp;format=rss">
              <channel>
                <item>
                  <title>OpenAI</title>
                  <link>https://example.com/openai</link>
                  <News:Source>Example News</News:Source>
                </item>
              </channel>
            </rss>
        "#;

        let results = parse_feed_results(xml);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_name.as_deref(), Some("Example News"));
    }

    #[test]
    fn parse_feed_results_canonicalizes_bing_apiclick_urls() {
        let xml = r#"
            <rss><channel>
                <item>
                    <title>Wrapped</title>
                    <link>https://www.bing.com/news/apiclick.aspx?ref=FexRss&amp;url=https%3A%2F%2Fexample.com%2Farticle&amp;c=123</link>
                </item>
            </channel></rss>
        "#;

        let results = parse_feed_results(xml);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/article");
        assert_eq!(
            results[0].raw_url.as_deref(),
            Some(
                "https://www.bing.com/news/apiclick.aspx?ref=FexRss&url=https%3A%2F%2Fexample.com%2Farticle&c=123"
            )
        );
    }

    #[test]
    fn bing_backend_uses_news_feed_path_for_news_mode() {
        let backend =
            BingRssSearchBackend::new(reqwest::Url::parse("https://www.bing.com/search").unwrap());
        let request = WebSearchRequest {
            query: "openai".to_string(),
            locale: SearchLocale {
                language: "en-US".to_string(),
                country: "us".to_string(),
            },
            freshness: WebSearchFreshness::AnyTime,
            source_mode: WebSearchSourceMode::News,
            limit: 5,
            offset: 0,
        };

        let request_url = backend.build_request_url(&request).unwrap();
        assert_eq!(request_url.path(), "/news/search");
    }

    #[test]
    fn backend_selector_parser_accepts_supported_aliases() {
        assert_eq!(parse_backend_selector(None).unwrap(), None);
        assert_eq!(
            parse_backend_selector(Some("auto")).unwrap(),
            Some(WebSearchBackendSelector::Auto)
        );
        assert_eq!(
            parse_backend_selector(Some("bing")).unwrap(),
            Some(WebSearchBackendSelector::Kind(
                WebSearchBackendKind::BingRss
            ))
        );
        assert_eq!(
            parse_backend_selector(Some("brave")).unwrap(),
            Some(WebSearchBackendSelector::Kind(
                WebSearchBackendKind::BraveApi
            ))
        );
        assert_eq!(
            parse_backend_selector(Some("exa")).unwrap(),
            Some(WebSearchBackendSelector::Kind(WebSearchBackendKind::ExaApi))
        );
        assert_eq!(
            parse_backend_selector(Some("ddg")).unwrap(),
            Some(WebSearchBackendSelector::Kind(
                WebSearchBackendKind::DuckDuckGoHtml
            ))
        );
        assert!(parse_backend_selector(Some("google")).is_err());
    }

    #[test]
    fn backend_registry_auto_prefers_hosted_backends() {
        let registry = WebSearchBackendRegistry::from_backends(
            WebSearchBackendSelector::Auto,
            vec![
                (
                    WebSearchBackendKind::BingRss,
                    Arc::new(BingRssSearchBackend::new(
                        reqwest::Url::parse("https://www.bing.com/search").unwrap(),
                    )) as Arc<dyn super::WebSearchBackend>,
                ),
                (
                    WebSearchBackendKind::BraveApi,
                    Arc::new(super::BraveApiSearchBackend::new(
                        reqwest::Url::parse("https://api.search.brave.com").unwrap(),
                        "token".to_string(),
                    )) as Arc<dyn super::WebSearchBackend>,
                ),
                (
                    WebSearchBackendKind::ExaApi,
                    Arc::new(ExaApiSearchBackend::new(
                        reqwest::Url::parse("https://api.exa.ai").unwrap(),
                        "token".to_string(),
                    )) as Arc<dyn super::WebSearchBackend>,
                ),
            ],
        );

        let (selector, backend) = registry.resolve(None).unwrap();
        assert_eq!(selector, WebSearchBackendSelector::Auto);
        assert_eq!(backend.backend_name(), "exa_api");
    }

    #[tokio::test]
    async fn web_search_backends_reports_catalog_and_default_resolution() {
        let registry = WebSearchBackendRegistry::from_backends(
            WebSearchBackendSelector::Auto,
            vec![
                (
                    WebSearchBackendKind::BingRss,
                    Arc::new(BingRssSearchBackend::new(
                        reqwest::Url::parse("https://www.bing.com/search").unwrap(),
                    )) as Arc<dyn super::WebSearchBackend>,
                ),
                (
                    WebSearchBackendKind::ExaApi,
                    Arc::new(ExaApiSearchBackend::new(
                        reqwest::Url::parse("https://api.exa.ai").unwrap(),
                        "token".to_string(),
                    )) as Arc<dyn super::WebSearchBackend>,
                ),
            ],
        );
        let tool = WebSearchBackendsTool::with_backend_registry(registry);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::json!({}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let text = result.text_content();
        assert!(text.contains("default_selector> auto"));
        assert!(text.contains("resolved_default_backend> exa_api"));
        assert!(text.contains("backend> exa_api"));
        assert!(text.contains("backend> brave_api"));
        assert!(text.contains("missing_requirement> AGENT_CORE_WEB_SEARCH_BRAVE_API_KEY"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["default_selector"], "auto");
        assert_eq!(structured["resolved_default_backend"], "exa_api");
        assert_eq!(structured["available_backends"][0], "exa_api");
        assert_eq!(structured["available_backends"][1], "bing_rss");
        assert_eq!(structured["backends"][0]["name"], "exa_api");
        assert_eq!(structured["backends"][0]["configured"], true);
        assert_eq!(structured["backends"][0]["selected_by_default"], true);
        assert_eq!(structured["backends"][1]["name"], "brave_api");
        assert_eq!(structured["backends"][1]["configured"], false);
        assert_eq!(
            structured["backends"][1]["missing_requirement"],
            "AGENT_CORE_WEB_SEARCH_BRAVE_API_KEY"
        );
    }

    fn brave_results(start: usize, end: usize) -> Vec<serde_json::Value> {
        (start..=end)
            .map(|index| {
                serde_json::json!({
                    "title": format!("Result {index}"),
                    "url": format!("https://example.com/{index}"),
                    "description": format!("summary {index}"),
                    "extra_snippets": [
                        format!("follow-up {index}"),
                        format!("detail {index}")
                    ],
                    "page_age": "2026-03-25T09:00:00Z",
                    "meta_url": {
                        "hostname": "example.com"
                    }
                })
            })
            .collect()
    }

    #[tokio::test]
    async fn web_search_brave_backend_translates_offsets_into_hosted_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/res/v1/web/search"))
            .and(query_param("offset", "1"))
            .and(query_param("count", "20"))
            .and(query_param("country", "US"))
            .and(query_param("search_lang", "en"))
            .and(query_param("freshness", "pw"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        serde_json::json!({
                            "query": { "more_results_available": true },
                            "web": { "results": brave_results(21, 40) }
                        })
                        .to_string(),
                    ),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/res/v1/web/search"))
            .and(query_param("offset", "2"))
            .and(query_param("count", "20"))
            .and(query_param("country", "US"))
            .and(query_param("search_lang", "en"))
            .and(query_param("freshness", "pw"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        serde_json::json!({
                            "query": { "more_results_available": false },
                            "web": { "results": brave_results(41, 60) }
                        })
                        .to_string(),
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebSearchTool::with_brave_backend(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            Some(server.uri()),
            "test-token".to_string(),
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "example".to_string(),
                    backend: None,
                    limit: Some(5),
                    offset: Some(38),
                    domains: None,
                    locale: None,
                    freshness: Some(WebSearchFreshness::PastWeek),
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("Result 39"));
        assert!(text.contains("Result 43"));
        assert!(text.contains("extra_snippets: follow-up 39 | detail 39"));

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["requested_backend"], "brave_api");
        assert_eq!(structured["backend"], "brave_api");
        assert_eq!(structured["retrieval_mode"], "json_api");
        assert_eq!(structured["backend_capabilities"]["pagination"], true);
        assert_eq!(structured["backend_capabilities"]["extra_snippets"], true);
        assert_eq!(structured["response_pages"], 2);
        assert_eq!(structured["backend_offset_base"], 20);
        assert_eq!(structured["results"][0]["rank"], 39);
        assert_eq!(structured["results"][0]["title"], "Result 39");
        assert_eq!(
            structured["results"][0]["extra_snippets"][0],
            "follow-up 39"
        );
        assert_eq!(structured["results"][4]["title"], "Result 43");
        assert_eq!(structured["more_results_available"], false);
        assert_eq!(structured["request_urls"].as_array().unwrap().len(), 2);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in &requests {
            assert_eq!(
                request
                    .headers
                    .get("x-subscription-token")
                    .and_then(|value| value.to_str().ok()),
                Some("test-token")
            );
        }
    }

    #[tokio::test]
    async fn web_search_exa_backend_overfetches_for_offset_windows() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("x-api-key", "exa-token"))
            .and(body_partial_json(serde_json::json!({
                "query": "openai",
                "numResults": 4,
                "category": "news",
                "summary": true,
                "highlights": true,
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        serde_json::json!({
                            "results": [
                                {
                                    "title": "One",
                                    "url": "https://example.com/1",
                                    "summary": "summary 1",
                                    "highlights": ["highlight 1a", "highlight 1b"],
                                    "publishedDate": "2026-03-25T09:00:00Z"
                                },
                                {
                                    "title": "Two",
                                    "url": "https://example.com/2",
                                    "summary": "summary 2",
                                    "highlights": ["highlight 2a"],
                                    "publishedDate": "2026-03-25T09:00:00Z"
                                },
                                {
                                    "title": "Three",
                                    "url": "https://example.com/3",
                                    "summary": "summary 3",
                                    "highlights": ["highlight 3a", "highlight 3b"],
                                    "publishedDate": "2026-03-25T09:00:00Z"
                                },
                                {
                                    "title": "Four",
                                    "url": "https://example.com/4",
                                    "summary": "summary 4",
                                    "highlights": ["highlight 4a"],
                                    "publishedDate": "2026-03-25T09:00:00Z"
                                }
                            ]
                        })
                        .to_string(),
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebSearchTool::with_exa_backend(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            Some(server.uri()),
            "exa-token".to_string(),
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "openai".to_string(),
                    backend: None,
                    limit: Some(2),
                    offset: Some(2),
                    domains: None,
                    locale: None,
                    freshness: Some(WebSearchFreshness::PastWeek),
                    source_mode: Some(WebSearchSourceMode::News),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("Three"));
        assert!(text.contains("Four"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["requested_backend"], "exa_api");
        assert_eq!(structured["backend"], "exa_api");
        assert_eq!(structured["results"][0]["rank"], 3);
        assert_eq!(structured["results"][0]["title"], "Three");
        assert_eq!(
            structured["results"][0]["extra_snippets"][0],
            "highlight 3b"
        );
        assert_eq!(structured["backend_capabilities"]["extra_snippets"], true);
    }

    #[tokio::test]
    async fn web_search_duckduckgo_backend_parses_html_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/html/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(
                        r#"
                        <html><body>
                          <div class="result results_links">
                            <a class="result__a" href="https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Falpha">Alpha</a>
                            <div class="result__snippet">alpha snippet</div>
                            <span class="result__url">example.com</span>
                          </div>
                          <div class="result results_links">
                            <a class="result__a" href="https://example.org/beta">Beta</a>
                            <div class="result__snippet">beta snippet</div>
                            <span class="result__url">example.org</span>
                          </div>
                        </body></html>
                        "#,
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebSearchTool::with_duckduckgo_backend(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            Some(format!("{}/html/", server.uri())),
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "openai".to_string(),
                    backend: None,
                    limit: Some(2),
                    offset: Some(0),
                    domains: None,
                    locale: None,
                    freshness: None,
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("https://example.com/alpha"));
        assert!(text.contains("raw_url: https://duckduckgo.com/l/?uddg="));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["requested_backend"], "duckduckgo_html");
        assert_eq!(structured["backend"], "duckduckgo_html");
        assert_eq!(structured["results"][0]["url"], "https://example.com/alpha");
    }

    #[tokio::test]
    async fn web_search_duckduckgo_backend_rejects_challenge_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/html/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(
                        r#"
                        <html><body>
                          <div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
                        </body></html>
                        "#,
                    ),
            )
            .mount(&server)
            .await;

        let tool = WebSearchTool::with_duckduckgo_backend(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            Some(format!("{}/html/", server.uri())),
        )
        .unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "openai".to_string(),
                    backend: None,
                    limit: Some(2),
                    offset: Some(0),
                    domains: None,
                    locale: None,
                    freshness: None,
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.text_content().contains("bot challenge"));
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
                    backend: None,
                    limit: Some(5),
                    offset: None,
                    domains: Some(vec!["allowed.example.com".to_string()]),
                    locale: None,
                    freshness: None,
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("allowed.example.com/article"));
        assert!(!text.contains("other.example.org/post"));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured["requested_backend"], "bing_rss");
        assert_eq!(structured["backend"], "bing_rss");
        assert_eq!(structured["available_backends"][0], "bing_rss");
        assert_eq!(structured["retrieval_mode"], "rss");
        assert_eq!(structured["locale"], "en-US");
        assert_eq!(structured["freshness"], "any_time");
        assert_eq!(structured["source_mode"], "general");
        assert_eq!(structured["backend_capabilities"]["locale"], true);
        assert_eq!(structured["backend_capabilities"]["freshness"], false);
        assert_eq!(structured["backend_capabilities"]["source_mode"], false);
        assert_eq!(structured["freshness_filter"]["mode"], "not_requested");
        assert_eq!(
            structured["citation_ids"][0],
            structured["results"][0]["citation_id"]
        );
        assert_eq!(
            structured["sources"][0]["citation_id"],
            structured["results"][0]["citation_id"]
        );
        assert_eq!(structured["domains"][0], "allowed.example.com");
        assert_eq!(
            structured["results"][0]["url"],
            "https://allowed.example.com/article"
        );
        assert!(text.contains("citation: wsrc_"));
        assert_eq!(
            result.metadata.unwrap()["domains"][0],
            "allowed.example.com"
        );
    }

    #[tokio::test]
    async fn web_search_uses_requested_locale_in_backend_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string("<rss><channel></channel></rss>"),
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
                    query: "bonjour".to_string(),
                    backend: None,
                    limit: Some(5),
                    offset: None,
                    domains: None,
                    locale: Some("fr-FR".to_string()),
                    freshness: Some(WebSearchFreshness::PastWeek),
                    source_mode: Some(WebSearchSourceMode::News),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["locale"], "fr-FR");
        assert_eq!(structured["freshness"], "past_week");
        assert_eq!(structured["source_mode"], "news");
        assert_eq!(structured["requested_backend"], "bing_rss");
        assert_eq!(structured["backend"], "bing_rss");
        let request_url = structured["request_url"].as_str().unwrap();
        assert!(request_url.contains("cc=fr"));
        assert!(request_url.contains("setlang=fr-FR"));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let query = requests[0].url.query().unwrap_or_default();
        assert!(query.contains("cc=fr"));
        assert!(query.contains("setlang=fr-FR"));
    }

    #[tokio::test]
    async fn web_search_filters_wrapped_results_by_canonical_domain() {
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
                            <title>Wrapped</title>
                            <link>https://www.bing.com/news/apiclick.aspx?ref=FexRss&amp;url=https%3A%2F%2Fallowed.example.com%2Farticle&amp;c=123</link>
                        </item>
                        <item>
                            <title>Other</title>
                            <link>https://www.bing.com/news/apiclick.aspx?ref=FexRss&amp;url=https%3A%2F%2Fother.example.org%2Fpost&amp;c=456</link>
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
                    backend: None,
                    limit: Some(5),
                    offset: None,
                    domains: Some(vec!["allowed.example.com".to_string()]),
                    locale: None,
                    freshness: None,
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("https://allowed.example.com/article"));
        assert!(text.contains("raw_url: https://www.bing.com/news/apiclick.aspx"));
        assert!(!text.contains("other.example.org/post"));
        let structured = result.structured_content.unwrap();
        assert_eq!(
            structured["results"][0]["url"],
            "https://allowed.example.com/article"
        );
        assert!(
            structured["results"][0]["raw_url"]
                .as_str()
                .unwrap()
                .contains("bing.com/news/apiclick.aspx")
        );
    }

    #[tokio::test]
    async fn web_search_applies_best_effort_freshness_filter() {
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
                            <title>Recent</title>
                            <link>https://example.com/recent</link>
                            <pubDate>Tue, 25 Mar 2026 09:00:00 GMT</pubDate>
                        </item>
                        <item>
                            <title>Old</title>
                            <link>https://example.com/old</link>
                            <pubDate>Tue, 25 Feb 2026 09:00:00 GMT</pubDate>
                        </item>
                        <item>
                            <title>Undated</title>
                            <link>https://example.com/undated</link>
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
                    backend: None,
                    limit: Some(5),
                    offset: None,
                    domains: None,
                    locale: None,
                    freshness: Some(WebSearchFreshness::PastWeek),
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("Recent"));
        assert!(text.contains("Undated"));
        assert!(!text.contains("Old"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["freshness_filter"]["mode"], "best_effort");
        assert_eq!(structured["freshness_filter"]["dropped_results"], 1);
        assert_eq!(structured["freshness_filter"]["kept_without_timestamp"], 1);
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
                    backend: None,
                    limit: Some(1),
                    offset: Some(1),
                    domains: None,
                    locale: None,
                    freshness: None,
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("Two"));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured["offset"], 1);
        assert_eq!(structured["next_offset"], 2);
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

    #[tokio::test]
    async fn web_search_allows_per_call_backend_override() {
        let bing_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(
                        r#"
                    <rss><channel>
                        <item><title>Bing Result</title><link>https://example.com/bing</link></item>
                    </channel></rss>
                "#,
                    ),
            )
            .mount(&bing_server)
            .await;

        let exa_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("x-api-key", "exa-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        serde_json::json!({
                            "results": [
                                {
                                    "title": "Exa Result",
                                    "url": "https://example.com/exa",
                                    "summary": "summary exa"
                                }
                            ]
                        })
                        .to_string(),
                    ),
            )
            .mount(&exa_server)
            .await;

        let tool = WebSearchTool::with_backend_registry(
            WebToolPolicy {
                allow_private_hosts: true,
                allowed_domains: BTreeSet::new(),
                blocked_domains: BTreeSet::new(),
            },
            5_000,
            WebSearchBackendRegistry::from_backends(
                WebSearchBackendSelector::Kind(WebSearchBackendKind::BingRss),
                vec![
                    (
                        WebSearchBackendKind::BingRss,
                        Arc::new(BingRssSearchBackend::new(
                            reqwest::Url::parse(&format!("{}/search", bing_server.uri())).unwrap(),
                        )) as Arc<dyn super::WebSearchBackend>,
                    ),
                    (
                        WebSearchBackendKind::ExaApi,
                        Arc::new(ExaApiSearchBackend::new(
                            reqwest::Url::parse(&exa_server.uri()).unwrap(),
                            "exa-token".to_string(),
                        )) as Arc<dyn super::WebSearchBackend>,
                    ),
                ],
            ),
        )
        .unwrap();

        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WebSearchToolInput {
                    query: "openai".to_string(),
                    backend: Some("exa".to_string()),
                    limit: Some(1),
                    offset: Some(0),
                    domains: None,
                    locale: None,
                    freshness: None,
                    source_mode: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("Exa Result"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["requested_backend"], "exa_api");
        assert_eq!(structured["backend"], "exa_api");
        assert_eq!(structured["available_backends"][0], "exa_api");
        assert_eq!(structured["available_backends"][1], "bing_rss");
        assert_eq!(bing_server.received_requests().await.unwrap().len(), 0);
        assert_eq!(exa_server.received_requests().await.unwrap().len(), 1);
    }
}
