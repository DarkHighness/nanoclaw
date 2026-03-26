use crate::{
    MEMORY_CORE_SQLITE_INDEX_RELATIVE, MemoryBackend, MemoryCoreConfig, MemoryDocument,
    MemoryGetRequest, MemorySearchHit, MemorySearchRequest, MemorySearchResponse,
    MemorySidecarLifecycle, MemorySidecarStatus, MemoryStateLayout, MemorySyncStatus, Result,
    chunk_corpus, load_configured_memory_corpus,
};
use async_trait::async_trait;
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::json;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::RunStore;
use tools::format_numbered_lines;

const INDEX_BACKEND_ID: &str = "memory-core";
const INDEX_SCHEMA_VERSION: u32 = 1;
const CHUNKS_TABLE: &str = "chunks";
const FTS_TABLE: &str = "chunks_fts";
const META_TABLE: &str = "meta";
const META_KEY: &str = "memory_core_index_meta_v1";
#[derive(Clone, Debug)]
struct IndexedChunk {
    chunk_id: String,
    path: String,
    snapshot_id: String,
    start_line: usize,
    end_line: usize,
    text: String,
}

#[derive(Clone, Debug, Serialize)]
struct PersistedIndexMeta {
    schema_version: u32,
    config_fingerprint: String,
}

pub struct MemoryCoreBackend {
    workspace_root: PathBuf,
    config: MemoryCoreConfig,
    run_store: Option<Arc<dyn RunStore>>,
}

impl MemoryCoreBackend {
    #[must_use]
    pub fn new(workspace_root: PathBuf, config: MemoryCoreConfig) -> Self {
        Self {
            workspace_root,
            config,
            run_store: None,
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
        Ok(stable_hash(&serde_json::to_string(&json!({
            "corpus": &self.config.corpus,
            "chunking": &self.config.chunking,
        }))?))
    }

    fn default_index_relative_path() -> &'static Path {
        Path::new(MEMORY_CORE_SQLITE_INDEX_RELATIVE)
    }

    async fn ensure_index_ready(&self) -> Result<MemorySidecarLifecycle> {
        let layout = self.state_layout();
        let artifact_path = layout.resolve_index_path(None, Self::default_index_relative_path())?;
        let expected_fingerprint = self.config_fingerprint()?;
        let mut lifecycle = layout.load_lifecycle(INDEX_BACKEND_ID)?;
        let mut needs_sync = lifecycle.as_ref().is_none_or(|entry| {
            entry.backend != INDEX_BACKEND_ID
                || entry.status != MemorySidecarStatus::Ready
                || entry.schema_version != INDEX_SCHEMA_VERSION
                || entry.config_fingerprint != expected_fingerprint
                || entry.artifact_path != artifact_path.relative_display()
        }) || !artifact_path.absolute_path().exists();

        if !needs_sync && let Some(existing) = lifecycle.as_ref() {
            let (corpus, _) = load_configured_memory_corpus(
                &self.workspace_root,
                &self.config.corpus,
                self.run_store.as_ref(),
            )
            .await?;
            // The SQLite file is only a derived cache. Even when the lifecycle
            // manifest still looks structurally valid, treat any Markdown
            // snapshot drift as stale and rebuild from source-of-truth files.
            needs_sync = document_snapshots(&corpus) != existing.document_snapshots;
        }

        if needs_sync {
            self.sync().await?;
            lifecycle = layout.load_lifecycle(INDEX_BACKEND_ID)?;
        }

        lifecycle.ok_or_else(|| {
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
        let lifecycle = self.state_layout();
        let artifact_path =
            lifecycle.resolve_index_path(None, Self::default_index_relative_path())?;
        let config_fingerprint = self.config_fingerprint()?;
        let indexed_chunks = build_indexed_chunks(&corpus, &self.config);
        let indexed_chunk_count = indexed_chunks.len();
        let document_snapshots = document_snapshots(&corpus);

        lifecycle.write_lifecycle(
            INDEX_BACKEND_ID,
            MemorySidecarLifecycle {
                backend: INDEX_BACKEND_ID.to_string(),
                status: MemorySidecarStatus::Rebuilding,
                schema_version: INDEX_SCHEMA_VERSION,
                config_fingerprint: config_fingerprint.clone(),
                indexed_chunk_count,
                indexed_document_count: indexed_documents,
                exported_run_count: runtime_exports.exported_runs,
                artifact_path: artifact_path.relative_display(),
                document_snapshots: document_snapshots.clone(),
                ..MemorySidecarLifecycle::default()
            },
        )?;

        replace_sqlite_index(
            artifact_path.absolute_path().to_path_buf(),
            indexed_chunks,
            PersistedIndexMeta {
                schema_version: INDEX_SCHEMA_VERSION,
                config_fingerprint: config_fingerprint.clone(),
            },
        )
        .await?;

        lifecycle.write_lifecycle(
            INDEX_BACKEND_ID,
            MemorySidecarLifecycle {
                backend: INDEX_BACKEND_ID.to_string(),
                status: MemorySidecarStatus::Ready,
                schema_version: INDEX_SCHEMA_VERSION,
                config_fingerprint,
                indexed_chunk_count,
                indexed_document_count: indexed_documents,
                exported_run_count: runtime_exports.exported_runs,
                artifact_path: artifact_path.relative_display(),
                document_snapshots,
                ..MemorySidecarLifecycle::default()
            },
        )?;

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
        let lifecycle = self.ensure_index_ready().await?;
        let layout = self.state_layout();
        let artifact_path = layout.resolve_index_path(None, Self::default_index_relative_path())?;
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
        let hits = search_sqlite_index(
            artifact_path.absolute_path().to_path_buf(),
            req.query.clone(),
            prefix,
            limit,
            self.config.search.max_snippet_chars,
        )
        .await?;

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
            json!(lifecycle.exported_run_count),
        );
        if let Some(runtime_exports) = layout.load_lifecycle("runtime-exports")?
            && !runtime_exports.artifact_path.is_empty()
        {
            metadata.insert(
                "runtime_export_dir".to_string(),
                json!(runtime_exports.artifact_path),
            );
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

fn build_indexed_chunks(
    corpus: &crate::MemoryCorpus,
    config: &MemoryCoreConfig,
) -> Vec<IndexedChunk> {
    chunk_corpus(corpus, &config.chunking)
        .into_iter()
        .map(|chunk| IndexedChunk {
            chunk_id: hit_id(&chunk.path, chunk.start_line),
            path: chunk.path,
            snapshot_id: chunk.snapshot_id,
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            text: chunk.text,
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

async fn replace_sqlite_index(
    path: PathBuf,
    chunks: Vec<IndexedChunk>,
    meta: PersistedIndexMeta,
) -> Result<()> {
    tokio::task::spawn_blocking(move || replace_sqlite_index_blocking(&path, &chunks, &meta))
        .await
        .map_err(|error| {
            crate::MemoryError::invalid(format!("memory-core sqlite task failed: {error}"))
        })?
}

fn replace_sqlite_index_blocking(
    path: &Path,
    chunks: &[IndexedChunk],
    meta: &PersistedIndexMeta,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = open_sqlite(path)?;
    let transaction = connection.transaction().map_err(map_sqlite_error)?;
    transaction
        .execute(&format!("DELETE FROM {CHUNKS_TABLE}"), [])
        .map_err(map_sqlite_error)?;
    transaction
        .execute(&format!("DELETE FROM {FTS_TABLE}"), [])
        .map_err(map_sqlite_error)?;
    transaction
        .execute(&format!("DELETE FROM {META_TABLE}"), [])
        .map_err(map_sqlite_error)?;

    let mut chunk_statement = transaction
        .prepare(&format!(
            "INSERT INTO {CHUNKS_TABLE} \
             (chunk_id, path, snapshot_id, start_line, end_line, text) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        ))
        .map_err(map_sqlite_error)?;
    let mut fts_statement = transaction
        .prepare(&format!(
            "INSERT INTO {FTS_TABLE} \
             (text, chunk_id, path, snapshot_id, start_line, end_line) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        ))
        .map_err(map_sqlite_error)?;
    for chunk in chunks {
        chunk_statement
            .execute(params![
                chunk.chunk_id,
                chunk.path,
                chunk.snapshot_id,
                chunk.start_line as u32,
                chunk.end_line as u32,
                chunk.text,
            ])
            .map_err(map_sqlite_error)?;
        fts_statement
            .execute(params![
                chunk.text,
                chunk.chunk_id,
                chunk.path,
                chunk.snapshot_id,
                chunk.start_line as u32,
                chunk.end_line as u32,
            ])
            .map_err(map_sqlite_error)?;
    }
    let encoded_meta = serde_json::to_string(meta)?;
    transaction
        .execute(
            &format!("INSERT INTO {META_TABLE} (key, value) VALUES (?1, ?2)"),
            params![META_KEY, encoded_meta],
        )
        .map_err(map_sqlite_error)?;
    drop(chunk_statement);
    drop(fts_statement);
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(())
}

async fn search_sqlite_index(
    path: PathBuf,
    query: String,
    path_prefix: Option<String>,
    limit: usize,
    max_snippet_chars: usize,
) -> Result<Vec<MemorySearchHit>> {
    tokio::task::spawn_blocking(move || {
        search_sqlite_index_blocking(
            &path,
            &query,
            path_prefix.as_deref(),
            limit,
            max_snippet_chars,
        )
    })
    .await
    .map_err(|error| {
        crate::MemoryError::invalid(format!("memory-core sqlite task failed: {error}"))
    })?
}

fn search_sqlite_index_blocking(
    path: &Path,
    query: &str,
    path_prefix: Option<&str>,
    limit: usize,
    max_snippet_chars: usize,
) -> Result<Vec<MemorySearchHit>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let Some(fts_query) = build_fts_query(query) else {
        return Ok(Vec::new());
    };
    let connection = open_sqlite(path)?;
    let sql = if path_prefix.is_some() {
        format!(
            "SELECT chunk_id, path, snapshot_id, start_line, end_line, text, \
                    snippet({FTS_TABLE}, 0, '', '', '...', 32) AS snippet, \
                    bm25({FTS_TABLE}) AS rank \
               FROM {FTS_TABLE} \
              WHERE {FTS_TABLE} MATCH ?1 \
                AND path LIKE ?2 ESCAPE '\\' \
              ORDER BY rank ASC \
              LIMIT ?3"
        )
    } else {
        format!(
            "SELECT chunk_id, path, snapshot_id, start_line, end_line, text, \
                    snippet({FTS_TABLE}, 0, '', '', '...', 32) AS snippet, \
                    bm25({FTS_TABLE}) AS rank \
               FROM {FTS_TABLE} \
              WHERE {FTS_TABLE} MATCH ?1 \
              ORDER BY rank ASC \
              LIMIT ?2"
        )
    };
    let mut statement = connection.prepare(&sql).map_err(map_sqlite_error)?;
    let escaped_prefix = path_prefix.map(|prefix| format!("{}%", escape_sql_like_prefix(prefix)));
    let mut rows = if let Some(prefix) = escaped_prefix {
        statement
            .query(params![fts_query, prefix, limit as u32])
            .map_err(map_sqlite_error)?
    } else {
        statement
            .query(params![fts_query, limit as u32])
            .map_err(map_sqlite_error)?
    };
    let mut hits = Vec::new();
    while let Some(row) = rows.next().map_err(map_sqlite_error)? {
        let hit_id: String = row.get(0).map_err(map_sqlite_error)?;
        let path: String = row.get(1).map_err(map_sqlite_error)?;
        let snapshot_id: String = row.get(2).map_err(map_sqlite_error)?;
        let start_line = row.get::<_, u32>(3).map_err(map_sqlite_error)? as usize;
        let end_line = row.get::<_, u32>(4).map_err(map_sqlite_error)? as usize;
        let text: String = row.get(5).map_err(map_sqlite_error)?;
        let snippet: Option<String> = row.get(6).map_err(map_sqlite_error)?;
        let raw_rank: f64 = row.get(7).map_err(map_sqlite_error)?;
        let lexical_score = bm25_rank_to_score(raw_rank);
        let score = if path == "MEMORY.md" {
            (lexical_score + 0.15).min(1.0)
        } else {
            lexical_score
        };
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "snapshot_id".to_string(),
            serde_json::Value::String(snapshot_id),
        );
        metadata.insert("bm25_rank".to_string(), json!(raw_rank));
        metadata.insert("lexical_score".to_string(), json!(lexical_score));
        hits.push(MemorySearchHit {
            hit_id,
            path,
            start_line,
            end_line,
            score,
            snippet: render_snippet(
                &snippet
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(text),
                max_snippet_chars,
            ),
            metadata,
        });
    }
    hits.sort_by(compare_hit_score);
    Ok(hits)
}

fn open_sqlite(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path).map_err(map_sqlite_error)?;
    connection
        .execute_batch("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;")
        .map_err(map_sqlite_error)?;
    connection
        .execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {META_TABLE} (\
               key TEXT PRIMARY KEY, \
               value TEXT NOT NULL\
             );\
             CREATE TABLE IF NOT EXISTS {CHUNKS_TABLE} (\
               chunk_id TEXT PRIMARY KEY, \
               path TEXT NOT NULL, \
               snapshot_id TEXT NOT NULL, \
               start_line INTEGER NOT NULL, \
               end_line INTEGER NOT NULL, \
               text TEXT NOT NULL\
             );\
             CREATE INDEX IF NOT EXISTS idx_memory_core_chunks_path \
               ON {CHUNKS_TABLE}(path);\
             CREATE VIRTUAL TABLE IF NOT EXISTS {FTS_TABLE} USING fts5(\
               text, \
               chunk_id UNINDEXED, \
               path UNINDEXED, \
               snapshot_id UNINDEXED, \
               start_line UNINDEXED, \
               end_line UNINDEXED\
             );"
        ))
        .map_err(map_sqlite_error)?;
    Ok(connection)
}

fn build_fts_query(raw: &str) -> Option<String> {
    let tokens = raw
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace('"', ""))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .into_iter()
            .map(|token| format!("\"{token}\""))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

fn bm25_rank_to_score(rank: f64) -> f64 {
    if !rank.is_finite() {
        return 1.0 / 1000.0;
    }
    if rank < 0.0 {
        let relevance = -rank;
        return relevance / (1.0 + relevance);
    }
    1.0 / (1.0 + rank)
}

fn escape_sql_like_prefix(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn stable_hash(value: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(value.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn map_sqlite_error(error: rusqlite::Error) -> crate::MemoryError {
    crate::MemoryError::invalid(format!("memory-core sqlite error: {error}"))
}

fn tokenize_query(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_alphanumeric())
        .map(|value| value.to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
}

pub(crate) fn lexical_score(query: &str, text: &str) -> f64 {
    score_text(text, &tokenize_query(query))
}

fn score_text(text: &str, terms: &[String]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    let text = text.to_lowercase();
    let mut score = 0.0;
    let mut seen = 0usize;
    for term in terms {
        let count = text.matches(term).count();
        if count == 0 {
            continue;
        }
        score += count as f64;
        seen += 1;
    }
    if seen == 0 {
        return 0.0;
    }
    let recall = seen as f64 / terms.len() as f64;
    // Favor chunks that cover more of the query, then add a small frequency
    // term so repeated exact tokens still separate otherwise similar chunks.
    (recall * 2.0) + (score * 0.1)
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

fn hit_id(path: &str, line: usize) -> String {
    stable_hash(&format!("{path}:{line}"))
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
