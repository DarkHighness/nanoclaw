use super::{
    DEFAULT_BRAVE_API_BASE_URL, DEFAULT_DUCKDUCKGO_HTML_ENDPOINT, DEFAULT_EXA_API_BASE_URL,
    DEFAULT_FIRECRAWL_API_BASE_URL, DEFAULT_SEARCH_ENDPOINT, DEFAULT_TAVILY_API_BASE_URL,
    WebSearchBackend, WebSearchBackendCapabilities, WebSearchBackendType,
};
use crate::{Result, ToolError};
use agent_env::vars;
use reqwest::Url;
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) mod bing;
pub(super) mod brave;
pub(super) mod duckduckgo;
pub(super) mod exa;
pub(super) mod firecrawl;
pub(super) mod tavily;

use bing::BingRssSearchBackend;
use brave::BraveApiSearchBackend;
use duckduckgo::DuckDuckGoHtmlSearchBackend;
use exa::ExaApiSearchBackend;
use firecrawl::FirecrawlApiSearchBackend;
use tavily::TavilyApiSearchBackend;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WebSearchBackendKind {
    BingRss,
    BraveApi,
    ExaApi,
    TavilyApi,
    FirecrawlApi,
    DuckDuckGoHtml,
}

impl WebSearchBackendKind {
    pub(super) fn backend_name(self) -> &'static str {
        match self {
            Self::BingRss => "bing_rss",
            Self::BraveApi => "brave_api",
            Self::ExaApi => "exa_api",
            Self::TavilyApi => "tavily_api",
            Self::FirecrawlApi => "firecrawl_api",
            Self::DuckDuckGoHtml => "duckduckgo_html",
        }
    }

    pub(super) fn retrieval_mode(self) -> &'static str {
        match self {
            Self::BingRss => "rss",
            Self::BraveApi | Self::ExaApi | Self::TavilyApi | Self::FirecrawlApi => "json_api",
            Self::DuckDuckGoHtml => "html_scrape",
        }
    }

    pub(super) fn backend_type(self) -> WebSearchBackendType {
        match self {
            Self::BingRss => WebSearchBackendType::RssFeed,
            Self::BraveApi | Self::ExaApi | Self::TavilyApi | Self::FirecrawlApi => {
                WebSearchBackendType::HostedApi
            }
            Self::DuckDuckGoHtml => WebSearchBackendType::HtmlScrape,
        }
    }

    pub(super) fn capabilities(self) -> WebSearchBackendCapabilities {
        match self {
            Self::BingRss => WebSearchBackendCapabilities {
                locale: true,
                freshness: false,
                source_mode: true,
                pagination: false,
                extra_snippets: false,
            },
            Self::BraveApi => WebSearchBackendCapabilities {
                locale: true,
                freshness: true,
                source_mode: true,
                pagination: true,
                extra_snippets: true,
            },
            Self::ExaApi => WebSearchBackendCapabilities {
                locale: true,
                freshness: true,
                source_mode: true,
                pagination: false,
                extra_snippets: true,
            },
            Self::TavilyApi => WebSearchBackendCapabilities {
                locale: false,
                freshness: true,
                source_mode: true,
                pagination: false,
                extra_snippets: false,
            },
            Self::FirecrawlApi => WebSearchBackendCapabilities {
                locale: true,
                freshness: true,
                source_mode: true,
                pagination: false,
                extra_snippets: false,
            },
            Self::DuckDuckGoHtml => WebSearchBackendCapabilities {
                locale: false,
                freshness: false,
                source_mode: false,
                pagination: true,
                extra_snippets: false,
            },
        }
    }

    pub(super) fn selector_aliases(self) -> &'static [&'static str] {
        match self {
            Self::BingRss => &["bing", "bing_rss"],
            Self::BraveApi => &["brave", "brave_api"],
            Self::ExaApi => &["exa", "exa_api"],
            Self::TavilyApi => &["tavily", "tavily_api"],
            Self::FirecrawlApi => &["firecrawl", "firecrawl_api"],
            Self::DuckDuckGoHtml => &["duckduckgo", "duckduckgo_html", "ddg"],
        }
    }

    fn unavailable_message(self) -> &'static str {
        match self {
            Self::BingRss => "bing_rss backend is not registered",
            Self::BraveApi => {
                "NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_KEY is required for the brave_api backend"
            }
            Self::ExaApi => {
                "NANOCLAW_CORE_WEB_SEARCH_EXA_API_KEY is required for the exa_api backend"
            }
            Self::TavilyApi => {
                "TAVILY_API_KEY or NANOCLAW_CORE_WEB_SEARCH_TAVILY_API_KEY is required for the tavily_api backend"
            }
            Self::FirecrawlApi => {
                "FIRECRAWL_API_KEY or NANOCLAW_CORE_WEB_SEARCH_FIRECRAWL_API_KEY is required for the firecrawl_api backend"
            }
            Self::DuckDuckGoHtml => "duckduckgo_html backend is not registered",
        }
    }

    pub(super) fn missing_requirement(self) -> Option<&'static str> {
        match self {
            Self::BraveApi => Some("NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_KEY"),
            Self::ExaApi => Some("NANOCLAW_CORE_WEB_SEARCH_EXA_API_KEY"),
            Self::TavilyApi => Some("TAVILY_API_KEY or NANOCLAW_CORE_WEB_SEARCH_TAVILY_API_KEY"),
            Self::FirecrawlApi => {
                Some("FIRECRAWL_API_KEY or NANOCLAW_CORE_WEB_SEARCH_FIRECRAWL_API_KEY")
            }
            Self::BingRss | Self::DuckDuckGoHtml => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WebSearchBackendSelector {
    Auto,
    Kind(WebSearchBackendKind),
}

impl WebSearchBackendSelector {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Kind(kind) => kind.backend_name(),
        }
    }
}

#[derive(Clone)]
pub(super) struct WebSearchBackendRegistry {
    default_selector: WebSearchBackendSelector,
    backends: BTreeMap<WebSearchBackendKind, Arc<dyn WebSearchBackend>>,
}

impl WebSearchBackendRegistry {
    pub(super) fn from_env() -> Result<Self> {
        let default_selector = parse_backend_selector(
            agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_BACKEND).as_deref(),
        )?
        .unwrap_or(WebSearchBackendSelector::Auto);

        let mut backends = BTreeMap::<WebSearchBackendKind, Arc<dyn WebSearchBackend>>::new();
        backends.insert(
            WebSearchBackendKind::BingRss,
            Arc::new(BingRssSearchBackend::new(parse_backend_url(
                agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_ENDPOINT),
                DEFAULT_SEARCH_ENDPOINT,
                "search endpoint",
            )?)),
        );
        backends.insert(
            WebSearchBackendKind::DuckDuckGoHtml,
            Arc::new(DuckDuckGoHtmlSearchBackend::new(parse_backend_url(
                agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_DUCKDUCKGO_ENDPOINT),
                DEFAULT_DUCKDUCKGO_HTML_ENDPOINT,
                "DuckDuckGo HTML endpoint",
            )?)),
        );

        if let Some(api_key) =
            agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_KEY)
                .or_else(|| agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_API_KEY))
        {
            backends.insert(
                WebSearchBackendKind::BraveApi,
                Arc::new(BraveApiSearchBackend::new(
                    parse_backend_url(
                        agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_ENDPOINT)
                            .or_else(|| {
                                agent_env::get_non_empty(
                                    vars::NANOCLAW_CORE_WEB_SEARCH_API_ENDPOINT,
                                )
                            }),
                        DEFAULT_BRAVE_API_BASE_URL,
                        "Brave API endpoint",
                    )?,
                    api_key,
                )),
            );
        }

        if let Some(api_key) = agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_EXA_API_KEY)
        {
            backends.insert(
                WebSearchBackendKind::ExaApi,
                Arc::new(ExaApiSearchBackend::new(
                    parse_backend_url(
                        agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_EXA_API_ENDPOINT),
                        DEFAULT_EXA_API_BASE_URL,
                        "Exa API endpoint",
                    )?,
                    api_key,
                )),
            );
        }

        if let Some(api_key) =
            agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_TAVILY_API_KEY)
                .or_else(|| agent_env::get_non_empty(vars::TAVILY_API_KEY))
        {
            backends.insert(
                WebSearchBackendKind::TavilyApi,
                Arc::new(TavilyApiSearchBackend::new(
                    parse_backend_url(
                        agent_env::get_non_empty(
                            vars::NANOCLAW_CORE_WEB_SEARCH_TAVILY_API_ENDPOINT,
                        ),
                        DEFAULT_TAVILY_API_BASE_URL,
                        "Tavily API endpoint",
                    )?,
                    api_key,
                )),
            );
        }

        if let Some(api_key) =
            agent_env::get_non_empty(vars::NANOCLAW_CORE_WEB_SEARCH_FIRECRAWL_API_KEY)
                .or_else(|| agent_env::get_non_empty(vars::FIRECRAWL_API_KEY))
        {
            backends.insert(
                WebSearchBackendKind::FirecrawlApi,
                Arc::new(FirecrawlApiSearchBackend::new(
                    parse_backend_url(
                        agent_env::get_non_empty(
                            vars::NANOCLAW_CORE_WEB_SEARCH_FIRECRAWL_API_ENDPOINT,
                        ),
                        DEFAULT_FIRECRAWL_API_BASE_URL,
                        "Firecrawl API endpoint",
                    )?,
                    api_key,
                )),
            );
        }

        let registry = Self {
            default_selector,
            backends,
        };
        registry.ensure_selector_available(default_selector)?;
        Ok(registry)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn single(kind: WebSearchBackendKind, backend: Arc<dyn WebSearchBackend>) -> Self {
        let mut backends = BTreeMap::new();
        backends.insert(kind, backend);
        Self {
            default_selector: WebSearchBackendSelector::Kind(kind),
            backends,
        }
    }

    #[cfg(test)]
    pub(super) fn from_backends<I>(default_selector: WebSearchBackendSelector, backends: I) -> Self
    where
        I: IntoIterator<Item = (WebSearchBackendKind, Arc<dyn WebSearchBackend>)>,
    {
        Self {
            default_selector,
            backends: backends.into_iter().collect(),
        }
    }

    pub(super) fn available_backend_names(&self) -> Vec<String> {
        Self::auto_priority()
            .into_iter()
            .filter(|kind| self.backends.contains_key(kind))
            .map(|kind| kind.backend_name().to_string())
            .collect()
    }

    pub(super) fn default_selector(&self) -> WebSearchBackendSelector {
        self.default_selector
    }

    pub(super) fn resolved_default_kind(&self) -> Option<WebSearchBackendKind> {
        self.resolve_selector_kind(self.default_selector)
    }

    pub(super) fn contains(&self, kind: WebSearchBackendKind) -> bool {
        self.backends.contains_key(&kind)
    }

    pub(super) fn get(&self, kind: WebSearchBackendKind) -> Option<Arc<dyn WebSearchBackend>> {
        self.backends.get(&kind).cloned()
    }

    pub(super) fn known_backend_kinds(
        &self,
        include_unconfigured: bool,
    ) -> Vec<WebSearchBackendKind> {
        Self::auto_priority()
            .into_iter()
            .filter(|kind| include_unconfigured || self.contains(*kind))
            .collect()
    }

    pub(super) fn auto_priority_rank(kind: WebSearchBackendKind) -> usize {
        Self::auto_priority()
            .into_iter()
            .position(|candidate| candidate == kind)
            .map(|index| index + 1)
            .expect("known backend kind")
    }

    pub(super) fn resolve(
        &self,
        requested: Option<&str>,
    ) -> Result<(WebSearchBackendSelector, Arc<dyn WebSearchBackend>)> {
        let selector = parse_backend_selector(requested)?.unwrap_or(self.default_selector);
        self.ensure_selector_available(selector)?;
        let kind = self
            .resolve_selector_kind(selector)
            .ok_or_else(|| ToolError::invalid("no configured web search backends are available"))?;
        let backend = self
            .backends
            .get(&kind)
            .cloned()
            .ok_or_else(|| ToolError::invalid(kind.unavailable_message()))?;
        Ok((selector, backend))
    }

    fn ensure_selector_available(&self, selector: WebSearchBackendSelector) -> Result<()> {
        match selector {
            WebSearchBackendSelector::Auto => {
                if self.resolve_auto_kind().is_some() {
                    Ok(())
                } else {
                    Err(ToolError::invalid(
                        "no configured web search backends are available",
                    ))
                }
            }
            WebSearchBackendSelector::Kind(kind) => {
                if self.backends.contains_key(&kind) {
                    Ok(())
                } else {
                    Err(ToolError::invalid(kind.unavailable_message()))
                }
            }
        }
    }

    fn resolve_selector_kind(
        &self,
        selector: WebSearchBackendSelector,
    ) -> Option<WebSearchBackendKind> {
        match selector {
            WebSearchBackendSelector::Auto => self.resolve_auto_kind(),
            WebSearchBackendSelector::Kind(kind) => self.contains(kind).then_some(kind),
        }
    }

    fn resolve_auto_kind(&self) -> Option<WebSearchBackendKind> {
        // Auto prefers richer hosted APIs first, then the more stable RSS
        // fallback, and keeps HTML scraping last because challenge pages are
        // materially less predictable than feed-backed retrieval.
        Self::auto_priority()
            .into_iter()
            .find(|kind| self.backends.contains_key(kind))
    }

    fn auto_priority() -> [WebSearchBackendKind; 6] {
        [
            WebSearchBackendKind::ExaApi,
            WebSearchBackendKind::TavilyApi,
            WebSearchBackendKind::FirecrawlApi,
            WebSearchBackendKind::BraveApi,
            WebSearchBackendKind::BingRss,
            WebSearchBackendKind::DuckDuckGoHtml,
        ]
    }
}

pub(super) fn parse_backend_selector(
    value: Option<&str>,
) -> Result<Option<WebSearchBackendSelector>> {
    match value
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    {
        None => Ok(None),
        Some("auto") => Ok(Some(WebSearchBackendSelector::Auto)),
        Some("bing") | Some("bing_rss") => Ok(Some(WebSearchBackendSelector::Kind(
            WebSearchBackendKind::BingRss,
        ))),
        Some("brave") | Some("brave_api") => Ok(Some(WebSearchBackendSelector::Kind(
            WebSearchBackendKind::BraveApi,
        ))),
        Some("exa") | Some("exa_api") => Ok(Some(WebSearchBackendSelector::Kind(
            WebSearchBackendKind::ExaApi,
        ))),
        Some("tavily") | Some("tavily_api") => Ok(Some(WebSearchBackendSelector::Kind(
            WebSearchBackendKind::TavilyApi,
        ))),
        Some("firecrawl") | Some("firecrawl_api") => Ok(Some(WebSearchBackendSelector::Kind(
            WebSearchBackendKind::FirecrawlApi,
        ))),
        Some("duckduckgo") | Some("duckduckgo_html") | Some("ddg") => Ok(Some(
            WebSearchBackendSelector::Kind(WebSearchBackendKind::DuckDuckGoHtml),
        )),
        Some(other) => Err(ToolError::invalid(format!(
            "unsupported web search backend `{other}`"
        ))),
    }
}

fn parse_backend_url(endpoint: Option<String>, default_url: &str, label: &str) -> Result<Url> {
    let endpoint = endpoint.unwrap_or_else(|| default_url.to_string());
    Url::parse(&endpoint).map_err(|error| ToolError::invalid(format!("invalid {label}: {error}")))
}
