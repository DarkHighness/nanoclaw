use crate::{
    EmbeddingClient, EmbeddingConfig, ExpandedQuery, ExpandedQueryKind, InferenceError,
    LlmServiceConfig, QueryExpansionClient, QueryExpansionConfig, RerankClient, RerankConfig,
    RerankDocument, RerankJudgment, Result,
};
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub const OPENAI_COMPATIBLE_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Clone)]
pub struct HttpEmbeddingClient {
    model: String,
    client: reqwest::Client,
    base_url: String,
}

impl HttpEmbeddingClient {
    pub fn from_config(config: &EmbeddingConfig) -> Result<Self> {
        Ok(Self {
            model: config.model.clone(),
            client: http_client_from_service_parts(
                config.api_key.as_deref(),
                &config.headers,
                config.timeout_ms,
            )?,
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| OPENAI_COMPATIBLE_BASE_URL.to_string()),
        })
    }
}

#[derive(Clone)]
struct HttpChatClient {
    model: String,
    client: reqwest::Client,
    base_url: String,
}

impl HttpChatClient {
    fn from_config(config: &LlmServiceConfig) -> Result<Self> {
        Ok(Self {
            model: config.model.clone(),
            client: http_client_from_service_parts(
                config.api_key.as_deref(),
                &config.headers,
                config.timeout_ms,
            )?,
            base_url: config
                .base_url
                .clone()
                .unwrap_or_else(|| OPENAI_COMPATIBLE_BASE_URL.to_string()),
        })
    }

    async fn complete_json(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String> {
        let response = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .json(&json!({
                "model": if model.is_empty() { &self.model } else { model },
                "messages": [
                    {
                        "role": "system",
                        "content": system_prompt,
                    },
                    {
                        "role": "user",
                        "content": user_prompt,
                    }
                ],
                "temperature": 0.0,
            }))
            .send()
            .await
            .map_err(|error| InferenceError::invalid(error.to_string()))?;
        if !response.status().is_success() {
            return Err(InferenceError::invalid(format!(
                "generation service returned HTTP {}",
                response.status()
            )));
        }
        let payload: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|error| InferenceError::invalid(error.to_string()))?;
        let content = payload
            .choices
            .first()
            .and_then(|choice| extract_chat_content(&choice.message.content))
            .ok_or_else(|| InferenceError::invalid("generation service returned no content"))?;
        Ok(content)
    }
}

#[derive(Clone)]
pub struct HttpQueryExpansionClient {
    inner: HttpChatClient,
}

impl HttpQueryExpansionClient {
    pub fn from_config(config: &QueryExpansionConfig) -> Result<Self> {
        Ok(Self {
            inner: HttpChatClient::from_config(&config.service)?,
        })
    }
}

#[derive(Clone)]
pub struct HttpRerankClient {
    inner: HttpChatClient,
}

impl HttpRerankClient {
    pub fn from_config(config: &RerankConfig) -> Result<Self> {
        Ok(Self {
            inner: HttpChatClient::from_config(&config.service)?,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct EmbeddingResponseItem {
    embedding: Vec<f32>,
}

#[derive(Clone, Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingResponseItem>,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionMessage {
    content: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Clone, Debug, Deserialize)]
struct QueryExpansionPayload {
    #[serde(default)]
    queries: Vec<ExpandedQueryPayload>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum ExpandedQueryPayload {
    Typed {
        #[serde(rename = "type")]
        kind: ExpandedQueryKind,
        query: String,
    },
    Raw(String),
}

#[derive(Clone, Debug, Deserialize)]
struct RerankPayload {
    #[serde(default)]
    judgments: Vec<RerankJudgment>,
}

#[async_trait]
impl EmbeddingClient for HttpEmbeddingClient {
    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .client
            .post(format!(
                "{}/embeddings",
                self.base_url.trim_end_matches('/')
            ))
            .json(&json!({
                "model": if model.is_empty() { &self.model } else { model },
                "input": texts,
            }))
            .send()
            .await
            .map_err(|error| InferenceError::invalid(error.to_string()))?;
        if !response.status().is_success() {
            return Err(InferenceError::invalid(format!(
                "embedding service returned HTTP {}",
                response.status()
            )));
        }
        let payload: EmbeddingResponse = response
            .json()
            .await
            .map_err(|error| InferenceError::invalid(error.to_string()))?;
        Ok(payload
            .data
            .into_iter()
            .map(|item| item.embedding)
            .collect())
    }
}

#[async_trait]
impl QueryExpansionClient for HttpQueryExpansionClient {
    async fn expand(
        &self,
        model: &str,
        query: &str,
        variants: usize,
    ) -> Result<Vec<ExpandedQuery>> {
        if variants == 0 {
            return Ok(Vec::new());
        }
        let payload = self
            .inner
            .complete_json(
                model,
                "You expand retrieval queries for hybrid search. Return only typed search lines using the prefixes `lex:`, `vec:`, or `hyde:`. Do not include explanations, bullets, numbering, or the original query. Prefer concise keyword-heavy `lex:` lines, natural-language `vec:` lines, and at most one short hypothetical-answer `hyde:` line.",
                &format!(
                    "/no_think Expand this search query: {query}\nRequested semantic variations: {variants}"
                ),
            )
            .await?;
        parse_expanded_queries(&payload)
    }
}

#[async_trait]
impl RerankClient for HttpRerankClient {
    async fn rerank(
        &self,
        model: &str,
        query: &str,
        documents: &[RerankDocument],
    ) -> Result<Vec<RerankJudgment>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let payload = self
            .inner
            .complete_json(
                model,
                "You rerank retrieval candidates. Return strict JSON with key `judgments`, an array aligned to candidate order. Each item must contain `relevant` (boolean) and `confidence` (float between 0 and 1). Do not include explanations.",
                &format!(
                    "Query: {query}\nCandidates: {}\nReturn JSON only.",
                    serde_json::to_string(documents)
                        .map_err(|error| InferenceError::invalid(error.to_string()))?
                ),
            )
            .await?;
        let parsed = parse_json_relaxed::<RerankPayload>(&payload)?;
        if parsed.judgments.len() != documents.len() {
            return Err(InferenceError::invalid(format!(
                "rerank service returned {} judgments for {} candidates",
                parsed.judgments.len(),
                documents.len()
            )));
        }
        Ok(parsed.judgments)
    }
}

pub fn http_client_from_service_parts(
    api_key: Option<&str>,
    headers: &BTreeMap<String, String>,
    timeout_ms: u64,
) -> Result<reqwest::Client> {
    let mut default_headers = HeaderMap::new();
    default_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(api_key) = api_key {
        default_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|error| InferenceError::invalid(error.to_string()))?,
        );
    }
    for (key, value) in headers {
        default_headers.insert(
            HeaderName::from_bytes(key.as_bytes())
                .map_err(|error| InferenceError::invalid(error.to_string()))?,
            HeaderValue::from_str(value)
                .map_err(|error| InferenceError::invalid(error.to_string()))?,
        );
    }
    reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .default_headers(default_headers)
        .build()
        .map_err(|error| InferenceError::invalid(error.to_string()))
}

fn extract_chat_content(content: &Value) -> Option<String> {
    match content {
        Value::String(value) => Some(value.clone()),
        Value::Array(items) => {
            // OpenAI-compatible providers may return structured message parts
            // (`[{type,text}, ...]`) instead of a single content string.
            let text = items
                .iter()
                .filter_map(|item| match item {
                    Value::Object(map) => map.get("text").and_then(Value::as_str),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn parse_json_relaxed<T: DeserializeOwned>(raw: &str) -> Result<T> {
    serde_json::from_str(raw).or_else(|_| {
        // LLM endpoints occasionally wrap JSON with markdown fences or extra
        // prose; salvage the first obvious object/array before failing hard.
        extract_json_candidate(raw)
            .ok_or_else(|| InferenceError::invalid("response did not contain JSON"))
            .and_then(|candidate| serde_json::from_str(candidate).map_err(Into::into))
    })
}

fn extract_json_candidate(raw: &str) -> Option<&str> {
    let object = raw
        .find('{')
        .zip(raw.rfind('}'))
        .map(|(start, end)| &raw[start..=end]);
    let array = raw
        .find('[')
        .zip(raw.rfind(']'))
        .map(|(start, end)| &raw[start..=end]);
    object.or(array)
}

fn normalize_query(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn parse_expanded_queries(raw: &str) -> Result<Vec<ExpandedQuery>> {
    if let Ok(payload) = parse_json_relaxed::<QueryExpansionPayload>(raw) {
        let mut out = Vec::new();
        for query in payload.queries {
            match query {
                ExpandedQueryPayload::Typed { kind, query } => {
                    if !normalize_query(&query).is_empty() {
                        out.push(ExpandedQuery { kind, query });
                    }
                }
                ExpandedQueryPayload::Raw(line) => {
                    if let Some(parsed) = parse_typed_query_line(&line) {
                        out.push(parsed);
                    }
                }
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }

    let parsed = raw
        .lines()
        .filter_map(parse_typed_query_line)
        .collect::<Vec<_>>();
    if parsed.is_empty() {
        return Err(InferenceError::invalid(
            "query expansion did not return any typed lex:/vec:/hyde: lines",
        ));
    }
    Ok(parsed)
}

fn parse_typed_query_line(line: &str) -> Option<ExpandedQuery> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (prefix, query) = trimmed.split_once(':')?;
    let kind = match prefix.trim().to_ascii_lowercase().as_str() {
        "lex" => ExpandedQueryKind::Lex,
        "vec" => ExpandedQueryKind::Vec,
        "hyde" => ExpandedQueryKind::Hyde,
        _ => return None,
    };
    let query = query.trim();
    (!query.is_empty()).then(|| ExpandedQuery {
        kind,
        query: query.to_string(),
    })
}
