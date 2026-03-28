use crate::lexical_index::{self, LexicalIndex, LexicalIndexChunk};
use crate::managed_files::record_memory;
use crate::promotion::promote_memory;
use crate::retention::forget_memory;
use crate::retrieval_policy;
use crate::vector_store::{CachedMemoryEmbedIndex, MemoryVectorStore, PersistedChunkEmbedding};
use crate::{
    MemoryBackend, MemoryDocument, MemoryEmbedConfig, MemoryForgetRequest, MemoryGetRequest,
    MemoryListRequest, MemoryListResponse, MemoryMutationResponse, MemoryPromoteRequest,
    MemoryRecordRequest, MemorySearchHit, MemorySearchRequest, MemorySearchResponse,
    MemorySidecarLifecycle, MemorySidecarStatus, MemoryStateLayout, MemorySyncStatus, Result,
    chunk_corpus, load_configured_memory_corpus, load_configured_memory_corpus_read_only,
};
use async_trait::async_trait;
use inference::{
    EmbeddingClient, ExpandedQueryKind, HttpEmbeddingClient, HttpQueryExpansionClient,
    HttpRerankClient, QueryExpansionClient, RerankClient, RerankDocument,
};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use store::RunStore;
use wide::f32x8;

const INDEX_SCHEMA_VERSION: u32 = 2;
const INDEX_BACKEND_ID: &str = "memory-embed";
const LEXICAL_INDEX_BACKEND_ID: &str = "memory-embed-lexical";
const LEXICAL_INDEX_SCHEMA_VERSION: u32 = 1;
const MEMORY_EMBED_LEXICAL_SQLITE_INDEX_RELATIVE: &str =
    ".nanoclaw/memory/indexes/memory-embed-lexical.sqlite";

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

#[derive(Clone, Debug)]
struct CandidateAccumulator {
    chunk_id: String,
    chunk: crate::MemoryCorpusChunk,
    title: String,
    scope_weight: f64,
    recency_multiplier: f64,
    run_match_bonus: f64,
    session_match_bonus: f64,
    agent_match_bonus: f64,
    task_match_bonus: f64,
    stale_penalty: f64,
    lexical_score: f64,
    vector_score: f64,
    base_retrieval_score: f64,
    retrieval_score: f64,
    final_score: f64,
    rerank_score: Option<f64>,
    rerank_relevant: Option<bool>,
    matched_streams: usize,
    applied_mmr: bool,
}

pub struct MemoryEmbedBackend {
    workspace_root: PathBuf,
    config: MemoryEmbedConfig,
    client: Arc<dyn EmbeddingClient>,
    query_expander: Option<Arc<dyn QueryExpansionClient>>,
    reranker: Option<Arc<dyn RerankClient>>,
    run_store: Option<Arc<dyn RunStore>>,
    vector_store: MemoryVectorStore,
    lexical_index: LexicalIndex,
    state: RwLock<Option<CachedMemoryEmbedIndex>>,
}

impl MemoryEmbedBackend {
    pub fn new(
        workspace_root: PathBuf,
        config: MemoryEmbedConfig,
        client: Arc<dyn EmbeddingClient>,
    ) -> Result<Self> {
        let vector_store = MemoryVectorStore::from_config(
            &MemoryStateLayout::new(&workspace_root),
            &config.vector_store,
        )?;
        Ok(Self {
            workspace_root: workspace_root.clone(),
            config,
            client,
            query_expander: None,
            reranker: None,
            run_store: None,
            vector_store,
            lexical_index: LexicalIndex::new(
                &workspace_root,
                LEXICAL_INDEX_BACKEND_ID,
                MEMORY_EMBED_LEXICAL_SQLITE_INDEX_RELATIVE,
                LEXICAL_INDEX_SCHEMA_VERSION,
            ),
            state: RwLock::new(None),
        })
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

    #[must_use]
    pub fn with_run_store(mut self, run_store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(run_store);
        self
    }

    fn core_delegate(&self) -> crate::MemoryCoreBackend {
        let backend = crate::MemoryCoreBackend::new(
            self.workspace_root.clone(),
            self.config.as_core_config(),
        );
        if let Some(run_store) = self.run_store.as_ref() {
            backend.with_run_store(run_store.clone())
        } else {
            backend
        }
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
            .transpose()
            .map_err(map_inference_error)?
            .map(|client| Arc::new(client) as Arc<dyn QueryExpansionClient>);
        let reranker = config
            .rerank
            .as_ref()
            .map(HttpRerankClient::from_config)
            .transpose()
            .map_err(map_inference_error)?
            .map(|client| Arc::new(client) as Arc<dyn RerankClient>);
        Ok(Self::new(
            workspace_root,
            config,
            Arc::new(HttpEmbeddingClient::from_config(&embedding).map_err(map_inference_error)?),
        )?
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
        let titles = document_titles(corpus);
        let mut upserted_chunks = if fingerprint_changed {
            BTreeMap::new()
        } else {
            self.reuse_stale_chunk_embeddings(chunks, &titles, &cached)
        };
        cached.chunks.extend(upserted_chunks.clone());
        let snapshots_changed = cached.document_snapshots != current_snapshots;
        let mut changed = fingerprint_changed || snapshots_changed;
        let removed_chunk_ids = if fingerprint_changed {
            BTreeSet::new()
        } else {
            self.trim_cached_chunks(chunks, &mut cached)
        };
        changed |= !removed_chunk_ids.is_empty();
        let missing = chunks
            .iter()
            .filter(|chunk| !cached.chunks.contains_key(&chunk_id(chunk)))
            .cloned()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            for entry in self.embed_missing_chunks(&missing, &titles).await? {
                upserted_chunks.insert(entry.chunk_id.clone(), entry.clone());
                cached.chunks.insert(entry.chunk_id.clone(), entry);
            }
            changed = true;
        }
        changed |= !upserted_chunks.is_empty();
        cached.config_fingerprint = current_fingerprint;
        cached.document_snapshots = current_snapshots;
        if changed {
            if fingerprint_changed {
                self.persist_index(&cached).await?;
            } else {
                self.persist_incremental(&cached, &upserted_chunks, &removed_chunk_ids)
                    .await?;
            }
        } else {
            self.write_lifecycle(&cached, MemorySidecarStatus::Ready)?;
        }

        *self.state.write().expect("memory-embed write lock") = Some(cached.clone());
        Ok(cached)
    }

    async fn ensure_lexical_index(
        &self,
        corpus: &crate::MemoryCorpus,
        chunks: &[crate::MemoryCorpusChunk],
        exported_run_count: usize,
    ) -> Result<()> {
        self.lexical_index
            .ensure_ready(
                &lexical_index::config_fingerprint(&json!({
                    "corpus": &self.config.corpus,
                    "chunking": &self.config.chunking,
                }))?,
                &document_snapshots(corpus),
                &lexical_index_chunks(chunks),
                exported_run_count,
            )
            .await?;
        Ok(())
    }

    async fn load_persisted_index(&self) -> Result<CachedMemoryEmbedIndex> {
        let Some(persisted) = self.state_layout().load_lifecycle(INDEX_BACKEND_ID)? else {
            return Ok(CachedMemoryEmbedIndex::default());
        };
        if persisted.backend != INDEX_BACKEND_ID
            || persisted.status != MemorySidecarStatus::Ready
            || persisted.schema_version != INDEX_SCHEMA_VERSION
            || persisted.vector_store != self.vector_store.kind().as_str()
        {
            return Ok(CachedMemoryEmbedIndex::default());
        }
        Ok(CachedMemoryEmbedIndex {
            config_fingerprint: persisted.config_fingerprint,
            document_snapshots: persisted.document_snapshots,
            chunks: self.vector_store.load_chunks().await?,
        })
    }

    fn trim_cached_chunks(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
        cached: &mut CachedMemoryEmbedIndex,
    ) -> BTreeSet<String> {
        let valid_ids = chunks.iter().map(chunk_id).collect::<HashSet<_>>();
        let removed = cached
            .chunks
            .keys()
            .filter(|chunk_id| !valid_ids.contains(chunk_id.as_str()))
            .cloned()
            .collect::<BTreeSet<_>>();
        cached
            .chunks
            .retain(|chunk_id, _| valid_ids.contains(chunk_id.as_str()));
        removed
    }

    fn reuse_stale_chunk_embeddings(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
        titles: &BTreeMap<String, String>,
        cached: &CachedMemoryEmbedIndex,
    ) -> BTreeMap<String, PersistedChunkEmbedding> {
        let valid_ids = chunks.iter().map(chunk_id).collect::<HashSet<_>>();
        let reusable = cached
            .chunks
            .values()
            .filter(|entry| !valid_ids.contains(entry.chunk_id.as_str()))
            .fold(
                BTreeMap::<String, PersistedChunkEmbedding>::new(),
                |mut pool, entry| {
                    pool.entry(embedding_input_signature(
                        self.embedding_model(),
                        titles
                            .get(&entry.path)
                            .map(String::as_str)
                            .unwrap_or("Memory"),
                        &entry.text,
                    ))
                    .or_insert_with(|| entry.clone());
                    pool
                },
            );

        chunks
            .iter()
            .filter_map(|chunk| {
                let current_id = chunk_id(chunk);
                if cached.chunks.contains_key(&current_id) {
                    return None;
                }
                let signature = embedding_input_signature(
                    self.embedding_model(),
                    titles
                        .get(&chunk.path)
                        .map(String::as_str)
                        .unwrap_or("Memory"),
                    &chunk.text,
                );
                reusable.get(&signature).map(|entry| {
                    (
                        current_id.clone(),
                        PersistedChunkEmbedding {
                            chunk_id: current_id,
                            path: chunk.path.clone(),
                            snapshot_id: chunk.snapshot_id.clone(),
                            start_line: chunk.start_line,
                            end_line: chunk.end_line,
                            text: chunk.text.clone(),
                            embedding: entry.embedding.clone(),
                        },
                    )
                })
            })
            .collect()
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
        let mut payload_chunks = BTreeMap::<String, Vec<&crate::MemoryCorpusChunk>>::new();
        let mut payloads = BTreeMap::<String, String>::new();
        for chunk in missing {
            let payload = format_document_embedding_input(
                model,
                titles
                    .get(&chunk.path)
                    .map(String::as_str)
                    .unwrap_or("Memory"),
                &chunk.text,
            );
            let signature = embedding_input_signature(
                model,
                titles
                    .get(&chunk.path)
                    .map(String::as_str)
                    .unwrap_or("Memory"),
                &chunk.text,
            );
            payload_chunks
                .entry(signature.clone())
                .or_default()
                .push(chunk);
            payloads.entry(signature).or_insert(payload);
        }
        let unique_payloads = payloads.into_iter().collect::<Vec<_>>();
        let mut entries = Vec::with_capacity(missing.len());
        for group in unique_payloads.chunks(batch_size) {
            let texts = group
                .iter()
                .map(|(_, payload)| payload.clone())
                .collect::<Vec<_>>();
            let embeddings = self
                .client
                .embed(model, &texts)
                .await
                .map_err(map_inference_error)?;
            if embeddings.len() != group.len() {
                return Err(crate::MemoryError::invalid(format!(
                    "embedding service returned {} vectors for {} unique chunks",
                    embeddings.len(),
                    group.len()
                )));
            }
            for ((signature, _), embedding) in group.iter().zip(embeddings.into_iter()) {
                for chunk in payload_chunks.get(signature).into_iter().flatten() {
                    entries.push(PersistedChunkEmbedding {
                        chunk_id: chunk_id(chunk),
                        path: chunk.path.clone(),
                        snapshot_id: chunk.snapshot_id.clone(),
                        start_line: chunk.start_line,
                        end_line: chunk.end_line,
                        text: chunk.text.clone(),
                        embedding: embedding.clone(),
                    });
                }
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

    async fn persist_index(&self, cached: &CachedMemoryEmbedIndex) -> Result<()> {
        self.write_lifecycle(cached, MemorySidecarStatus::Rebuilding)?;
        self.vector_store.replace_chunks(&cached.chunks).await?;
        self.write_lifecycle(cached, MemorySidecarStatus::Ready)
    }

    async fn persist_incremental(
        &self,
        cached: &CachedMemoryEmbedIndex,
        inserted_chunks: &BTreeMap<String, PersistedChunkEmbedding>,
        removed_chunk_ids: &BTreeSet<String>,
    ) -> Result<()> {
        self.write_lifecycle(cached, MemorySidecarStatus::Rebuilding)?;
        if !removed_chunk_ids.is_empty() {
            self.vector_store.delete_chunks(removed_chunk_ids).await?;
        }
        if !inserted_chunks.is_empty() {
            self.vector_store.upsert_chunks(inserted_chunks).await?;
        }
        self.write_lifecycle(cached, MemorySidecarStatus::Ready)
    }

    fn state_layout(&self) -> MemoryStateLayout {
        MemoryStateLayout::new(&self.workspace_root)
    }

    fn write_lifecycle(
        &self,
        cached: &CachedMemoryEmbedIndex,
        status: MemorySidecarStatus,
    ) -> Result<()> {
        self.state_layout().write_lifecycle(
            INDEX_BACKEND_ID,
            MemorySidecarLifecycle {
                backend: INDEX_BACKEND_ID.to_string(),
                status,
                vector_store: self.vector_store.kind().as_str().to_string(),
                schema_version: INDEX_SCHEMA_VERSION,
                config_fingerprint: cached.config_fingerprint.clone(),
                indexed_chunk_count: cached.chunks.len(),
                indexed_document_count: cached.document_snapshots.len(),
                artifact_path: self.vector_store.artifact_path().relative_display(),
                document_snapshots: cached.document_snapshots.clone(),
                ..MemorySidecarLifecycle::default()
            },
        )?;
        Ok(())
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
            .await
            .map_err(map_inference_error);
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
        let embeddings = self
            .client
            .embed(model, &payloads)
            .await
            .map_err(map_inference_error)?;
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
            .map_err(map_inference_error)
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
            // Rerank is the only stage that can explicitly veto a high-retrieval
            // false positive. Encode it as a signed signal so "irrelevant"
            // judgments actively demote instead of merely withholding a bonus.
            let rerank_signal = if judgment.relevant {
                judgment.confidence.clamp(0.0, 1.0)
            } else {
                -judgment.confidence.clamp(0.0, 1.0)
            };
            candidate.rerank_relevant = Some(judgment.relevant);
            candidate.rerank_score = Some(rerank_signal);
            candidate.final_score =
                (normalized_retrieval * retrieval_weight) + (rerank_signal * rerank_weight);
        }
        Ok((rerank_pool, true, false))
    }
}

#[async_trait]
impl MemoryBackend for MemoryEmbedBackend {
    async fn sync(&self) -> Result<MemorySyncStatus> {
        let (corpus, runtime_exports) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        self.ensure_lexical_index(&corpus, &chunks, runtime_exports.exported_runs)
            .await?;
        self.ensure_chunk_index(&corpus, &chunks).await?;
        Ok(MemorySyncStatus {
            backend: INDEX_BACKEND_ID.to_string(),
            indexed_documents: corpus.documents.len(),
            indexed_lines: corpus.total_lines(),
        })
    }

    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse> {
        let (corpus, runtime_exports) =
            load_configured_memory_corpus_read_only(&self.workspace_root, &self.config.corpus)
                .await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        let limit = req
            .limit
            .unwrap_or(self.config.search.max_results)
            .max(1)
            .min(50);
        let prefix = req
            .path_prefix
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        self.ensure_lexical_index(&corpus, &chunks, runtime_exports.exported_runs)
            .await?;
        let cached = self.ensure_chunk_index(&corpus, &chunks).await?;
        let titles = document_titles(&corpus);
        let filtered_chunks = chunks
            .iter()
            .filter(|chunk| {
                prefix
                    .as_deref()
                    .is_none_or(|path_prefix| chunk.path.starts_with(path_prefix))
                    && retrieval_policy::matches_search_filters(&chunk.path, &chunk.metadata, &req)
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
        let mut vector_store_native_used = false;
        let mut vector_store_native_fallback = false;

        for weighted_query in &weighted_queries {
            if matches!(
                weighted_query.kind,
                ExpandedQueryKind::Hybrid | ExpandedQueryKind::Lex
            ) {
                let lexical_ranked = self
                    .lexical_index
                    .search_ranked(&weighted_query.text, prefix.as_deref(), candidate_limit)
                    .await?
                    .into_iter()
                    .map(|entry| (entry.chunk_id, entry.score))
                    .collect::<Vec<_>>();
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
                    let vector_ranked = match self
                        .vector_store
                        .search(query_vector, prefix.as_deref(), candidate_limit)
                        .await
                    {
                        Ok(Some(ranked)) => {
                            vector_store_native_used = true;
                            ranked
                        }
                        Ok(None) => ranked_vector_list(
                            &filtered_chunks,
                            &cached,
                            query_vector,
                            candidate_limit,
                        ),
                        Err(_) => {
                            vector_store_native_fallback = true;
                            ranked_vector_list(
                                &filtered_chunks,
                                &cached,
                                query_vector,
                                candidate_limit,
                            )
                        }
                    };
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
        apply_temporal_scoring(&mut ranked, &req);
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
                    "base_retrieval_score".to_string(),
                    json!(candidate.base_retrieval_score),
                );
                metadata.insert(
                    "retrieval_score".to_string(),
                    json!(candidate.retrieval_score),
                );
                metadata.insert(
                    "memory_scope".to_string(),
                    json!(candidate.chunk.metadata.scope.as_str()),
                );
                metadata.insert(
                    "memory_layer".to_string(),
                    json!(candidate.chunk.metadata.layer),
                );
                metadata.insert(
                    "memory_status".to_string(),
                    json!(candidate.chunk.metadata.status.as_str()),
                );
                metadata.insert("scope_weight".to_string(), json!(candidate.scope_weight));
                metadata.insert(
                    "recency_multiplier".to_string(),
                    json!(candidate.recency_multiplier),
                );
                metadata.insert(
                    "run_match_bonus".to_string(),
                    json!(candidate.run_match_bonus),
                );
                metadata.insert(
                    "session_match_bonus".to_string(),
                    json!(candidate.session_match_bonus),
                );
                metadata.insert(
                    "agent_match_bonus".to_string(),
                    json!(candidate.agent_match_bonus),
                );
                metadata.insert(
                    "task_match_bonus".to_string(),
                    json!(candidate.task_match_bonus),
                );
                metadata.insert("stale_penalty".to_string(), json!(candidate.stale_penalty));
                if let Some(updated_at_ms) = candidate.chunk.metadata.updated_at_ms {
                    metadata.insert("updated_at_ms".to_string(), json!(updated_at_ms));
                }
                metadata.insert(
                    "matched_streams".to_string(),
                    json!(candidate.matched_streams),
                );
                metadata.insert("mmr_applied".to_string(), json!(candidate.applied_mmr));
                if let Some(rerank_score) = candidate.rerank_score {
                    metadata.insert("rerank_score".to_string(), json!(rerank_score));
                }
                if let Some(rerank_relevant) = candidate.rerank_relevant {
                    metadata.insert("rerank_relevant".to_string(), json!(rerank_relevant));
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
                    document_metadata: candidate.chunk.metadata.clone(),
                    metadata,
                }
            })
            .collect::<Vec<_>>();

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "indexed_documents".to_string(),
            json!(corpus.documents.len()),
        );
        metadata.insert(
            "runtime_exported_runs".to_string(),
            json!(runtime_exports.exported_runs),
        );
        metadata.insert(
            "runtime_exported_documents".to_string(),
            json!(runtime_exports.exported_documents),
        );
        if let Some(output_dir) = runtime_exports.output_dir {
            metadata.insert("runtime_export_dir".to_string(), json!(output_dir));
        }
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
            "vector_store_path".to_string(),
            json!(self.vector_store.artifact_path().relative_display()),
        );
        metadata.insert(
            "vector_store_kind".to_string(),
            json!(self.vector_store.kind().as_str()),
        );
        metadata.insert(
            "vector_store_native_used".to_string(),
            json!(vector_store_native_used),
        );
        metadata.insert(
            "vector_store_native_fallback".to_string(),
            json!(vector_store_native_fallback),
        );
        metadata.insert(
            "lexical_index_path".to_string(),
            json!(self.lexical_index.artifact_path()?.relative_display()),
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
        self.core_delegate().get(req).await
    }

    async fn list(&self, req: MemoryListRequest) -> Result<MemoryListResponse> {
        self.core_delegate().list(req).await
    }

    async fn record(&self, req: MemoryRecordRequest) -> Result<MemoryMutationResponse> {
        record_memory(&self.workspace_root, req, None, None).await
    }

    async fn promote(&self, req: MemoryPromoteRequest) -> Result<MemoryMutationResponse> {
        promote_memory(&self.workspace_root, req).await
    }

    async fn forget(&self, req: MemoryForgetRequest) -> Result<MemoryMutationResponse> {
        forget_memory(&self.workspace_root, req).await
    }
}

fn normalize_query(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn map_inference_error(error: inference::InferenceError) -> crate::MemoryError {
    crate::MemoryError::invalid(error.to_string())
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

fn lexical_index_chunks(chunks: &[crate::MemoryCorpusChunk]) -> Vec<LexicalIndexChunk> {
    chunks
        .iter()
        .map(|chunk| LexicalIndexChunk {
            chunk_id: chunk_id(chunk),
            path: chunk.path.clone(),
            snapshot_id: chunk.snapshot_id.clone(),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            text: chunk.text.clone(),
        })
        .collect()
}

fn embedding_input_signature(model: &str, title: &str, text: &str) -> String {
    stable_digest(format_document_embedding_input(model, title, text).as_bytes())
}

fn ranked_vector_list(
    chunks: &[crate::MemoryCorpusChunk],
    cached: &CachedMemoryEmbedIndex,
    query_vector: &[f32],
    limit: usize,
) -> Vec<(String, f64)> {
    // Vector scoring is the dominant CPU path in hybrid retrieval. We parallelize
    // chunk scoring so large corpora can saturate available cores.
    let mut ranked = chunks
        .par_iter()
        .filter_map(|chunk| {
            let id = chunk_id(chunk);
            let score = cached
                .chunks
                .get(&id)
                .map(|entry| cosine_similarity(query_vector, &entry.embedding))
                .unwrap_or(0.0);
            (score > 0.0).then_some((id, score))
        })
        .collect::<Vec<_>>();
    ranked.par_sort_unstable_by(compare_ranked_score);
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
                base_retrieval_score: 0.0,
                retrieval_score: 0.0,
                final_score: 0.0,
                rerank_score: None,
                rerank_relevant: None,
                matched_streams: 0,
                applied_mmr: false,
                scope_weight: 1.0,
                recency_multiplier: 1.0,
                run_match_bonus: 0.0,
                session_match_bonus: 0.0,
                agent_match_bonus: 0.0,
                task_match_bonus: 0.0,
                stale_penalty: 1.0,
            });
        match stream_kind {
            RankedStreamKind::Lexical => entry.lexical_score = entry.lexical_score.max(score),
            RankedStreamKind::Vector => entry.vector_score = entry.vector_score.max(score),
        }
        entry.matched_streams += 1;
        entry.base_retrieval_score +=
            (weighted_query.weight * stream_weight) / (config.hybrid.rrf_k + rank + 1) as f64;
        if weighted_query.is_original {
            if rank == 0 {
                entry.base_retrieval_score += config.hybrid.top_rank_bonus_first;
            } else if rank < 3 {
                entry.base_retrieval_score += config.hybrid.top_rank_bonus_other;
            }
        }
    }
}

fn apply_temporal_scoring(candidates: &mut [CandidateAccumulator], request: &MemorySearchRequest) {
    for candidate in candidates {
        let signals = retrieval_policy::search_signals(
            &candidate.chunk.path,
            &candidate.chunk.metadata,
            request,
        );
        candidate.scope_weight = signals.scope_weight;
        candidate.recency_multiplier = signals.recency_multiplier;
        candidate.run_match_bonus = signals.run_match_bonus;
        candidate.session_match_bonus = signals.session_match_bonus;
        candidate.agent_match_bonus = signals.agent_match_bonus;
        candidate.task_match_bonus = signals.task_match_bonus;
        candidate.stale_penalty = signals.stale_penalty;
        candidate.retrieval_score = candidate.base_retrieval_score * signals.total_multiplier();
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
        .rerank_relevant
        .unwrap_or(false)
        .cmp(&left.rerank_relevant.unwrap_or(false))
        .then_with(|| {
            right
                .final_score
                .partial_cmp(&left.final_score)
                .unwrap_or(Ordering::Equal)
        })
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
    const LANES: usize = 8;
    let mut dot = f32x8::from([0.0; LANES]);
    let mut left_norm = f32x8::from([0.0; LANES]);
    let mut right_norm = f32x8::from([0.0; LANES]);
    let mut index = 0usize;

    while index + LANES <= left.len() {
        let lhs = f32x8::from([
            left[index],
            left[index + 1],
            left[index + 2],
            left[index + 3],
            left[index + 4],
            left[index + 5],
            left[index + 6],
            left[index + 7],
        ]);
        let rhs = f32x8::from([
            right[index],
            right[index + 1],
            right[index + 2],
            right[index + 3],
            right[index + 4],
            right[index + 5],
            right[index + 6],
            right[index + 7],
        ]);
        dot += lhs * rhs;
        left_norm += lhs * lhs;
        right_norm += rhs * rhs;
        index += LANES;
    }
    let mut dot_sum = f64::from(dot.reduce_add());
    let mut left_norm_sum = f64::from(left_norm.reduce_add());
    let mut right_norm_sum = f64::from(right_norm.reduce_add());
    for (lhs, rhs) in left[index..].iter().zip(&right[index..]) {
        dot_sum += f64::from(*lhs) * f64::from(*rhs);
        left_norm_sum += f64::from(*lhs) * f64::from(*lhs);
        right_norm_sum += f64::from(*rhs) * f64::from(*rhs);
    }
    let denom = left_norm_sum.sqrt() * right_norm_sum.sqrt();
    if denom <= f64::EPSILON {
        0.0
    } else {
        dot_sum / denom
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
        CachedMemoryEmbedIndex, CandidateAccumulator, MemoryEmbedBackend,
        format_document_embedding_input, format_query_embedding_input, maybe_apply_mmr,
        rerank_blend_weights,
    };
    use crate::vector_store::PersistedChunkEmbedding as MemoryVectorChunkRecord;
    use crate::{
        MemoryBackend, MemoryChunkingConfig, MemoryEmbedConfig, MemorySearchRequest,
        MemorySidecarStatus, MemoryStateLayout,
    };
    use async_trait::async_trait;
    use inference::{
        EmbeddingClient, EmbeddingConfig, ExpandedQuery, ExpandedQueryKind, LlmServiceConfig,
        QueryExpansionClient, QueryExpansionConfig, RerankClient, RerankConfig, RerankDocument,
        RerankJudgment, parse_expanded_queries,
    };
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use time::{Duration, OffsetDateTime};
    use tokio::fs;

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    #[derive(Default)]
    struct MockEmbeddingClient {
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait]
    impl EmbeddingClient for MockEmbeddingClient {
        async fn embed(&self, _model: &str, texts: &[String]) -> inference::Result<Vec<Vec<f32>>> {
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

    struct ZeroEmbeddingClient;

    #[async_trait]
    impl EmbeddingClient for ZeroEmbeddingClient {
        async fn embed(&self, _model: &str, texts: &[String]) -> inference::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0, 0.0]).collect())
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
        ) -> inference::Result<Vec<ExpandedQuery>> {
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
        ) -> inference::Result<Vec<RerankJudgment>> {
            Ok(self.judgments.clone())
        }
    }

    bounded_async_test!(
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
            )
            .unwrap();
            let response = backend
                .search(MemorySearchRequest {
                    query: "query".to_string(),
                    limit: Some(2),
                    path_prefix: None,
                    scopes: None,
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: None,
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
    );

    bounded_async_test!(
        async fn sync_reuses_persisted_chunk_embeddings() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "semantic recall target")
                .await
                .unwrap();
            let client = Arc::new(MockEmbeddingClient::default());
            let config = MemoryEmbedConfig::default();

            let backend =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config.clone(), client.clone())
                    .unwrap();
            backend.sync().await.unwrap();
            let first_calls = client.calls.lock().unwrap().clone();
            assert_eq!(first_calls.len(), 1);
            assert_eq!(first_calls[0].len(), 1);

            let backend =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone()).unwrap();
            backend.sync().await.unwrap();
            let second_calls = client.calls.lock().unwrap().clone();
            assert_eq!(second_calls.len(), 1);
        }
    );

    bounded_async_test!(
        async fn sync_writes_ready_lifecycle_manifest() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "semantic recall target")
                .await
                .unwrap();
            let backend = MemoryEmbedBackend::new(
                dir.path().to_path_buf(),
                MemoryEmbedConfig::default(),
                Arc::new(MockEmbeddingClient::default()),
            )
            .unwrap();

            backend.sync().await.unwrap();

            let lifecycle = MemoryStateLayout::new(dir.path())
                .load_lifecycle("memory-embed")
                .unwrap()
                .unwrap();
            assert_eq!(lifecycle.status, MemorySidecarStatus::Ready);
            assert_eq!(lifecycle.vector_store, "sqlite");
            assert_eq!(
                lifecycle.artifact_path,
                ".nanoclaw/memory/indexes/memory-embed.sqlite"
            );
            assert_eq!(lifecycle.indexed_document_count, 1);
        }
    );

    bounded_async_test!(
        async fn search_uses_sqlite_lexical_sidecar_for_exact_matches() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "semantic recall target")
                .await
                .unwrap();
            fs::create_dir_all(dir.path().join("memory")).await.unwrap();
            fs::write(
                dir.path().join("memory/today.md"),
                "browserless exact token in lexical sidecar",
            )
            .await
            .unwrap();

            let backend = MemoryEmbedBackend::new(
                dir.path().to_path_buf(),
                MemoryEmbedConfig::default(),
                Arc::new(ZeroEmbeddingClient),
            )
            .unwrap();
            let response = backend
                .search(MemorySearchRequest {
                    query: "browserless".to_string(),
                    limit: Some(2),
                    path_prefix: None,
                    scopes: None,
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: None,
                })
                .await
                .unwrap();

            assert_eq!(response.hits[0].path, "memory/today.md");
            assert_eq!(
                response
                    .metadata
                    .get("lexical_index_path")
                    .and_then(Value::as_str),
                Some(".nanoclaw/memory/indexes/memory-embed-lexical.sqlite")
            );

            let lifecycle = MemoryStateLayout::new(dir.path())
                .load_lifecycle("memory-embed-lexical")
                .unwrap()
                .unwrap();
            assert_eq!(lifecycle.status, MemorySidecarStatus::Ready);
            assert_eq!(
                lifecycle.artifact_path,
                ".nanoclaw/memory/indexes/memory-embed-lexical.sqlite"
            );
            assert_eq!(lifecycle.indexed_document_count, 2);
        }
    );

    bounded_async_test!(
        async fn sync_after_content_change_only_embeds_new_chunks() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "semantic alpha")
                .await
                .unwrap();
            let client = Arc::new(MockEmbeddingClient::default());
            let config = MemoryEmbedConfig::default();

            let backend =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config.clone(), client.clone())
                    .unwrap();
            backend.sync().await.unwrap();
            assert_eq!(client.calls.lock().unwrap().len(), 1);

            fs::create_dir_all(dir.path().join("memory")).await.unwrap();
            fs::write(dir.path().join("memory/new.md"), "semantic beta")
                .await
                .unwrap();

            let restarted =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone()).unwrap();
            restarted.sync().await.unwrap();
            assert_eq!(client.calls.lock().unwrap().len(), 2);
        }
    );

    bounded_async_test!(
        async fn sync_reuses_matching_chunk_embeddings_when_document_snapshot_changes() {
            let dir = tempdir().unwrap();
            fs::write(
                dir.path().join("MEMORY.md"),
                [
                    "semantic line one",
                    "semantic line two",
                    "semantic line three",
                    "semantic line four",
                ]
                .join("\n"),
            )
            .await
            .unwrap();
            let client = Arc::new(MockEmbeddingClient::default());
            let mut config = MemoryEmbedConfig::default();
            config.chunking = MemoryChunkingConfig {
                target_tokens: 8,
                overlap_tokens: 1,
            };

            let backend =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config.clone(), client.clone())
                    .unwrap();
            backend.sync().await.unwrap();
            let first_calls = client.calls.lock().unwrap().clone();
            assert_eq!(first_calls.len(), 1);
            assert_eq!(first_calls[0].len(), 4);

            fs::write(
                dir.path().join("MEMORY.md"),
                [
                    "semantic line one",
                    "semantic line two",
                    "semantic line three changed",
                    "semantic line four changed",
                ]
                .join("\n"),
            )
            .await
            .unwrap();

            let restarted =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone()).unwrap();
            restarted.sync().await.unwrap();
            let second_calls = client.calls.lock().unwrap().clone();
            assert_eq!(second_calls.len(), 2);
            assert_eq!(second_calls[1].len(), 2);
        }
    );

    bounded_async_test!(
        async fn sync_deduplicates_identical_embedding_payloads() {
            let dir = tempdir().unwrap();
            fs::create_dir_all(dir.path().join("memory")).await.unwrap();
            fs::write(dir.path().join("memory/first.md"), "semantic duplicate")
                .await
                .unwrap();
            fs::write(dir.path().join("memory/second.md"), "semantic duplicate")
                .await
                .unwrap();
            let client = Arc::new(MockEmbeddingClient::default());

            let backend = MemoryEmbedBackend::new(
                dir.path().to_path_buf(),
                MemoryEmbedConfig::default(),
                client.clone(),
            )
            .unwrap();
            backend.sync().await.unwrap();

            let calls = client.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0], vec!["semantic duplicate".to_string()]);
        }
    );

    bounded_async_test!(
        async fn search_prefers_recent_daily_logs_after_temporal_decay() {
            let dir = tempdir().unwrap();
            let today = OffsetDateTime::now_utc().date();
            let stale = today - Duration::days(120);
            fs::create_dir_all(dir.path().join("memory")).await.unwrap();
            fs::write(
                dir.path().join("memory").join(format!("{stale}.md")),
                "browserless rollout recap",
            )
            .await
            .unwrap();
            fs::write(
                dir.path().join("memory").join(format!("{today}.md")),
                "browserless rollout recap",
            )
            .await
            .unwrap();

            let backend = MemoryEmbedBackend::new(
                dir.path().to_path_buf(),
                MemoryEmbedConfig::default(),
                Arc::new(ZeroEmbeddingClient),
            )
            .unwrap();
            let response = backend
                .search(MemorySearchRequest {
                    query: "browserless".to_string(),
                    limit: Some(2),
                    path_prefix: Some("memory/".to_string()),
                    scopes: None,
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: None,
                })
                .await
                .unwrap();

            assert_eq!(response.hits[0].path, format!("memory/{today}.md"));
            assert_eq!(
                response.hits[0]
                    .metadata
                    .get("memory_layer")
                    .and_then(Value::as_str),
                Some("daily-log")
            );
            assert!(
                response.hits[0].score > response.hits[1].score,
                "recent daily log should outrank stale daily log after decay"
            );
        }
    );

    bounded_async_test!(
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

            let backend =
                MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone()).unwrap();
            backend.sync().await.unwrap();
            let calls = client.calls.lock().unwrap().clone();
            assert_eq!(
                calls.iter().map(Vec::len).collect::<Vec<_>>(),
                vec![2, 2, 1]
            );
        }
    );

    bounded_async_test!(
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
                .unwrap()
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
                    scopes: None,
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: None,
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
    );

    #[test]
    fn typed_query_lines_parse_into_qmd_query_kinds() {
        let parsed = parse_expanded_queries(
            "lex: authentication config\nvec: how do I configure authentication\nhyde: Authentication is configured with AUTH_SECRET",
        )
        .unwrap();
        let lex = &parsed[0];
        let vec = &parsed[1];
        let hyde = &parsed[2];

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
                    MemoryVectorChunkRecord {
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
                    MemoryVectorChunkRecord {
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
                    MemoryVectorChunkRecord {
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
                    metadata: crate::MemoryDocumentMetadata::default(),
                },
                title: "Memory".to_string(),
                scope_weight: 1.0,
                recency_multiplier: 1.0,
                run_match_bonus: 0.0,
                session_match_bonus: 0.0,
                agent_match_bonus: 0.0,
                task_match_bonus: 0.0,
                stale_penalty: 1.0,
                lexical_score: 2.0,
                vector_score: 1.0,
                retrieval_score: 0.9,
                final_score: 0.9,
                rerank_score: None,
                rerank_relevant: None,
                matched_streams: 2,
                applied_mmr: false,
                base_retrieval_score: 0.9,
            },
            CandidateAccumulator {
                chunk_id: "b".to_string(),
                chunk: crate::MemoryCorpusChunk {
                    path: "MEMORY.md".to_string(),
                    snapshot_id: "s1".to_string(),
                    start_line: 3,
                    end_line: 6,
                    text: "duplicate rollout canary".to_string(),
                    metadata: crate::MemoryDocumentMetadata::default(),
                },
                title: "Memory".to_string(),
                scope_weight: 1.0,
                recency_multiplier: 1.0,
                run_match_bonus: 0.0,
                session_match_bonus: 0.0,
                agent_match_bonus: 0.0,
                task_match_bonus: 0.0,
                stale_penalty: 1.0,
                lexical_score: 1.9,
                vector_score: 0.98,
                retrieval_score: 0.85,
                final_score: 0.85,
                rerank_score: None,
                rerank_relevant: None,
                matched_streams: 2,
                applied_mmr: false,
                base_retrieval_score: 0.85,
            },
            CandidateAccumulator {
                chunk_id: "c".to_string(),
                chunk: crate::MemoryCorpusChunk {
                    path: "memory/other.md".to_string(),
                    snapshot_id: "s2".to_string(),
                    start_line: 1,
                    end_line: 4,
                    text: "fallback recovery procedure".to_string(),
                    metadata: crate::MemoryDocumentMetadata::default(),
                },
                title: "Other".to_string(),
                scope_weight: 1.0,
                recency_multiplier: 1.0,
                run_match_bonus: 0.0,
                session_match_bonus: 0.0,
                agent_match_bonus: 0.0,
                task_match_bonus: 0.0,
                stale_penalty: 1.0,
                lexical_score: 0.8,
                vector_score: 0.2,
                retrieval_score: 0.7,
                final_score: 0.7,
                rerank_score: None,
                rerank_relevant: None,
                matched_streams: 1,
                applied_mmr: false,
                base_retrieval_score: 0.7,
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
