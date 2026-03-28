use crate::lexical_index::{self, LexicalIndex, LexicalIndexChunk};
use crate::managed_files::record_memory;
use crate::promotion::promote_memory;
use crate::retention::forget_memory;
use crate::retrieval_policy;
use crate::{
    MEMORY_CORE_SQLITE_INDEX_RELATIVE, MemoryBackend, MemoryCoreConfig, MemoryDocument,
    MemoryForgetRequest, MemoryGetRequest, MemoryListEntry, MemoryListRequest, MemoryListResponse,
    MemoryMutationResponse, MemoryPromoteRequest, MemoryRecordRequest, MemorySearchHit,
    MemorySearchRequest, MemorySearchResponse, MemoryStateLayout, MemorySyncStatus, Result,
    chunk_corpus, load_configured_memory_corpus,
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

    async fn load_corpus(&self) -> Result<(crate::MemoryCorpus, crate::MemoryRuntimeExportStats)> {
        load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await
    }
}

#[async_trait]
impl MemoryBackend for MemoryCoreBackend {
    async fn sync(&self) -> Result<MemorySyncStatus> {
        let (corpus, runtime_exports) = self.load_corpus().await?;
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
        let (corpus, runtime_exports) = self.load_corpus().await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        let limit = req
            .limit
            .unwrap_or(self.config.search.max_results)
            .max(1)
            .min(50);
        let candidate_limit = limit.saturating_mul(12).min(400);
        let prefix = req
            .path_prefix
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        self.ensure_index_ready(&corpus, &chunks, runtime_exports.exported_runs)
            .await?;
        let lifecycle = self.load_index_lifecycle()?;
        let chunk_map = chunks
            .iter()
            .map(|chunk| (chunk_id(chunk), chunk))
            .collect::<BTreeMap<_, _>>();
        let mut hits = self
            .lexical_index
            .search_hits(&req.query, prefix.as_deref(), candidate_limit)
            .await?
            .into_iter()
            .filter_map(|entry| {
                let chunk = chunk_map.get(&entry.chunk_id)?;
                if !retrieval_policy::matches_search_filters(&entry.path, &chunk.metadata, &req) {
                    return None;
                }
                let signals = retrieval_policy::search_signals(&entry.path, &chunk.metadata, &req);
                let lexical_score = if entry.path == "MEMORY.md" {
                    (entry.score - 0.15).max(0.0)
                } else {
                    entry.score
                };
                let retrieval_score = lexical_score * signals.total_multiplier();
                let mut metadata = BTreeMap::new();
                metadata.insert("snapshot_id".to_string(), json!(entry.snapshot_id));
                metadata.insert("bm25_rank".to_string(), json!(entry.raw_rank));
                metadata.insert("lexical_score".to_string(), json!(lexical_score));
                metadata.insert(
                    "memory_scope".to_string(),
                    json!(chunk.metadata.scope.as_str()),
                );
                metadata.insert("memory_layer".to_string(), json!(chunk.metadata.layer));
                metadata.insert(
                    "memory_status".to_string(),
                    json!(chunk.metadata.status.as_str()),
                );
                metadata.insert("scope_weight".to_string(), json!(signals.scope_weight));
                metadata.insert(
                    "recency_multiplier".to_string(),
                    json!(signals.recency_multiplier),
                );
                metadata.insert(
                    "session_match_bonus".to_string(),
                    json!(signals.session_match_bonus),
                );
                metadata.insert(
                    "agent_match_bonus".to_string(),
                    json!(signals.agent_match_bonus),
                );
                metadata.insert(
                    "task_match_bonus".to_string(),
                    json!(signals.task_match_bonus),
                );
                metadata.insert(
                    "run_match_bonus".to_string(),
                    json!(signals.run_match_bonus),
                );
                metadata.insert("stale_penalty".to_string(), json!(signals.stale_penalty));
                metadata.insert("retrieval_score".to_string(), json!(retrieval_score));
                if let Some(updated_at_ms) = chunk.metadata.updated_at_ms {
                    metadata.insert("updated_at_ms".to_string(), json!(updated_at_ms));
                }
                Some(MemorySearchHit {
                    hit_id: entry.chunk_id,
                    path: entry.path,
                    start_line: entry.start_line,
                    end_line: entry.end_line,
                    score: retrieval_score,
                    snippet: render_snippet(&entry.snippet, self.config.search.max_snippet_chars),
                    document_metadata: chunk.metadata.clone(),
                    metadata,
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(compare_hit_score);
        hits.truncate(limit);

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
        metadata.insert(
            "runtime_exported_documents".to_string(),
            json!(runtime_exports.exported_documents),
        );
        metadata.insert("candidate_limit".to_string(), json!(candidate_limit));
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
        let (corpus, _) = self.load_corpus().await?;
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
                title: document.title.clone(),
                requested_start_line: req.start_line.unwrap_or(1).max(1),
                resolved_start_line: 0,
                resolved_end_line: 0,
                total_lines: 0,
                text: String::new(),
                metadata: document.metadata.clone(),
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
            title: document.title.clone(),
            requested_start_line: start_line,
            resolved_start_line: resolved_start,
            resolved_end_line: resolved_end,
            total_lines: document.lines.len(),
            text,
            metadata: document.metadata.clone(),
        })
    }

    async fn list(&self, req: MemoryListRequest) -> Result<MemoryListResponse> {
        let (corpus, runtime_exports) = self.load_corpus().await?;
        let limit = req.limit.unwrap_or(100).max(1).min(500);
        let mut entries = corpus
            .documents
            .into_iter()
            .filter(|document| {
                retrieval_policy::matches_list_filters(&document.path, &document.metadata, &req)
            })
            .map(|document| MemoryListEntry {
                path: document.path,
                title: document.title,
                snapshot_id: document.snapshot_id,
                total_lines: document.lines.len(),
                metadata: document.metadata,
            })
            .collect::<Vec<_>>();
        entries.sort_by(compare_list_entry);
        entries.truncate(limit);

        let mut metadata = BTreeMap::new();
        metadata.insert("count".to_string(), json!(entries.len()));
        metadata.insert(
            "runtime_exported_documents".to_string(),
            json!(runtime_exports.exported_documents),
        );
        Ok(MemoryListResponse { entries, metadata })
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

fn compare_list_entry(left: &MemoryListEntry, right: &MemoryListEntry) -> Ordering {
    right
        .metadata
        .updated_at_ms
        .cmp(&left.metadata.updated_at_ms)
        .then_with(|| left.path.cmp(&right.path))
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
        MEMORY_CORE_SQLITE_INDEX_RELATIVE, MemoryBackend, MemoryCoreConfig, MemoryForgetRequest,
        MemoryGetRequest, MemoryListRequest, MemoryPromoteRequest, MemoryRecordRequest,
        MemoryScope, MemorySearchRequest, MemorySidecarStatus, MemoryStateLayout, MemoryStatus,
    };
    use nanoclaw_test_support::run_current_thread_test;
    use rusqlite::Connection;
    use std::path::Path;
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

    bounded_async_test!(
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

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            let response = backend
                .search(MemorySearchRequest {
                    query: "redis sentinel".to_string(),
                    limit: Some(3),
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
    );

    bounded_async_test!(
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

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            let response = backend
                .search(MemorySearchRequest {
                    query: "redis sentinel".to_string(),
                    limit: Some(3),
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
            assert_eq!(response.hits.len(), 1);
            assert_eq!(response.hits[0].path, "memory/today.md");
        }
    );

    bounded_async_test!(
        async fn sync_rebuilds_index_after_file_changes() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "alpha rollout notes")
                .await
                .unwrap();

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
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
            assert_eq!(response.hits.len(), 1);
            assert_eq!(response.hits[0].path, "MEMORY.md");
        }
    );

    bounded_async_test!(
        async fn search_rebuilds_when_corpus_snapshots_drift() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "alpha rollout notes")
                .await
                .unwrap();

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            backend
                .search(MemorySearchRequest {
                    query: "alpha".to_string(),
                    limit: Some(3),
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

            fs::write(dir.path().join("MEMORY.md"), "beta rollout notes")
                .await
                .unwrap();

            let response = backend
                .search(MemorySearchRequest {
                    query: "beta".to_string(),
                    limit: Some(3),
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
            assert_eq!(response.hits.len(), 1);
            assert_eq!(response.hits[0].path, "MEMORY.md");

            let layout = MemoryStateLayout::new(dir.path());
            let lifecycle = layout.load_lifecycle("memory-core").unwrap().unwrap();
            assert_eq!(
                lifecycle.document_snapshots.get("MEMORY.md").cloned(),
                Some(super::stable_hash("beta rollout notes"))
            );
        }
    );

    bounded_async_test!(
        async fn get_reads_line_window() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "line1\nline2\nline3")
                .await
                .unwrap();

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
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
    );

    bounded_async_test!(
        async fn search_prefers_recent_daily_logs_over_stale_ones() {
            let dir = tempdir().unwrap();
            let today = OffsetDateTime::now_utc().date();
            let stale = today - Duration::days(120);
            fs::create_dir_all(dir.path().join("memory")).await.unwrap();
            fs::write(
                dir.path().join("memory").join(format!("{stale}.md")),
                "rollout checklist",
            )
            .await
            .unwrap();
            fs::write(
                dir.path().join("memory").join(format!("{today}.md")),
                "rollout checklist",
            )
            .await
            .unwrap();

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            let response = backend
                .search(MemorySearchRequest {
                    query: "rollout".to_string(),
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
                    .and_then(serde_json::Value::as_str),
                Some("daily-log")
            );
            assert!(
                response.hits[0].score > response.hits[1].score,
                "recent daily log should outrank stale daily log after decay"
            );
        }
    );

    bounded_async_test!(
        async fn search_filters_by_scope_and_tags() {
            let dir = tempdir().unwrap();
            fs::write(dir.path().join("MEMORY.md"), "deploy checklist")
                .await
                .unwrap();
            fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions"))
                .await
                .unwrap();
            fs::write(
            dir.path()
                .join(".nanoclaw/memory/working/sessions/session_1.md"),
            "---\nscope: working\nlayer: working-session\nsession_id: session_1\ntags:\n  - debug\nstatus: ready\n---\n# Session session_1\n\ndeploy checklist",
        )
        .await
        .unwrap();

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            let response = backend
                .search(MemorySearchRequest {
                    query: "deploy".to_string(),
                    limit: Some(2),
                    path_prefix: None,
                    scopes: Some(vec![MemoryScope::Working]),
                    tags: Some(vec!["debug".to_string()]),
                    run_id: None,
                    session_id: Some("session_1".into()),
                    agent_name: None,
                    task_id: None,
                    include_stale: Some(false),
                })
                .await
                .unwrap();

            assert_eq!(response.hits.len(), 1);
            assert_eq!(
                response.hits[0].path,
                ".nanoclaw/memory/working/sessions/session_1.md"
            );
        }
    );

    bounded_async_test!(
        async fn promote_and_forget_update_memory_lifecycle() {
            let dir = tempdir().unwrap();
            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            let source = backend
                .record(MemoryRecordRequest {
                    scope: MemoryScope::Working,
                    title: "Observed fix".to_string(),
                    content: "Use a canary deploy before restart.".to_string(),
                    layer: None,
                    tags: vec!["deploy".to_string()],
                    run_id: Some("run_1".into()),
                    session_id: Some("session_1".into()),
                    agent_name: None,
                    task_id: None,
                })
                .await
                .unwrap();

            let promoted = backend
                .promote(MemoryPromoteRequest {
                    source_path: source.path.clone(),
                    target_scope: MemoryScope::Semantic,
                    title: "Canary Deploy Rule".to_string(),
                    content: "Always do a canary deploy before restart.".to_string(),
                    layer: None,
                    tags: vec!["deploy".to_string(), "verified".to_string()],
                })
                .await
                .unwrap();

            let source_doc = backend
                .get(MemoryGetRequest {
                    path: source.path.clone(),
                    start_line: None,
                    line_count: None,
                })
                .await
                .unwrap();
            assert_eq!(source_doc.metadata.status, MemoryStatus::Stale);

            let promoted_search = backend
                .search(MemorySearchRequest {
                    query: "canary deploy".to_string(),
                    limit: Some(5),
                    path_prefix: Some(".nanoclaw/memory/semantic/".to_string()),
                    scopes: Some(vec![MemoryScope::Semantic]),
                    tags: Some(vec!["verified".to_string()]),
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: Some(false),
                })
                .await
                .unwrap();
            assert_eq!(promoted_search.hits.len(), 1);
            assert_eq!(promoted_search.hits[0].path, promoted.path);

            backend
                .forget(MemoryForgetRequest {
                    path: promoted.path.clone(),
                    status: MemoryStatus::Archived,
                })
                .await
                .unwrap();

            let listed = backend
                .list(MemoryListRequest {
                    limit: Some(10),
                    path_prefix: Some(".nanoclaw/memory/semantic/".to_string()),
                    scopes: Some(vec![MemoryScope::Semantic]),
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: Some(true),
                })
                .await
                .unwrap();
            assert_eq!(listed.entries.len(), 1);
            assert_eq!(listed.entries[0].metadata.status, MemoryStatus::Archived);
        }
    );

    bounded_async_test!(
        async fn list_hides_non_ready_entries_by_default_and_can_include_them_explicitly() {
            let dir = tempdir().unwrap();
            fs::create_dir_all(dir.path().join(".nanoclaw/memory/semantic"))
                .await
                .unwrap();
            fs::write(
                dir.path().join(".nanoclaw/memory/semantic/ready.md"),
                "---\nscope: semantic\nlayer: rule\nstatus: ready\n---\n# Ready\n\nkeep this",
            )
            .await
            .unwrap();
            fs::write(
                dir.path().join(".nanoclaw/memory/semantic/stale.md"),
                "---\nscope: semantic\nlayer: rule\nstatus: stale\n---\n# Stale\n\nhide this by default",
            )
            .await
            .unwrap();
            fs::write(
                dir.path().join(".nanoclaw/memory/semantic/archived.md"),
                "---\nscope: semantic\nlayer: rule\nstatus: archived\n---\n# Archived\n\nshow only when requested",
            )
            .await
            .unwrap();

            let backend =
                MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
            let default_list = backend
                .list(MemoryListRequest {
                    limit: Some(10),
                    path_prefix: Some(".nanoclaw/memory/semantic/".to_string()),
                    scopes: Some(vec![MemoryScope::Semantic]),
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: None,
                })
                .await
                .unwrap();
            assert_eq!(default_list.entries.len(), 1);
            assert_eq!(default_list.entries[0].metadata.status, MemoryStatus::Ready);

            let full_list = backend
                .list(MemoryListRequest {
                    limit: Some(10),
                    path_prefix: Some(".nanoclaw/memory/semantic/".to_string()),
                    scopes: Some(vec![MemoryScope::Semantic]),
                    tags: None,
                    run_id: None,
                    session_id: None,
                    agent_name: None,
                    task_id: None,
                    include_stale: Some(true),
                })
                .await
                .unwrap();
            assert_eq!(full_list.entries.len(), 3);
            assert!(
                full_list
                    .entries
                    .iter()
                    .any(|entry| entry.metadata.status == MemoryStatus::Stale)
            );
            assert!(
                full_list
                    .entries
                    .iter()
                    .any(|entry| entry.metadata.status == MemoryStatus::Archived)
            );
        }
    );
}
