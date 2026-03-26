use crate::{
    EmbeddingConfig, LlmServiceConfig, MemoryBackend, MemoryDocument, MemoryEmbedConfig,
    MemoryGetRequest, MemorySearchHit, MemorySearchRequest, MemorySearchResponse, MemorySyncStatus,
    QueryExpansionConfig, RerankConfig, Result, chunk_corpus, lexical_score, load_memory_corpus,
};
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const INDEX_SCHEMA_VERSION: u32 = 2;
const INDEX_BACKEND_ID: &str = "memory-embed";
const OPENAI_COMPATIBLE_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpandedQueryKind {
    Lex,
    Vec,
    Hyde,
    Hybrid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandedQuery {
    pub kind: ExpandedQueryKind,
    pub query: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RerankJudgment {
    pub relevant: bool,
    pub confidence: f64,
}

#[async_trait]
pub trait EmbeddingClient: Send + Sync {
    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[async_trait]
pub trait QueryExpansionClient: Send + Sync {
    async fn expand(&self, model: &str, query: &str, variants: usize)
    -> Result<Vec<ExpandedQuery>>;
}

#[async_trait]
pub trait RerankClient: Send + Sync {
    async fn rerank(
        &self,
        model: &str,
        query: &str,
        documents: &[RerankDocument],
    ) -> Result<Vec<RerankJudgment>>;
}

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
            .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;
        if !response.status().is_success() {
            return Err(crate::MemoryError::invalid(format!(
                "generation service returned HTTP {}",
                response.status()
            )));
        }
        let payload: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;
        let content = payload
            .choices
            .first()
            .and_then(|choice| extract_chat_content(&choice.message.content))
            .ok_or_else(|| crate::MemoryError::invalid("generation service returned no content"))?;
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
            .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;
        if !response.status().is_success() {
            return Err(crate::MemoryError::invalid(format!(
                "embedding service returned HTTP {}",
                response.status()
            )));
        }
        let payload: EmbeddingResponse = response
            .json()
            .await
            .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;
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
                        .map_err(|error| crate::MemoryError::invalid(error.to_string()))?
                ),
            )
            .await?;
        let parsed = parse_json_relaxed::<RerankPayload>(&payload)?;
        if parsed.judgments.len() != documents.len() {
            return Err(crate::MemoryError::invalid(format!(
                "rerank service returned {} judgments for {} candidates",
                parsed.judgments.len(),
                documents.len()
            )));
        }
        Ok(parsed.judgments)
    }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedChunkEmbedding {
    chunk_id: String,
    path: String,
    snapshot_id: String,
    start_line: usize,
    end_line: usize,
    text: String,
    embedding: Vec<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedMemoryEmbedIndex {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    backend: String,
    #[serde(default)]
    config_fingerprint: String,
    #[serde(default)]
    document_snapshots: BTreeMap<String, String>,
    chunks: Vec<PersistedChunkEmbedding>,
}

#[derive(Clone, Debug, Default)]
struct CachedMemoryEmbedIndex {
    config_fingerprint: String,
    document_snapshots: BTreeMap<String, String>,
    chunks: BTreeMap<String, PersistedChunkEmbedding>,
}

#[derive(Clone, Debug)]
struct WeightedQuery {
    kind: ExpandedQueryKind,
    text: String,
    weight: f64,
    is_original: bool,
}

#[derive(Clone, Copy, Debug)]
enum RankedStreamKind {
    Lexical,
    Vector,
}

#[derive(Clone, Debug, Serialize)]
pub struct RerankDocument {
    title: String,
    path: String,
    text: String,
}

#[derive(Clone, Debug)]
struct CandidateAccumulator {
    chunk_id: String,
    chunk: crate::MemoryCorpusChunk,
    title: String,
    lexical_score: f64,
    vector_score: f64,
    retrieval_score: f64,
    final_score: f64,
    rerank_score: Option<f64>,
    matched_streams: usize,
    applied_mmr: bool,
}

pub struct MemoryEmbedBackend {
    workspace_root: PathBuf,
    config: MemoryEmbedConfig,
    client: Arc<dyn EmbeddingClient>,
    query_expander: Option<Arc<dyn QueryExpansionClient>>,
    reranker: Option<Arc<dyn RerankClient>>,
    state: RwLock<Option<CachedMemoryEmbedIndex>>,
}

impl MemoryEmbedBackend {
    #[must_use]
    pub fn new(
        workspace_root: PathBuf,
        config: MemoryEmbedConfig,
        client: Arc<dyn EmbeddingClient>,
    ) -> Self {
        Self {
            workspace_root,
            config,
            client,
            query_expander: None,
            reranker: None,
            state: RwLock::new(None),
        }
    }

    #[must_use]
    pub fn with_optional_clients(
        mut self,
        query_expander: Option<Arc<dyn QueryExpansionClient>>,
        reranker: Option<Arc<dyn RerankClient>>,
    ) -> Self {
        self.query_expander = query_expander;
        self.reranker = reranker;
        self
    }

    pub fn from_http_config(workspace_root: PathBuf, config: MemoryEmbedConfig) -> Result<Self> {
        let embedding = config
            .embedding
            .clone()
            .ok_or_else(|| crate::MemoryError::invalid("memory-embed requires embedding config"))?;
        let query_expander = config
            .query_expansion
            .as_ref()
            .map(HttpQueryExpansionClient::from_config)
            .transpose()?
            .map(|client| Arc::new(client) as Arc<dyn QueryExpansionClient>);
        let reranker = config
            .rerank
            .as_ref()
            .map(HttpRerankClient::from_config)
            .transpose()?
            .map(|client| Arc::new(client) as Arc<dyn RerankClient>);
        Ok(Self::new(
            workspace_root,
            config,
            Arc::new(HttpEmbeddingClient::from_config(&embedding)?),
        )
        .with_optional_clients(query_expander, reranker))
    }

    async fn ensure_chunk_index(
        &self,
        corpus: &crate::MemoryCorpus,
        chunks: &[crate::MemoryCorpusChunk],
    ) -> Result<CachedMemoryEmbedIndex> {
        let mut cached =
            if let Some(index) = self.state.read().expect("memory-embed read lock").clone() {
                index
            } else {
                self.load_persisted_index().await?
            };
        let current_snapshots = document_snapshots(corpus);
        let current_fingerprint = self.config_fingerprint()?;
        let fingerprint_changed = cached.config_fingerprint != current_fingerprint;
        if fingerprint_changed {
            // Embeddings are model-dependent. A changed embedding config invalidates all vectors
            // even when chunk boundaries and snapshots are unchanged.
            cached.chunks.clear();
        }
        let snapshots_changed = cached.document_snapshots != current_snapshots;
        let mut changed = fingerprint_changed || snapshots_changed;
        changed |= self.trim_cached_chunks(chunks, &mut cached);
        let titles = document_titles(corpus);
        let missing = chunks
            .iter()
            .filter(|chunk| !cached.chunks.contains_key(&chunk_id(chunk)))
            .cloned()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            for entry in self.embed_missing_chunks(&missing, &titles).await? {
                cached.chunks.insert(entry.chunk_id.clone(), entry);
            }
            changed = true;
        }
        cached.config_fingerprint = current_fingerprint;
        cached.document_snapshots = current_snapshots;
        if changed {
            self.persist_index(chunks, &cached).await?;
        }

        *self.state.write().expect("memory-embed write lock") = Some(cached.clone());
        Ok(cached)
    }

    async fn load_persisted_index(&self) -> Result<CachedMemoryEmbedIndex> {
        let path = self.index_path();
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CachedMemoryEmbedIndex::default());
            }
            Err(error) => return Err(error.into()),
        };
        let persisted: PersistedMemoryEmbedIndex = serde_json::from_str(&raw)?;
        if persisted.backend != INDEX_BACKEND_ID {
            return Ok(CachedMemoryEmbedIndex::default());
        }
        if persisted.schema_version != INDEX_SCHEMA_VERSION {
            return Ok(CachedMemoryEmbedIndex::default());
        }
        Ok(CachedMemoryEmbedIndex {
            config_fingerprint: persisted.config_fingerprint,
            document_snapshots: persisted.document_snapshots,
            chunks: persisted
                .chunks
                .into_iter()
                .map(|entry| (entry.chunk_id.clone(), entry))
                .collect(),
        })
    }

    fn trim_cached_chunks(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
        cached: &mut CachedMemoryEmbedIndex,
    ) -> bool {
        let valid_ids = chunks.iter().map(chunk_id).collect::<Vec<_>>();
        let before = cached.chunks.len();
        cached
            .chunks
            .retain(|chunk_id, _| valid_ids.iter().any(|valid| valid == chunk_id));
        cached.chunks.len() != before
    }

    async fn embed_missing_chunks(
        &self,
        missing: &[crate::MemoryCorpusChunk],
        titles: &BTreeMap<String, String>,
    ) -> Result<Vec<PersistedChunkEmbedding>> {
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        let batch_size = self
            .config
            .embedding
            .as_ref()
            .map(|embedding| embedding.batch_size.max(1))
            .unwrap_or(16);
        let model = self.embedding_model();
        let mut entries = Vec::with_capacity(missing.len());
        for group in missing.chunks(batch_size) {
            let texts = group
                .iter()
                .map(|chunk| {
                    format_document_embedding_input(
                        model,
                        titles
                            .get(&chunk.path)
                            .map(String::as_str)
                            .unwrap_or("Memory"),
                        &chunk.text,
                    )
                })
                .collect::<Vec<_>>();
            let embeddings = self.client.embed(model, &texts).await?;
            if embeddings.len() != group.len() {
                return Err(crate::MemoryError::invalid(format!(
                    "embedding service returned {} vectors for {} chunks",
                    embeddings.len(),
                    group.len()
                )));
            }
            for (chunk, embedding) in group.iter().zip(embeddings.into_iter()) {
                entries.push(PersistedChunkEmbedding {
                    chunk_id: chunk_id(chunk),
                    path: chunk.path.clone(),
                    snapshot_id: chunk.snapshot_id.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    text: chunk.text.clone(),
                    embedding,
                });
            }
        }
        Ok(entries)
    }

    fn config_fingerprint(&self) -> Result<String> {
        #[derive(Serialize)]
        struct FingerprintInput<'a> {
            include_globs: &'a Vec<String>,
            extra_paths: Vec<String>,
            target_tokens: usize,
            overlap_tokens: usize,
            embedding_provider: Option<&'a str>,
            embedding_model: Option<&'a str>,
            embedding_base_url: Option<&'a str>,
            embedding_headers: Option<&'a BTreeMap<String, String>>,
        }

        let payload = FingerprintInput {
            include_globs: &self.config.corpus.include_globs,
            extra_paths: self
                .config
                .corpus
                .extra_paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            target_tokens: self.config.chunking.target_tokens,
            overlap_tokens: self.config.chunking.overlap_tokens,
            embedding_provider: self
                .config
                .embedding
                .as_ref()
                .map(|cfg| cfg.provider.as_str()),
            embedding_model: self.config.embedding.as_ref().map(|cfg| cfg.model.as_str()),
            embedding_base_url: self
                .config
                .embedding
                .as_ref()
                .and_then(|cfg| cfg.base_url.as_deref()),
            embedding_headers: self.config.embedding.as_ref().map(|cfg| &cfg.headers),
        };
        let encoded = serde_json::to_vec(&payload)?;
        Ok(stable_digest(&encoded))
    }

    async fn persist_index(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
        cached: &CachedMemoryEmbedIndex,
    ) -> Result<()> {
        let valid_ids = chunks.iter().map(chunk_id).collect::<Vec<_>>();
        let persisted = PersistedMemoryEmbedIndex {
            schema_version: INDEX_SCHEMA_VERSION,
            backend: INDEX_BACKEND_ID.to_string(),
            config_fingerprint: cached.config_fingerprint.clone(),
            document_snapshots: cached.document_snapshots.clone(),
            chunks: valid_ids
                .iter()
                .filter_map(|chunk_id| cached.chunks.get(chunk_id).cloned())
                .collect(),
        };
        let path = self.index_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, serde_json::to_vec_pretty(&persisted)?).await?;
        Ok(())
    }

    fn index_path(&self) -> PathBuf {
        self.config.index_path.clone().unwrap_or_else(|| {
            self.workspace_root
                .join(".agent-core/memory/memory-embed.json")
        })
    }

    fn embedding_model(&self) -> &str {
        self.config
            .embedding
            .as_ref()
            .map(|config| config.model.as_str())
            .unwrap_or("")
    }

    async fn expand_queries(&self, query: &str) -> (Vec<WeightedQuery>, bool, bool) {
        let Some(config) = self.config.query_expansion.as_ref() else {
            return (
                vec![WeightedQuery {
                    kind: ExpandedQueryKind::Hybrid,
                    text: query.to_string(),
                    weight: 1.0,
                    is_original: true,
                }],
                false,
                false,
            );
        };
        if config.variants == 0 || self.query_expander.is_none() {
            return (
                vec![WeightedQuery {
                    kind: ExpandedQueryKind::Hybrid,
                    text: query.to_string(),
                    weight: 1.0,
                    is_original: true,
                }],
                false,
                false,
            );
        }
        let result = self
            .query_expander
            .as_ref()
            .expect("query expander checked above")
            .expand(&config.service.model, query, config.variants)
            .await;
        match result {
            Ok(expansions) => {
                let mut seen =
                    BTreeSet::from([(ExpandedQueryKind::Hybrid, normalize_query(query))]);
                let mut weighted = Vec::new();
                let mut unique_expansions = Vec::new();
                for expansion in expansions {
                    let normalized = normalize_query(&expansion.query);
                    if normalized.is_empty() || !seen.insert((expansion.kind, normalized)) {
                        continue;
                    }
                    unique_expansions.push(expansion);
                }
                weighted.push(WeightedQuery {
                    kind: ExpandedQueryKind::Hybrid,
                    text: query.to_string(),
                    weight: if unique_expansions.is_empty() {
                        1.0
                    } else {
                        2.0
                    },
                    is_original: true,
                });
                weighted.extend(
                    unique_expansions
                        .into_iter()
                        .map(|expansion| WeightedQuery {
                            kind: expansion.kind,
                            text: expansion.query,
                            weight: 1.0,
                            is_original: false,
                        }),
                );
                let expansion_used = weighted.len() > 1;
                (weighted, expansion_used, false)
            }
            Err(_) => (
                vec![WeightedQuery {
                    kind: ExpandedQueryKind::Hybrid,
                    text: query.to_string(),
                    weight: 1.0,
                    is_original: true,
                }],
                false,
                true,
            ),
        }
    }

    async fn embed_query_vectors(
        &self,
        weighted_queries: &[WeightedQuery],
    ) -> Result<BTreeMap<String, Vec<f32>>> {
        let unique_queries = weighted_queries
            .iter()
            .filter(|weighted| {
                matches!(
                    weighted.kind,
                    ExpandedQueryKind::Hybrid | ExpandedQueryKind::Vec | ExpandedQueryKind::Hyde
                )
            })
            .map(|weighted| query_vector_key(weighted.kind, &weighted.text))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if unique_queries.is_empty() {
            return Ok(BTreeMap::new());
        }
        let model = self.embedding_model();
        let payloads = unique_queries
            .iter()
            .map(|query_key| format_query_variant_embedding_input(model, query_key))
            .collect::<Vec<_>>();
        let embeddings = self.client.embed(model, &payloads).await?;
        if embeddings.len() != unique_queries.len() {
            return Err(crate::MemoryError::invalid(format!(
                "embedding service returned {} vectors for {} query variants",
                embeddings.len(),
                unique_queries.len()
            )));
        }
        Ok(unique_queries
            .into_iter()
            .zip(embeddings.into_iter())
            .collect())
    }

    fn candidate_limit(&self, requested_limit: usize) -> usize {
        let expanded_limit = requested_limit
            .max(1)
            .saturating_mul(self.config.hybrid.candidate_multiplier.max(1));
        expanded_limit.max(self.config.hybrid.rerank_top_k.max(1))
    }

    async fn maybe_rerank(
        &self,
        query: &str,
        mut candidates: Vec<CandidateAccumulator>,
    ) -> Result<(Vec<CandidateAccumulator>, bool, bool)> {
        for candidate in &mut candidates {
            candidate.final_score = candidate.retrieval_score;
        }
        let Some(config) = self.config.rerank.as_ref() else {
            return Ok((candidates, false, false));
        };
        let Some(reranker) = self.reranker.as_ref() else {
            return Ok((candidates, false, false));
        };
        if candidates.is_empty() {
            return Ok((candidates, false, false));
        }

        let rerank_limit = self.config.hybrid.rerank_top_k.max(1).min(candidates.len());
        let mut rerank_pool = candidates
            .iter()
            .take(rerank_limit)
            .cloned()
            .collect::<Vec<_>>();
        let documents = rerank_pool
            .iter()
            .map(|candidate| RerankDocument {
                title: candidate.title.clone(),
                path: candidate.chunk.path.clone(),
                text: candidate.chunk.text.clone(),
            })
            .collect::<Vec<_>>();

        let judgments = match reranker
            .rerank(&config.service.model, query, &documents)
            .await
        {
            Ok(judgments) => judgments,
            Err(_) => return Ok((candidates, false, true)),
        };
        if judgments.len() != rerank_pool.len() {
            return Ok((candidates, false, true));
        }
        let max_retrieval = rerank_pool
            .iter()
            .map(|candidate| candidate.retrieval_score)
            .fold(0.0f64, f64::max)
            .max(f64::EPSILON);
        for (position, (candidate, judgment)) in rerank_pool
            .iter_mut()
            .zip(judgments.into_iter())
            .enumerate()
        {
            let normalized_retrieval = candidate.retrieval_score / max_retrieval;
            let (retrieval_weight, rerank_weight) = rerank_blend_weights(position);
            let rerank_score = if judgment.relevant {
                judgment.confidence
            } else {
                0.0
            };
            let clamped_rerank = rerank_score.clamp(0.0, 1.0);
            candidate.rerank_score = Some(clamped_rerank);
            candidate.final_score =
                (normalized_retrieval * retrieval_weight) + (clamped_rerank * rerank_weight);
        }
        Ok((rerank_pool, true, false))
    }
}

#[async_trait]
impl MemoryBackend for MemoryEmbedBackend {
    async fn sync(&self) -> Result<MemorySyncStatus> {
        let corpus = load_memory_corpus(&self.workspace_root, &self.config.corpus).await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        self.ensure_chunk_index(&corpus, &chunks).await?;
        Ok(MemorySyncStatus {
            backend: INDEX_BACKEND_ID.to_string(),
            indexed_documents: corpus.documents.len(),
            indexed_lines: corpus.total_lines(),
        })
    }

    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse> {
        let corpus = load_memory_corpus(&self.workspace_root, &self.config.corpus).await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        let limit = req
            .limit
            .unwrap_or(self.config.search.max_results)
            .max(1)
            .min(50);
        let prefix = req.path_prefix.map(|value| value.trim().to_string());
        let cached = self.ensure_chunk_index(&corpus, &chunks).await?;
        let titles = document_titles(&corpus);
        let filtered_chunks = chunks
            .iter()
            .filter(|chunk| {
                prefix
                    .as_deref()
                    .is_none_or(|path_prefix| chunk.path.starts_with(path_prefix))
            })
            .cloned()
            .collect::<Vec<_>>();
        let chunk_map = filtered_chunks
            .iter()
            .cloned()
            .map(|chunk| (chunk_id(&chunk), chunk))
            .collect::<BTreeMap<_, _>>();

        let (weighted_queries, expansion_used, expansion_fallback) =
            self.expand_queries(&req.query).await;
        let query_vectors = self.embed_query_vectors(&weighted_queries).await.ok();
        let fallback_used = query_vectors.is_none();
        let vector_weight = if fallback_used {
            0.0
        } else {
            self.config.hybrid.vector_weight.max(0.0)
        };
        let text_weight = if fallback_used {
            1.0
        } else {
            self.config.hybrid.text_weight.max(0.0)
        };
        let candidate_limit = self.candidate_limit(limit);
        let mut candidates = BTreeMap::<String, CandidateAccumulator>::new();

        for weighted_query in &weighted_queries {
            if matches!(
                weighted_query.kind,
                ExpandedQueryKind::Hybrid | ExpandedQueryKind::Lex
            ) {
                let lexical_ranked =
                    ranked_lexical_list(&filtered_chunks, &weighted_query.text, candidate_limit);
                apply_ranked_list(
                    &mut candidates,
                    &chunk_map,
                    &titles,
                    lexical_ranked,
                    RankedStreamKind::Lexical,
                    weighted_query,
                    text_weight,
                    &self.config,
                );
            }
            if matches!(
                weighted_query.kind,
                ExpandedQueryKind::Hybrid | ExpandedQueryKind::Vec | ExpandedQueryKind::Hyde
            ) {
                let query_key = query_vector_key(weighted_query.kind, &weighted_query.text);
                if let Some(query_vectors) = query_vectors.as_ref()
                    && let Some(query_vector) = query_vectors.get(&query_key)
                {
                    let vector_ranked = ranked_vector_list(
                        &filtered_chunks,
                        &cached,
                        query_vector,
                        candidate_limit,
                    );
                    apply_ranked_list(
                        &mut candidates,
                        &chunk_map,
                        &titles,
                        vector_ranked,
                        RankedStreamKind::Vector,
                        weighted_query,
                        vector_weight,
                        &self.config,
                    );
                }
            }
        }

        let mut ranked = candidates.into_values().collect::<Vec<_>>();
        ranked.sort_by(compare_candidates_by_retrieval);
        let (mut ranked, rerank_used, rerank_fallback) =
            self.maybe_rerank(&req.query, ranked).await?;
        ranked.sort_by(compare_candidates_by_final_score);
        let (mut ranked, mmr_used) = maybe_apply_mmr(&ranked, &cached, limit, &self.config);
        if !mmr_used {
            ranked.truncate(limit);
        }

        let hits = ranked
            .into_iter()
            .map(|candidate| {
                let mut metadata = BTreeMap::new();
                metadata.insert("lexical_score".to_string(), json!(candidate.lexical_score));
                metadata.insert("vector_score".to_string(), json!(candidate.vector_score));
                metadata.insert(
                    "retrieval_score".to_string(),
                    json!(candidate.retrieval_score),
                );
                metadata.insert(
                    "matched_streams".to_string(),
                    json!(candidate.matched_streams),
                );
                metadata.insert("mmr_applied".to_string(), json!(candidate.applied_mmr));
                if let Some(rerank_score) = candidate.rerank_score {
                    metadata.insert("rerank_score".to_string(), json!(rerank_score));
                }
                if let Some(embedding) = &self.config.embedding {
                    metadata.insert("model".to_string(), json!(embedding.model));
                    metadata.insert("provider".to_string(), json!(embedding.provider));
                }
                metadata.insert(
                    "snapshot_id".to_string(),
                    json!(candidate.chunk.snapshot_id),
                );
                metadata.insert("title".to_string(), json!(candidate.title));
                MemorySearchHit {
                    hit_id: format!("{}:{}", candidate.chunk.path, candidate.chunk.start_line),
                    path: candidate.chunk.path.clone(),
                    start_line: candidate.chunk.start_line,
                    end_line: candidate.chunk.end_line,
                    score: candidate.final_score,
                    snippet: render_embed_snippet(
                        &candidate.chunk.text,
                        self.config.search.max_snippet_chars,
                    ),
                    metadata,
                }
            })
            .collect::<Vec<_>>();

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "indexed_documents".to_string(),
            json!(corpus.documents.len()),
        );
        metadata.insert("cached_chunks".to_string(), json!(cached.chunks.len()));
        metadata.insert("fallback_used".to_string(), json!(fallback_used));
        metadata.insert("expansion_used".to_string(), json!(expansion_used));
        metadata.insert("expansion_fallback".to_string(), json!(expansion_fallback));
        metadata.insert("query_variants".to_string(), json!(weighted_queries.len()));
        metadata.insert("candidate_limit".to_string(), json!(candidate_limit));
        metadata.insert("rerank_used".to_string(), json!(rerank_used));
        metadata.insert("rerank_fallback".to_string(), json!(rerank_fallback));
        metadata.insert("mmr_used".to_string(), json!(mmr_used));
        metadata.insert(
            "mmr_lambda".to_string(),
            json!(self.config.hybrid.mmr_lambda),
        );
        metadata.insert(
            "index_path".to_string(),
            json!(self.index_path().to_string_lossy().to_string()),
        );
        metadata.insert("schema_version".to_string(), json!(INDEX_SCHEMA_VERSION));
        metadata.insert(
            "config_fingerprint".to_string(),
            json!(cached.config_fingerprint),
        );
        Ok(MemorySearchResponse {
            backend: INDEX_BACKEND_ID.to_string(),
            hits,
            metadata,
        })
    }

    async fn get(&self, req: MemoryGetRequest) -> Result<MemoryDocument> {
        crate::MemoryCoreBackend::new(self.workspace_root.clone(), self.config.as_core_config())
            .get(req)
            .await
    }
}

fn http_client_from_service_parts(
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
                .map_err(|error| crate::MemoryError::invalid(error.to_string()))?,
        );
    }
    for (key, value) in headers {
        default_headers.insert(
            HeaderName::from_bytes(key.as_bytes())
                .map_err(|error| crate::MemoryError::invalid(error.to_string()))?,
            HeaderValue::from_str(value)
                .map_err(|error| crate::MemoryError::invalid(error.to_string()))?,
        );
    }
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .default_headers(default_headers)
        .build()
        .map_err(|error| crate::MemoryError::invalid(error.to_string()))
}

fn extract_chat_content(content: &Value) -> Option<String> {
    match content {
        Value::String(value) => Some(value.clone()),
        Value::Array(items) => {
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
        extract_json_candidate(raw)
            .ok_or_else(|| crate::MemoryError::invalid("response did not contain JSON"))
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

fn parse_expanded_queries(raw: &str) -> Result<Vec<ExpandedQuery>> {
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
        return Err(crate::MemoryError::invalid(
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

fn query_vector_key(kind: ExpandedQueryKind, query: &str) -> String {
    format!("{kind:?}:{}", query.trim())
}

fn format_query_variant_embedding_input(model: &str, query_key: &str) -> String {
    let (kind, query) = query_key
        .split_once(':')
        .map(|(kind, query)| (kind.to_ascii_lowercase(), query))
        .unwrap_or_else(|| ("hybrid".to_string(), query_key));
    match kind.as_str() {
        "hyde" => format_document_embedding_input(model, "HyDE", query),
        _ => format_query_embedding_input(model, query),
    }
}

fn format_query_embedding_input(model: &str, query: &str) -> String {
    match embedding_prompt_style(model) {
        EmbeddingPromptStyle::Plain => query.to_string(),
        EmbeddingPromptStyle::EmbeddingGemma => {
            format!("task: search result | query: {}", query.trim())
        }
        EmbeddingPromptStyle::Qwen3 => format!(
            "Instruct: Given a search query, retrieve relevant passages that answer the query\nQuery: {}",
            query.trim()
        ),
    }
}

fn format_document_embedding_input(model: &str, title: &str, text: &str) -> String {
    match embedding_prompt_style(model) {
        EmbeddingPromptStyle::Plain | EmbeddingPromptStyle::Qwen3 => text.to_string(),
        EmbeddingPromptStyle::EmbeddingGemma => {
            format!("title: {} | text: {}", title.trim(), text.trim())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmbeddingPromptStyle {
    Plain,
    EmbeddingGemma,
    Qwen3,
}

fn embedding_prompt_style(model: &str) -> EmbeddingPromptStyle {
    let normalized = model.to_ascii_lowercase();
    if normalized.contains("embeddinggemma") {
        EmbeddingPromptStyle::EmbeddingGemma
    } else if normalized.contains("qwen3-embedding") {
        EmbeddingPromptStyle::Qwen3
    } else {
        EmbeddingPromptStyle::Plain
    }
}

fn chunk_id(chunk: &crate::MemoryCorpusChunk) -> String {
    let mut digest = Sha256::new();
    digest.update(chunk.path.as_bytes());
    digest.update(b":");
    digest.update(chunk.snapshot_id.as_bytes());
    digest.update(b":");
    digest.update(chunk.start_line.to_string().as_bytes());
    digest.update(b":");
    digest.update(chunk.end_line.to_string().as_bytes());
    stable_digest(&digest.finalize())
}

fn document_snapshots(corpus: &crate::MemoryCorpus) -> BTreeMap<String, String> {
    corpus
        .documents
        .iter()
        .map(|document| (document.path.clone(), document.snapshot_id.clone()))
        .collect()
}

fn document_titles(corpus: &crate::MemoryCorpus) -> BTreeMap<String, String> {
    corpus
        .documents
        .iter()
        .map(|document| (document.path.clone(), document_title(document)))
        .collect()
}

fn document_title(document: &crate::MemoryCorpusDocument) -> String {
    document
        .lines
        .iter()
        .find_map(|line| line.strip_prefix('#').map(str::trim))
        .filter(|title| !title.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            document
                .path
                .rsplit('/')
                .next()
                .unwrap_or("Memory")
                .trim_end_matches(".md")
                .to_string()
        })
}

fn stable_digest(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn ranked_lexical_list(
    chunks: &[crate::MemoryCorpusChunk],
    query: &str,
    limit: usize,
) -> Vec<(String, f64)> {
    let mut ranked = chunks
        .iter()
        .filter_map(|chunk| {
            let score = lexical_score(query, &chunk.text);
            (score > 0.0).then(|| (chunk_id(chunk), score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(compare_ranked_score);
    ranked.truncate(limit);
    ranked
}

fn ranked_vector_list(
    chunks: &[crate::MemoryCorpusChunk],
    cached: &CachedMemoryEmbedIndex,
    query_vector: &[f32],
    limit: usize,
) -> Vec<(String, f64)> {
    let mut ranked = chunks
        .iter()
        .filter_map(|chunk| {
            let score = cached
                .chunks
                .get(&chunk_id(chunk))
                .map(|entry| cosine_similarity(query_vector, &entry.embedding))
                .unwrap_or(0.0);
            (score > 0.0).then(|| (chunk_id(chunk), score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(compare_ranked_score);
    ranked.truncate(limit);
    ranked
}

fn compare_ranked_score(left: &(String, f64), right: &(String, f64)) -> Ordering {
    right
        .1
        .partial_cmp(&left.1)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.0.cmp(&right.0))
}

fn apply_ranked_list(
    candidates: &mut BTreeMap<String, CandidateAccumulator>,
    chunk_map: &BTreeMap<String, crate::MemoryCorpusChunk>,
    titles: &BTreeMap<String, String>,
    ranked: Vec<(String, f64)>,
    stream_kind: RankedStreamKind,
    weighted_query: &WeightedQuery,
    stream_weight: f64,
    config: &MemoryEmbedConfig,
) {
    if stream_weight <= 0.0 {
        return;
    }
    for (rank, (chunk_id, score)) in ranked.into_iter().enumerate() {
        let Some(chunk) = chunk_map.get(&chunk_id) else {
            continue;
        };
        let entry = candidates
            .entry(chunk_id.clone())
            .or_insert_with(|| CandidateAccumulator {
                chunk_id: chunk_id.clone(),
                chunk: chunk.clone(),
                title: titles
                    .get(&chunk.path)
                    .cloned()
                    .unwrap_or_else(|| "Memory".to_string()),
                lexical_score: 0.0,
                vector_score: 0.0,
                retrieval_score: 0.0,
                final_score: 0.0,
                rerank_score: None,
                matched_streams: 0,
                applied_mmr: false,
            });
        match stream_kind {
            RankedStreamKind::Lexical => entry.lexical_score = entry.lexical_score.max(score),
            RankedStreamKind::Vector => entry.vector_score = entry.vector_score.max(score),
        }
        entry.matched_streams += 1;
        entry.retrieval_score +=
            (weighted_query.weight * stream_weight) / (config.hybrid.rrf_k + rank + 1) as f64;
        if weighted_query.is_original {
            if rank == 0 {
                entry.retrieval_score += config.hybrid.top_rank_bonus_first;
            } else if rank < 3 {
                entry.retrieval_score += config.hybrid.top_rank_bonus_other;
            }
        }
    }
}

fn compare_candidates_by_retrieval(
    left: &CandidateAccumulator,
    right: &CandidateAccumulator,
) -> Ordering {
    right
        .retrieval_score
        .partial_cmp(&left.retrieval_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.chunk.path.cmp(&right.chunk.path))
        .then_with(|| left.chunk.start_line.cmp(&right.chunk.start_line))
}

fn compare_candidates_by_final_score(
    left: &CandidateAccumulator,
    right: &CandidateAccumulator,
) -> Ordering {
    right
        .final_score
        .partial_cmp(&left.final_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| {
            right
                .retrieval_score
                .partial_cmp(&left.retrieval_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| left.chunk.path.cmp(&right.chunk.path))
        .then_with(|| left.chunk.start_line.cmp(&right.chunk.start_line))
}

fn maybe_apply_mmr(
    ranked: &[CandidateAccumulator],
    cached: &CachedMemoryEmbedIndex,
    limit: usize,
    config: &MemoryEmbedConfig,
) -> (Vec<CandidateAccumulator>, bool) {
    let Some(lambda) = config.hybrid.mmr_lambda else {
        return (ranked.to_vec(), false);
    };
    if ranked.is_empty() {
        return (Vec::new(), false);
    }
    let pool_size = config.hybrid.mmr_pool_k.max(limit).min(ranked.len());
    let mut remaining = ranked.iter().take(pool_size).cloned().collect::<Vec<_>>();
    let mut selected = Vec::new();

    while selected.len() < limit && !remaining.is_empty() {
        let max_relevance = remaining
            .iter()
            .map(|candidate| candidate.final_score.max(candidate.retrieval_score))
            .fold(0.0f64, f64::max)
            .max(f64::EPSILON);
        let (best_index, _) = remaining
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let relevance =
                    candidate.final_score.max(candidate.retrieval_score) / max_relevance;
                let redundancy = selected
                    .iter()
                    .map(|other| candidate_similarity(candidate, other, cached))
                    .fold(0.0f64, f64::max);
                (index, (lambda * relevance) - ((1.0 - lambda) * redundancy))
            })
            .max_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| {
                        let left_candidate = &remaining[left.0];
                        let right_candidate = &remaining[right.0];
                        right_candidate
                            .final_score
                            .partial_cmp(&left_candidate.final_score)
                            .unwrap_or(Ordering::Equal)
                    })
            })
            .expect("remaining is non-empty");
        let mut chosen = remaining.remove(best_index);
        chosen.applied_mmr = true;
        selected.push(chosen);
    }
    if selected.len() < limit {
        selected.extend(remaining.into_iter().take(limit - selected.len()));
    }
    (selected, true)
}

fn candidate_similarity(
    left: &CandidateAccumulator,
    right: &CandidateAccumulator,
    cached: &CachedMemoryEmbedIndex,
) -> f64 {
    if left.chunk.path == right.chunk.path {
        if ranges_overlap(
            left.chunk.start_line,
            left.chunk.end_line,
            right.chunk.start_line,
            right.chunk.end_line,
        ) {
            return 1.0;
        }
        let line_gap = left
            .chunk
            .start_line
            .max(right.chunk.start_line)
            .saturating_sub(left.chunk.end_line.min(right.chunk.end_line));
        if line_gap <= 3 {
            return 0.85;
        }
    }

    let embedding_similarity = cached
        .chunks
        .get(&left.chunk_id)
        .zip(cached.chunks.get(&right.chunk_id))
        .map(|(left, right)| cosine_similarity(&left.embedding, &right.embedding))
        .unwrap_or(0.0);
    if embedding_similarity > 0.0 {
        return embedding_similarity;
    }
    text_jaccard_similarity(&left.chunk.text, &right.chunk.text)
}

fn ranges_overlap(
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) -> bool {
    left_start <= right_end && right_start <= left_end
}

fn text_jaccard_similarity(left: &str, right: &str) -> f64 {
    let left_tokens = tokenize_similarity_text(left);
    let right_tokens = tokenize_similarity_text(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let intersection = left_tokens.intersection(&right_tokens).count() as f64;
    let union = left_tokens.union(&right_tokens).count() as f64;
    if union <= f64::EPSILON {
        0.0
    } else {
        intersection / union
    }
}

fn tokenize_similarity_text(value: &str) -> BTreeSet<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .map(|token| token.to_ascii_lowercase())
        .filter(|token| !token.is_empty())
        .collect()
}

fn rerank_blend_weights(position: usize) -> (f64, f64) {
    if position < 3 {
        (0.75, 0.25)
    } else if position < 10 {
        (0.60, 0.40)
    } else {
        (0.40, 0.60)
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (lhs, rhs) in left.iter().zip(right.iter()) {
        dot += f64::from(*lhs) * f64::from(*rhs);
        left_norm += f64::from(*lhs) * f64::from(*lhs);
        right_norm += f64::from(*rhs) * f64::from(*rhs);
    }
    let denom = left_norm.sqrt() * right_norm.sqrt();
    if denom <= f64::EPSILON {
        0.0
    } else {
        dot / denom
    }
}

fn render_embed_snippet(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    format!(
        "{}...",
        text.chars()
            .take(max_chars.saturating_sub(3))
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CachedMemoryEmbedIndex, CandidateAccumulator, EmbeddingClient, ExpandedQuery,
        ExpandedQueryKind, MemoryEmbedBackend, PersistedChunkEmbedding, QueryExpansionClient,
        RerankClient, RerankDocument, RerankJudgment, format_document_embedding_input,
        format_query_embedding_input, maybe_apply_mmr, parse_typed_query_line,
        rerank_blend_weights,
    };
    use crate::{
        EmbeddingConfig, MemoryBackend, MemoryEmbedConfig, MemorySearchRequest,
        QueryExpansionConfig, RerankConfig, config::LlmServiceConfig,
    };
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use tokio::fs;

    #[derive(Default)]
    struct MockEmbeddingClient {
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait]
    impl EmbeddingClient for MockEmbeddingClient {
        async fn embed(&self, _model: &str, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
            self.calls.lock().unwrap().push(texts.to_vec());
            Ok(texts
                .iter()
                .map(|text| {
                    if text.contains("semantic")
                        || text.contains("query")
                        || text.contains("canary")
                        || text.contains("phased rollout")
                    {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
        }
    }

    struct FixedQueryExpansionClient {
        variants: Vec<ExpandedQuery>,
    }

    #[async_trait]
    impl QueryExpansionClient for FixedQueryExpansionClient {
        async fn expand(
            &self,
            _model: &str,
            _query: &str,
            _variants: usize,
        ) -> crate::Result<Vec<ExpandedQuery>> {
            Ok(self.variants.clone())
        }
    }

    struct FixedRerankClient {
        judgments: Vec<RerankJudgment>,
    }

    #[async_trait]
    impl RerankClient for FixedRerankClient {
        async fn rerank(
            &self,
            _model: &str,
            _query: &str,
            _documents: &[RerankDocument],
        ) -> crate::Result<Vec<RerankJudgment>> {
            Ok(self.judgments.clone())
        }
    }

    #[tokio::test]
    async fn hybrid_search_prefers_vector_match_when_available() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "semantic recall target")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(dir.path().join("memory/today.md"), "lexical only result")
            .await
            .unwrap();

        let backend = MemoryEmbedBackend::new(
            dir.path().to_path_buf(),
            MemoryEmbedConfig::default(),
            Arc::new(MockEmbeddingClient::default()),
        );
        let response = backend
            .search(MemorySearchRequest {
                query: "query".to_string(),
                limit: Some(2),
                path_prefix: None,
            })
            .await
            .unwrap();

        assert_eq!(response.backend, "memory-embed");
        assert!(!response.hits.is_empty());
        assert_eq!(response.hits[0].path, "MEMORY.md");
        assert_eq!(
            response
                .metadata
                .get("fallback_used")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn sync_reuses_persisted_chunk_embeddings() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "semantic recall target")
            .await
            .unwrap();
        let client = Arc::new(MockEmbeddingClient::default());
        let config = MemoryEmbedConfig::default();

        let backend =
            MemoryEmbedBackend::new(dir.path().to_path_buf(), config.clone(), client.clone());
        backend.sync().await.unwrap();
        let first_calls = client.calls.lock().unwrap().clone();
        assert_eq!(first_calls.len(), 1);
        assert_eq!(first_calls[0].len(), 1);

        let backend = MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone());
        backend.sync().await.unwrap();
        let second_calls = client.calls.lock().unwrap().clone();
        assert_eq!(second_calls.len(), 1);
    }

    #[tokio::test]
    async fn sync_after_content_change_only_embeds_new_chunks() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "semantic alpha")
            .await
            .unwrap();
        let client = Arc::new(MockEmbeddingClient::default());
        let config = MemoryEmbedConfig::default();

        let backend =
            MemoryEmbedBackend::new(dir.path().to_path_buf(), config.clone(), client.clone());
        backend.sync().await.unwrap();
        assert_eq!(client.calls.lock().unwrap().len(), 1);

        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(dir.path().join("memory/new.md"), "semantic beta")
            .await
            .unwrap();

        let restarted = MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone());
        restarted.sync().await.unwrap();
        assert_eq!(client.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn sync_batches_missing_chunks_by_embedding_batch_size() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        for index in 0..5 {
            fs::write(
                dir.path().join("memory").join(format!("d{index}.md")),
                format!("semantic chunk {index}"),
            )
            .await
            .unwrap();
        }
        let client = Arc::new(MockEmbeddingClient::default());
        let config = MemoryEmbedConfig {
            embedding: Some(EmbeddingConfig {
                provider: "mock".to_string(),
                model: "mock-small".to_string(),
                base_url: None,
                api_key: None,
                headers: BTreeMap::new(),
                batch_size: 2,
                timeout_ms: 30_000,
            }),
            ..MemoryEmbedConfig::default()
        };

        let backend = MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone());
        backend.sync().await.unwrap();
        let calls = client.calls.lock().unwrap().clone();
        assert_eq!(
            calls.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![2, 2, 1]
        );
    }

    #[tokio::test]
    async fn qmd_query_expansion_and_rerank_can_flip_top_candidate() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "canary deploy checklist")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(
            dir.path().join("memory/rollout.md"),
            "phased rollout plan for production",
        )
        .await
        .unwrap();

        let client = Arc::new(MockEmbeddingClient::default());
        let config = MemoryEmbedConfig {
            query_expansion: Some(QueryExpansionConfig {
                service: LlmServiceConfig {
                    provider: "mock".to_string(),
                    model: "mock-expander".to_string(),
                    base_url: None,
                    api_key: None,
                    headers: BTreeMap::new(),
                    timeout_ms: 30_000,
                },
                variants: 1,
            }),
            rerank: Some(RerankConfig {
                service: LlmServiceConfig {
                    provider: "mock".to_string(),
                    model: "mock-reranker".to_string(),
                    base_url: None,
                    api_key: None,
                    headers: BTreeMap::new(),
                    timeout_ms: 30_000,
                },
            }),
            ..MemoryEmbedConfig::default()
        };
        let backend = MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client)
            .with_optional_clients(
                Some(Arc::new(FixedQueryExpansionClient {
                    variants: vec![
                        ExpandedQuery {
                            kind: ExpandedQueryKind::Lex,
                            query: "canary rollout".to_string(),
                        },
                        ExpandedQuery {
                            kind: ExpandedQueryKind::Vec,
                            query: "phased rollout".to_string(),
                        },
                    ],
                })),
                Some(Arc::new(FixedRerankClient {
                    judgments: vec![
                        RerankJudgment {
                            relevant: false,
                            confidence: 0.9,
                        },
                        RerankJudgment {
                            relevant: true,
                            confidence: 1.0,
                        },
                    ],
                })),
            );

        let response = backend
            .search(MemorySearchRequest {
                query: "canary deploy".to_string(),
                limit: Some(2),
                path_prefix: None,
            })
            .await
            .unwrap();

        assert_eq!(response.hits.len(), 2);
        assert_eq!(response.hits[0].path, "memory/rollout.md");
        assert_eq!(
            response
                .metadata
                .get("expansion_used")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            response
                .metadata
                .get("rerank_used")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn typed_query_lines_parse_into_qmd_query_kinds() {
        let lex = parse_typed_query_line("lex: authentication config").unwrap();
        let vec = parse_typed_query_line("vec: how do I configure authentication").unwrap();
        let hyde =
            parse_typed_query_line("hyde: Authentication is configured with AUTH_SECRET").unwrap();

        assert_eq!(lex.kind, ExpandedQueryKind::Lex);
        assert_eq!(vec.kind, ExpandedQueryKind::Vec);
        assert_eq!(hyde.kind, ExpandedQueryKind::Hyde);
    }

    #[test]
    fn mmr_prefers_diverse_chunks_when_enabled() {
        let cached = CachedMemoryEmbedIndex {
            chunks: BTreeMap::from([
                (
                    "a".to_string(),
                    PersistedChunkEmbedding {
                        chunk_id: "a".to_string(),
                        path: "MEMORY.md".to_string(),
                        snapshot_id: "s1".to_string(),
                        start_line: 1,
                        end_line: 4,
                        text: "duplicate rollout canary".to_string(),
                        embedding: vec![1.0, 0.0],
                    },
                ),
                (
                    "b".to_string(),
                    PersistedChunkEmbedding {
                        chunk_id: "b".to_string(),
                        path: "MEMORY.md".to_string(),
                        snapshot_id: "s1".to_string(),
                        start_line: 3,
                        end_line: 6,
                        text: "duplicate rollout canary".to_string(),
                        embedding: vec![1.0, 0.0],
                    },
                ),
                (
                    "c".to_string(),
                    PersistedChunkEmbedding {
                        chunk_id: "c".to_string(),
                        path: "memory/other.md".to_string(),
                        snapshot_id: "s2".to_string(),
                        start_line: 1,
                        end_line: 4,
                        text: "fallback recovery procedure".to_string(),
                        embedding: vec![0.0, 1.0],
                    },
                ),
            ]),
            ..CachedMemoryEmbedIndex::default()
        };
        let ranked = vec![
            CandidateAccumulator {
                chunk_id: "a".to_string(),
                chunk: crate::MemoryCorpusChunk {
                    path: "MEMORY.md".to_string(),
                    snapshot_id: "s1".to_string(),
                    start_line: 1,
                    end_line: 4,
                    text: "duplicate rollout canary".to_string(),
                },
                title: "Memory".to_string(),
                lexical_score: 2.0,
                vector_score: 1.0,
                retrieval_score: 0.9,
                final_score: 0.9,
                rerank_score: None,
                matched_streams: 2,
                applied_mmr: false,
            },
            CandidateAccumulator {
                chunk_id: "b".to_string(),
                chunk: crate::MemoryCorpusChunk {
                    path: "MEMORY.md".to_string(),
                    snapshot_id: "s1".to_string(),
                    start_line: 3,
                    end_line: 6,
                    text: "duplicate rollout canary".to_string(),
                },
                title: "Memory".to_string(),
                lexical_score: 1.9,
                vector_score: 0.98,
                retrieval_score: 0.85,
                final_score: 0.85,
                rerank_score: None,
                matched_streams: 2,
                applied_mmr: false,
            },
            CandidateAccumulator {
                chunk_id: "c".to_string(),
                chunk: crate::MemoryCorpusChunk {
                    path: "memory/other.md".to_string(),
                    snapshot_id: "s2".to_string(),
                    start_line: 1,
                    end_line: 4,
                    text: "fallback recovery procedure".to_string(),
                },
                title: "Other".to_string(),
                lexical_score: 0.8,
                vector_score: 0.2,
                retrieval_score: 0.7,
                final_score: 0.7,
                rerank_score: None,
                matched_streams: 1,
                applied_mmr: false,
            },
        ];
        let config = MemoryEmbedConfig {
            hybrid: crate::HybridWeights {
                mmr_lambda: Some(0.65),
                mmr_pool_k: 3,
                ..crate::HybridWeights::default()
            },
            ..MemoryEmbedConfig::default()
        };

        let (selected, used) = maybe_apply_mmr(&ranked, &cached, 2, &config);

        assert!(used);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].chunk_id, "a");
        assert_eq!(selected[1].chunk_id, "c");
        assert!(selected.iter().all(|candidate| candidate.applied_mmr));
    }

    #[test]
    fn qmd_embedding_prompt_formats_match_model_family() {
        assert_eq!(
            format_query_embedding_input("embeddinggemma-300m", "redis failover"),
            "task: search result | query: redis failover"
        );
        assert_eq!(
            format_document_embedding_input(
                "embeddinggemma-300m",
                "Memory",
                "semantic recall target"
            ),
            "title: Memory | text: semantic recall target"
        );
        assert!(
            format_query_embedding_input("Qwen3-Embedding-0.6B", "redis failover")
                .starts_with("Instruct: Given a search query")
        );
    }

    #[test]
    fn rerank_blending_preserves_retrieval_priority_near_top() {
        assert_eq!(rerank_blend_weights(0), (0.75, 0.25));
        assert_eq!(rerank_blend_weights(5), (0.60, 0.40));
        assert_eq!(rerank_blend_weights(12), (0.40, 0.60));
    }
}
