use crate::{
    EmbeddingConfig, MemoryBackend, MemoryDocument, MemoryEmbedConfig, MemoryGetRequest,
    MemorySearchHit, MemorySearchRequest, MemorySearchResponse, MemorySyncStatus, Result,
    chunk_corpus, lexical_score, load_memory_corpus,
};
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

#[async_trait]
pub trait EmbeddingClient: Send + Sync {
    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[derive(Clone)]
pub struct HttpEmbeddingClient {
    model: String,
    client: reqwest::Client,
    base_url: String,
}

impl HttpEmbeddingClient {
    pub fn from_config(config: &EmbeddingConfig) -> Result<Self> {
        let api_key = config.api_key.as_deref().ok_or_else(|| {
            crate::MemoryError::invalid("memory-embed requires embedding.api_key")
        })?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|error| crate::MemoryError::invalid(error.to_string()))?,
        );
        for (key, value) in &config.headers {
            headers.insert(
                HeaderName::from_bytes(key.as_bytes())
                    .map_err(|error| crate::MemoryError::invalid(error.to_string()))?,
                HeaderValue::from_str(value)
                    .map_err(|error| crate::MemoryError::invalid(error.to_string()))?,
            );
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.timeout_ms))
            .default_headers(headers)
            .build()
            .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;
        let base_url = config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        Ok(Self {
            model: config.model.clone(),
            client,
            base_url,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedChunkEmbedding {
    chunk_id: String,
    path: String,
    snapshot_id: String,
    start_line: usize,
    end_line: usize,
    embedding: Vec<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedMemoryEmbedIndex {
    backend: String,
    chunks: Vec<PersistedChunkEmbedding>,
}

#[derive(Clone, Debug, Default)]
struct CachedMemoryEmbedIndex {
    chunks: BTreeMap<String, PersistedChunkEmbedding>,
}

pub struct MemoryEmbedBackend {
    workspace_root: PathBuf,
    config: MemoryEmbedConfig,
    client: Arc<dyn EmbeddingClient>,
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
            state: RwLock::new(None),
        }
    }

    pub fn from_http_config(workspace_root: PathBuf, config: MemoryEmbedConfig) -> Result<Self> {
        let embedding = config
            .embedding
            .clone()
            .ok_or_else(|| crate::MemoryError::invalid("memory-embed requires embedding config"))?;
        Ok(Self::new(
            workspace_root,
            config,
            Arc::new(HttpEmbeddingClient::from_config(&embedding)?),
        ))
    }

    async fn ensure_chunk_index(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
    ) -> Result<CachedMemoryEmbedIndex> {
        // The persisted cache is keyed by chunk identity, not file path alone, so a restart can
        // reuse previous embeddings while content changes still invalidate the affected windows.
        let mut cached =
            if let Some(index) = self.state.read().expect("memory-embed read lock").clone() {
                index
            } else {
                self.load_persisted_index().await?
            };
        let missing = chunks
            .iter()
            .filter(|chunk| !cached.chunks.contains_key(&chunk_id(chunk)))
            .cloned()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            let texts = missing
                .iter()
                .map(|chunk| chunk.text.clone())
                .collect::<Vec<_>>();
            let embeddings = self
                .client
                .embed(
                    self.config
                        .embedding
                        .as_ref()
                        .map(|cfg| cfg.model.as_str())
                        .unwrap_or(""),
                    &texts,
                )
                .await?;
            for (chunk, embedding) in missing.iter().zip(embeddings.into_iter()) {
                let entry = PersistedChunkEmbedding {
                    chunk_id: chunk_id(chunk),
                    path: chunk.path.clone(),
                    snapshot_id: chunk.snapshot_id.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    embedding,
                };
                cached.chunks.insert(entry.chunk_id.clone(), entry);
            }
            self.persist_index(chunks, &cached).await?;
        } else {
            self.trim_cached_chunks(chunks, &mut cached).await?;
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
        Ok(CachedMemoryEmbedIndex {
            chunks: persisted
                .chunks
                .into_iter()
                .map(|entry| (entry.chunk_id.clone(), entry))
                .collect(),
        })
    }

    async fn trim_cached_chunks(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
        cached: &mut CachedMemoryEmbedIndex,
    ) -> Result<()> {
        let valid_ids = chunks.iter().map(chunk_id).collect::<Vec<_>>();
        let before = cached.chunks.len();
        cached
            .chunks
            .retain(|chunk_id, _| valid_ids.iter().any(|valid| valid == chunk_id));
        if cached.chunks.len() != before {
            self.persist_index(chunks, cached).await?;
        }
        Ok(())
    }

    async fn persist_index(
        &self,
        chunks: &[crate::MemoryCorpusChunk],
        cached: &CachedMemoryEmbedIndex,
    ) -> Result<()> {
        let valid_ids = chunks.iter().map(chunk_id).collect::<Vec<_>>();
        let persisted = PersistedMemoryEmbedIndex {
            backend: "memory-embed".to_string(),
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
}

#[async_trait]
impl MemoryBackend for MemoryEmbedBackend {
    async fn sync(&self) -> Result<MemorySyncStatus> {
        let corpus = load_memory_corpus(&self.workspace_root, &self.config.corpus).await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        let cached = self.ensure_chunk_index(&chunks).await?;
        Ok(MemorySyncStatus {
            backend: "memory-embed".to_string(),
            indexed_documents: corpus.documents.len(),
            indexed_lines: corpus.total_lines().max(cached.chunks.len()),
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
        let cached = self.ensure_chunk_index(&chunks).await?;
        let query_embedding = self
            .client
            .embed(
                self.config
                    .embedding
                    .as_ref()
                    .map(|cfg| cfg.model.as_str())
                    .unwrap_or(""),
                &[req.query.clone()],
            )
            .await;
        let (query_vector, fallback_used) = match query_embedding {
            Ok(mut embeddings) if embeddings.len() == 1 => (embeddings.pop(), false),
            _ => (None, true),
        };

        let mut hits = Vec::new();
        let vector_weight = self.config.hybrid.vector_weight.max(0.0);
        let text_weight = self.config.hybrid.text_weight.max(0.0);
        let weight_sum = (vector_weight + text_weight).max(f64::EPSILON);

        for chunk in &chunks {
            if let Some(prefix) = prefix.as_deref()
                && !chunk.path.starts_with(prefix)
            {
                continue;
            }
            let lexical = lexical_score(&req.query, &chunk.text);
            let vector = match (&query_vector, cached.chunks.get(&chunk_id(chunk))) {
                (Some(query), Some(entry)) => cosine_similarity(query, &entry.embedding),
                _ => 0.0,
            };
            let score = if fallback_used {
                lexical
            } else {
                ((vector_weight * vector) + (text_weight * lexical)) / weight_sum
            };
            if score <= 0.0 {
                continue;
            }
            let mut metadata = BTreeMap::new();
            metadata.insert("lexical_score".to_string(), json!(lexical));
            metadata.insert("vector_score".to_string(), json!(vector));
            if let Some(embedding) = &self.config.embedding {
                metadata.insert("model".to_string(), json!(embedding.model));
                metadata.insert("provider".to_string(), json!(embedding.provider));
            }
            metadata.insert("snapshot_id".to_string(), json!(chunk.snapshot_id));
            hits.push(MemorySearchHit {
                hit_id: format!("{}:{}", chunk.path, chunk.start_line),
                path: chunk.path.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                score,
                snippet: render_embed_snippet(&chunk.text, self.config.search.max_snippet_chars),
                metadata,
            });
        }

        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.start_line.cmp(&right.start_line))
        });
        hits.truncate(limit);

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "indexed_documents".to_string(),
            json!(corpus.documents.len()),
        );
        metadata.insert("cached_chunks".to_string(), json!(cached.chunks.len()));
        metadata.insert("fallback_used".to_string(), json!(fallback_used));
        metadata.insert(
            "index_path".to_string(),
            json!(self.index_path().to_string_lossy().to_string()),
        );
        Ok(MemorySearchResponse {
            backend: "memory-embed".to_string(),
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

fn chunk_id(chunk: &crate::MemoryCorpusChunk) -> String {
    // Chunk ids must be deterministic across process restarts so the on-disk cache can reuse
    // embeddings, but they also need to incorporate the snapshot boundary to invalidate edits.
    let mut digest = Sha256::new();
    digest.update(chunk.path.as_bytes());
    digest.update(b":");
    digest.update(chunk.snapshot_id.as_bytes());
    digest.update(b":");
    digest.update(chunk.start_line.to_string().as_bytes());
    digest.update(b":");
    digest.update(chunk.end_line.to_string().as_bytes());
    let output = digest.finalize();
    output[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
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
    use super::{EmbeddingClient, MemoryEmbedBackend};
    use crate::{MemoryBackend, MemoryEmbedConfig, MemorySearchRequest};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use tokio::fs;

    #[derive(Default)]
    struct MockEmbeddingClient {
        calls: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl EmbeddingClient for MockEmbeddingClient {
        async fn embed(&self, _model: &str, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
            self.calls.lock().unwrap().push(texts.len());
            Ok(texts
                .iter()
                .map(|text| {
                    if text.contains("semantic") || text.contains("query") {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                })
                .collect())
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
        assert_eq!(first_calls, vec![1]);

        let backend = MemoryEmbedBackend::new(dir.path().to_path_buf(), config, client.clone());
        backend.sync().await.unwrap();
        let second_calls = client.calls.lock().unwrap().clone();
        assert_eq!(second_calls, vec![1]);
    }
}
