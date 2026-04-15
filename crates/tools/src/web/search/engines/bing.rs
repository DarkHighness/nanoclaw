use super::super::{
    SearchBackendResponse, SearchResultItem, WebSearchBackend, WebSearchBackendCapabilities,
    WebSearchRequest, canonicalize_result_url, looks_like_markup_fragment, send_search_request,
};
use crate::Result;
use crate::web::common::WebToolPolicy;
use crate::web::common::{decode_html_entities, summarize_remote_body};
use async_trait::async_trait;
use quick_xml::Reader;
use quick_xml::events::{BytesCData, BytesRef, BytesStart, BytesText, Event};
use reqwest::{Client, Url};

#[derive(Clone, Debug)]
pub(crate) struct BingRssSearchBackend {
    endpoint: Url,
}

impl BingRssSearchBackend {
    pub(crate) fn new(endpoint: Url) -> Self {
        Self { endpoint }
    }

    fn supports_native_news_path(&self) -> bool {
        // Bing exposes a distinct RSS-compatible news endpoint. We only switch
        // paths for the known hosted fallback so custom test/server overrides
        // keep their caller-provided path contract.
        self.endpoint.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("www.bing.com") || host.eq_ignore_ascii_case("bing.com")
        })
    }

    pub(crate) fn build_request_url(&self, request: &WebSearchRequest) -> Result<Url> {
        let mut request_url = self.endpoint.clone();
        if matches!(request.source_mode, super::super::WebSearchSourceMode::News)
            && self.supports_native_news_path()
        {
            request_url.set_path("/news/search");
        }
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .append_pair("format", "rss")
            .append_pair("cc", &request.locale.country)
            .append_pair("setlang", &request.locale.language)
            .append_pair("q", &request.query);
        Ok(request_url)
    }
}

#[async_trait]
impl WebSearchBackend for BingRssSearchBackend {
    fn backend_name(&self) -> &'static str {
        "bing_rss"
    }

    fn backend_type(&self) -> super::super::WebSearchBackendType {
        super::super::WebSearchBackendType::RssFeed
    }

    fn retrieval_mode(&self) -> &'static str {
        "rss"
    }

    fn capabilities(&self) -> WebSearchBackendCapabilities {
        WebSearchBackendCapabilities {
            locale: true,
            freshness: false,
            source_mode: self.supports_native_news_path(),
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
        let request_url = self.build_request_url(request)?;
        let (final_url, status, content_type, body) =
            send_search_request(client, policy, request_url.clone(), None).await?;
        let results = if (200..300).contains(&status) {
            parse_feed_results(&body)
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
            results,
            more_results_available: None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FeedField {
    Title,
    Link,
    Snippet,
    PublishedAt,
    SourceName,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FeedResultBuilder {
    pub(super) title: String,
    pub(super) url: String,
    pub(super) snippet: String,
    pub(super) published_at: String,
    pub(super) source_name: String,
}

pub(crate) fn parse_feed_results(xml: &str) -> Vec<SearchResultItem> {
    // Search fallbacks still bootstrap from XML feeds, but we do not treat feed
    // parsing itself as a regex contract. Namespaces, CDATA, and Atom link
    // attributes are common enough that a real token stream is the safer boundary.
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut results = Vec::new();
    let mut current_result = None;
    let mut current_field = None;
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let event_name = event.name();
                let name = xml_name(event_name.as_ref());
                match name {
                    b"item" | b"entry" => {
                        current_result = Some(FeedResultBuilder::default());
                        current_field = None;
                    }
                    b"title" if current_result.is_some() => current_field = Some(FeedField::Title),
                    b"description" | b"summary" | b"content" if current_result.is_some() => {
                        current_field = Some(FeedField::Snippet);
                    }
                    b"pubDate" | b"published" | b"updated" if current_result.is_some() => {
                        current_field = Some(FeedField::PublishedAt);
                    }
                    b"Source" | b"source" if current_result.is_some() => {
                        current_field = Some(FeedField::SourceName);
                    }
                    b"link" if current_result.is_some() => {
                        if let Some(builder) = current_result.as_mut()
                            && builder.url.is_empty()
                            && let Some(href) = feed_link_href(&event)
                        {
                            builder.url = href;
                        }
                        current_field = Some(FeedField::Link);
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => {
                let event_name = event.name();
                let name = xml_name(event_name.as_ref());
                if matches!(name, b"link")
                    && let Some(builder) = current_result.as_mut()
                    && builder.url.is_empty()
                    && let Some(href) = feed_link_href(&event)
                {
                    builder.url = href;
                }
            }
            Ok(Event::Text(text)) => {
                if let (Some(builder), Some(field)) = (current_result.as_mut(), current_field) {
                    append_feed_text(builder, field, decode_feed_text(&text).as_ref());
                }
            }
            Ok(Event::CData(text)) => {
                if let (Some(builder), Some(field)) = (current_result.as_mut(), current_field) {
                    append_feed_text(builder, field, decode_feed_cdata(&text).as_ref());
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let (Some(builder), Some(field)) = (current_result.as_mut(), current_field) {
                    append_feed_entity(builder, field, decode_feed_ref(&reference).as_ref());
                }
            }
            Ok(Event::End(event)) => {
                let event_name = event.name();
                let name = xml_name(event_name.as_ref());
                match name {
                    b"item" | b"entry" => {
                        if let Some(builder) = current_result.take()
                            && let Some(result) = finalize_feed_result(builder)
                        {
                            results.push(result);
                        }
                        current_field = None;
                    }
                    b"title" => clear_field(&mut current_field, FeedField::Title),
                    b"link" => clear_field(&mut current_field, FeedField::Link),
                    b"description" | b"summary" | b"content" => {
                        clear_field(&mut current_field, FeedField::Snippet);
                    }
                    b"pubDate" | b"published" | b"updated" => {
                        clear_field(&mut current_field, FeedField::PublishedAt);
                    }
                    b"Source" | b"source" => clear_field(&mut current_field, FeedField::SourceName),
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buffer.clear();
    }

    results
}

fn xml_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn feed_link_href(event: &BytesStart<'_>) -> Option<String> {
    let mut href = None;
    let mut rel = None;
    for attribute in event.attributes().flatten() {
        match xml_name(attribute.key.as_ref()) {
            b"href" => {
                href = attribute
                    .decode_and_unescape_value(event.decoder())
                    .ok()
                    .map(|value| value.into_owned());
            }
            b"rel" => {
                rel = attribute
                    .decode_and_unescape_value(event.decoder())
                    .ok()
                    .map(|value| value.into_owned());
            }
            _ => {}
        }
    }

    let rel = rel.unwrap_or_else(|| "alternate".to_string());
    if !matches!(rel.as_str(), "alternate" | "self") {
        return None;
    }
    href.filter(|value| !value.is_empty())
}

fn append_feed_text(builder: &mut FeedResultBuilder, field: FeedField, raw: &[u8]) {
    let text = String::from_utf8_lossy(raw);
    let target = match field {
        FeedField::Title => &mut builder.title,
        FeedField::Link => &mut builder.url,
        FeedField::Snippet => &mut builder.snippet,
        FeedField::PublishedAt => &mut builder.published_at,
        FeedField::SourceName => &mut builder.source_name,
    };
    target.push_str(&text);
}

fn append_feed_entity(builder: &mut FeedResultBuilder, field: FeedField, raw: &str) {
    let target = match field {
        FeedField::Title => &mut builder.title,
        FeedField::Link => &mut builder.url,
        FeedField::Snippet => &mut builder.snippet,
        FeedField::PublishedAt => &mut builder.published_at,
        FeedField::SourceName => &mut builder.source_name,
    };
    target.push('&');
    target.push_str(raw);
    target.push(';');
}

fn decode_feed_text(text: &BytesText<'_>) -> String {
    text.xml_content()
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(text.as_ref()).into_owned())
}

fn decode_feed_cdata(text: &BytesCData<'_>) -> String {
    text.xml_content()
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(text.as_ref()).into_owned())
}

fn decode_feed_ref(reference: &BytesRef<'_>) -> String {
    reference
        .xml_content()
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(reference.as_ref()).into_owned())
}

fn clear_field(current_field: &mut Option<FeedField>, expected: FeedField) {
    if current_field.is_some_and(|field| field == expected) {
        *current_field = None;
    }
}

fn finalize_feed_result(builder: FeedResultBuilder) -> Option<SearchResultItem> {
    let title = normalize_feed_field(&builder.title, false)?;
    let url = normalize_feed_url(&builder.url)?;
    let (url, raw_url) = canonicalize_result_url(&url);
    let snippet = normalize_feed_field(&builder.snippet, true);
    let published_at = normalize_feed_field(&builder.published_at, false);
    let source_name = normalize_feed_field(&builder.source_name, false);

    Some(SearchResultItem {
        title,
        url,
        raw_url,
        snippet,
        extra_snippets: Vec::new(),
        published_at,
        source_name,
    })
}

fn normalize_feed_field(value: &str, prefer_html: bool) -> Option<String> {
    let raw = decode_html_entities(value.trim());
    if raw.is_empty() {
        return None;
    }
    let normalized = if prefer_html && looks_like_markup_fragment(&raw) {
        summarize_markup_fragment(&raw)
    } else {
        summarize_remote_body(&raw, None)
    };
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_feed_url(value: &str) -> Option<String> {
    let raw = decode_html_entities(value.trim());
    if raw.is_empty() {
        return None;
    }

    // Feed links often contain XML-escaped query strings. They need entity
    // decoding plus whitespace cleanup, but not the prose-oriented body
    // summarization path, which can legitimately rewrite punctuation.
    let normalized = raw.split_whitespace().collect::<String>();
    (!normalized.is_empty()).then_some(normalized)
}

fn summarize_markup_fragment(fragment: &str) -> String {
    let wrapped = format!("<fragment>{fragment}</fragment>");
    let mut reader = Reader::from_str(&wrapped);
    reader.config_mut().trim_text(true);

    let mut text = String::new();
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Text(value)) => {
                text.push_str(&String::from_utf8_lossy(value.as_ref()));
                text.push(' ');
            }
            Ok(Event::CData(value)) => {
                text.push_str(&String::from_utf8_lossy(value.as_ref()));
                text.push(' ');
            }
            Ok(Event::Eof) => break,
            Err(_) => return summarize_remote_body(fragment, None),
            _ => {}
        }
        buffer.clear();
    }

    normalize_markup_spacing(&summarize_remote_body(&text, None))
}

fn normalize_markup_spacing(value: &str) -> String {
    let mut normalized = value.to_string();
    for punctuation in [".", ",", "!", "?", ";", ":", ")", "]"] {
        normalized = normalized.replace(&format!(" {punctuation}"), punctuation);
    }
    normalized.replace("( ", "(")
}
