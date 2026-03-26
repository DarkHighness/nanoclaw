use crate::lexical_index::{self, LexicalIndex, LexicalIndexChunk};
use crate::{
    MEMORY_CORE_SQLITE_INDEX_RELATIVE, MemoryBackend, MemoryCoreConfig, MemoryDocument,
    MemoryGetRequest, MemorySearchHit, MemorySearchRequest, MemorySearchResponse,
    MemoryStateLayout, MemorySyncStatus, Result, chunk_corpus, load_configured_memory_corpus,
};
use async_trait::async_trait;
use serde_json::json;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use store::RunStore;
use tools::format_numbered_lines;

const INDEX_BACKEND_ID: &str = "memory-core";
const INDEX_SCHEMA_VERSION: u32 = 1;

pub struct MemoryCoreBackend {
    workspace_root: PathBuf,
    config: MemoryCoreConfig,
    run_store: Option<Arc<dyn RunStore>>,
    lexical_index: LexicalIndex,
}

impl MemoryCoreBackend {
    #[must_use]
    pub fn new(workspace_root: PathBuf, config: MemoryCoreConfig) -> Self {
        Self {
            workspace_root: workspace_root.clone(),
            config,
            run_store: None,
            lexical_index: LexicalIndex::new(
                &workspace_root,
                INDEX_BACKEND_ID,
                MEMORY_CORE_SQLITE_INDEX_RELATIVE,
                INDEX_SCHEMA_VERSION,
            ),
        }
    }

    #[must_use]
    pub fn with_run_store(mut self, run_store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(run_store);
        self
    }

    fn state_layout(&self) -> MemoryStateLayout {
        MemoryStateLayout::new(&self.workspace_root)
    }

    fn config_fingerprint(&self) -> Result<String> {
        lexical_index::config_fingerprint(&json!({
            "corpus": &self.config.corpus,
            "chunking": &self.config.chunking,
        }))
    }

    async fn ensure_index_ready(
        &self,
        corpus: &crate::MemoryCorpus,
        chunks: &[crate::MemoryCorpusChunk],
        exported_run_count: usize,
    ) -> Result<()> {
        self.lexical_index
            .ensure_ready(
                &self.config_fingerprint()?,
                &document_snapshots(corpus),
                &lexical_index_chunks(chunks),
                exported_run_count,
            )
            .await?;
        Ok(())
    }

    fn load_index_lifecycle(&self) -> Result<crate::MemorySidecarLifecycle> {
        self.state_layout()
            .load_lifecycle(INDEX_BACKEND_ID)?
            .ok_or_else(|| {
                crate::MemoryError::invalid("memory-core lifecycle manifest missing after sync")
            })
    }
}

#[async_trait]
impl MemoryBackend for MemoryCoreBackend {
    async fn sync(&self) -> Result<MemorySyncStatus> {
        let (corpus, runtime_exports) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await?;
        let indexed_documents = corpus.documents.len();
        let indexed_lines = corpus.total_lines();
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        self.ensure_index_ready(&corpus, &chunks, runtime_exports.exported_runs)
            .await?;

        Ok(MemorySyncStatus {
            backend: INDEX_BACKEND_ID.to_string(),
            indexed_documents,
            indexed_lines,
        })
    }

    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse> {
        // Markdown files remain the source of truth; the SQLite sidecar is just
        // the derived retrieval index. Search lazily bootstraps the sidecar when
        // it is missing or config-invalid so the default plugin still works
        // without a separate indexing command.
        let (corpus, runtime_exports) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
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
        self.ensure_index_ready(&corpus, &chunks, runtime_exports.exported_runs)
            .await?;
        let lifecycle = self.load_index_lifecycle()?;
        let mut hits = self
            .lexical_index
            .search_hits(&req.query, prefix.as_deref(), limit)
            .await?
            .into_iter()
            .map(|entry| {
                let lexical_score = if entry.path == "MEMORY.md" {
                    (entry.score - 0.15).max(0.0)
                } else {
                    entry.score
                };
                let mut metadata = BTreeMap::new();
                metadata.insert("snapshot_id".to_string(), json!(entry.snapshot_id));
                metadata.insert("bm25_rank".to_string(), json!(entry.raw_rank));
                metadata.insert("lexical_score".to_string(), json!(lexical_score));
                MemorySearchHit {
                    hit_id: entry.chunk_id,
                    path: entry.path,
                    start_line: entry.start_line,
                    end_line: entry.end_line,
                    score: entry.score,
                    snippet: render_snippet(&entry.snippet, self.config.search.max_snippet_chars),
                    metadata,
                }
            })
            .collect::<Vec<_>>();
        hits.sort_by(compare_hit_score);

        let mut metadata = BTreeMap::new();
        metadata.insert("query".to_string(), json!(req.query));
        metadata.insert(
            "indexed_documents".to_string(),
            json!(lifecycle.indexed_document_count),
        );
        metadata.insert(
            "indexed_chunks".to_string(),
            json!(lifecycle.indexed_chunk_count),
        );
        metadata.insert(
            "runtime_exported_runs".to_string(),
            json!(runtime_exports.exported_runs),
        );
        if let Some(output_dir) = runtime_exports.output_dir {
            metadata.insert("runtime_export_dir".to_string(), json!(output_dir));
        }
        metadata.insert("fallback_used".to_string(), json!(false));

        Ok(MemorySearchResponse {
            backend: INDEX_BACKEND_ID.to_string(),
            hits,
            metadata,
        })
    }

    async fn get(&self, req: MemoryGetRequest) -> Result<MemoryDocument> {
        let (corpus, _) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await?;
        let requested = normalize_path(&req.path);
        let document = corpus
            .documents
            .iter()
            .find(|doc| doc.path == requested)
            .ok_or_else(|| crate::MemoryError::PathNotInCorpus(req.path.clone()))?;
        if document.lines.is_empty() {
            return Ok(MemoryDocument {
                path: document.path.clone(),
                snapshot_id: document.snapshot_id.clone(),
                requested_start_line: req.start_line.unwrap_or(1).max(1),
                resolved_start_line: 0,
                resolved_end_line: 0,
                total_lines: 0,
                text: String::new(),
            });
        }
        let start_line = req.start_line.unwrap_or(1).max(1);
        let line_count = req.line_count.unwrap_or(document.lines.len()).max(1);
        let resolved_start = start_line.min(document.lines.len().max(1));
        let resolved_end = (resolved_start + line_count - 1).min(document.lines.len());
        let text = format_numbered_lines(
            &document.lines[resolved_start - 1..resolved_end].to_vec(),
            resolved_start,
        );

        Ok(MemoryDocument {
            path: document.path.clone(),
            snapshot_id: document.snapshot_id.clone(),
            requested_start_line: start_line,
            resolved_start_line: resolved_start,
            resolved_end_line: resolved_end,
            total_lines: document.lines.len(),
            text,
        })
    }
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

fn document_snapshots(corpus: &crate::MemoryCorpus) -> BTreeMap<String, String> {
    corpus
        .documents
        .iter()
        .map(|doc| (doc.path.clone(), doc.snapshot_id.clone()))
        .collect()
}

fn render_snippet(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut value = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    value.push_str("...");
    value
}

fn compare_hit_score(left: &MemorySearchHit, right: &MemorySearchHit) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.start_line.cmp(&right.start_line))
}

fn stable_hash(value: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(value.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn chunk_id(chunk: &crate::MemoryCorpusChunk) -> String {
    stable_hash(&format!("{}:{}", chunk.path, chunk.start_line))
}

fn normalize_path(value: &str) -> String {
    value.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::MemoryCoreBackend;
    use crate::{
        MEMORY_CORE_SQLITE_INDEX_RELATIVE, MemoryBackend, MemoryCoreConfig, MemoryGetRequest,
        MemorySearchRequest, MemorySidecarStatus, MemoryStateLayout,
    };
    use rusqlite::Connection;
    use std::path::Path;
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn search_bootstraps_sqlite_sidecar() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("MEMORY.md"),
            "primary redis sentinel\nfallback worker",
        )
        .await
        .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(
            dir.path().join("memory/today.md"),
            "redis sentinel incident review\nanother note",
        )
        .await
        .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        let response = backend
            .search(MemorySearchRequest {
                query: "redis sentinel".to_string(),
                limit: Some(3),
                path_prefix: None,
            })
            .await
            .unwrap();
        assert_eq!(response.backend, "memory-core");
        assert!(!response.hits.is_empty());

        let layout = MemoryStateLayout::new(dir.path());
        let index_path = layout
            .resolve_index_path(None, Path::new(MEMORY_CORE_SQLITE_INDEX_RELATIVE))
            .unwrap();
        assert!(index_path.absolute_path().exists());
        let connection = Connection::open(index_path.absolute_path()).unwrap();
        let chunk_count: u32 = connection
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
            .unwrap();
        let fts_count: u32 = connection
            .query_row("SELECT COUNT(*) FROM chunks_fts", [], |row| row.get(0))
            .unwrap();
        assert!(chunk_count > 0);
        assert_eq!(chunk_count, fts_count);

        let lifecycle = layout.load_lifecycle("memory-core").unwrap().unwrap();
        assert_eq!(lifecycle.status, MemorySidecarStatus::Ready);
        assert_eq!(lifecycle.indexed_document_count, 2);
    }

    #[tokio::test]
    async fn search_respects_path_prefix() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("MEMORY.md"),
            "redis sentinel from curated memory",
        )
        .await
        .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(
            dir.path().join("memory/today.md"),
            "redis sentinel from daily note",
        )
        .await
        .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        let response = backend
            .search(MemorySearchRequest {
                query: "redis sentinel".to_string(),
                limit: Some(3),
                path_prefix: Some("memory/".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].path, "memory/today.md");
    }

    #[tokio::test]
    async fn sync_rebuilds_index_after_file_changes() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "alpha rollout notes")
            .await
            .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        backend.sync().await.unwrap();
        fs::write(dir.path().join("MEMORY.md"), "beta rollout notes")
            .await
            .unwrap();
        backend.sync().await.unwrap();

        let response = backend
            .search(MemorySearchRequest {
                query: "beta".to_string(),
                limit: Some(3),
                path_prefix: None,
            })
            .await
            .unwrap();
        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].path, "MEMORY.md");
    }

    #[tokio::test]
    async fn search_rebuilds_when_corpus_snapshots_drift() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "alpha rollout notes")
            .await
            .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        backend
            .search(MemorySearchRequest {
                query: "alpha".to_string(),
                limit: Some(3),
                path_prefix: None,
            })
            .await
            .unwrap();

        fs::write(dir.path().join("MEMORY.md"), "beta rollout notes")
            .await
            .unwrap();

        let response = backend
            .search(MemorySearchRequest {
                query: "beta".to_string(),
                limit: Some(3),
                path_prefix: None,
            })
            .await
            .unwrap();
        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].path, "MEMORY.md");

        let layout = MemoryStateLayout::new(dir.path());
        let lifecycle = layout.load_lifecycle("memory-core").unwrap().unwrap();
        assert_eq!(
            lifecycle.document_snapshots.get("MEMORY.md").cloned(),
            Some(super::stable_hash("beta rollout notes"))
        );
    }

    #[tokio::test]
    async fn get_reads_line_window() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "line1\nline2\nline3")
            .await
            .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        let document = backend
            .get(MemoryGetRequest {
                path: "MEMORY.md".to_string(),
                start_line: Some(2),
                line_count: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(document.resolved_start_line, 2);
        assert_eq!(document.resolved_end_line, 2);
        assert_eq!(document.text, " 2 | line2");
    }
}
