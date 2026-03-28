use crate::{
    MemorySidecarLifecycle, MemorySidecarStatus, MemoryStateLayout, ResolvedStatePath, Result,
};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const CHUNKS_TABLE: &str = "chunks";
const FTS_TABLE: &str = "chunks_fts";
const META_TABLE: &str = "meta";
const META_KEY: &str = "memory_lexical_index_meta_v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LexicalIndexChunk {
    pub(crate) chunk_id: String,
    pub(crate) path: String,
    pub(crate) snapshot_id: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LexicalSearchMatch {
    pub(crate) chunk_id: String,
    pub(crate) path: String,
    pub(crate) snapshot_id: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) snippet: String,
    pub(crate) score: f64,
    pub(crate) raw_rank: f64,
}

#[derive(Clone, Debug, Serialize)]
struct PersistedLexicalIndexMeta {
    schema_version: u32,
    config_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RefreshPlan {
    Ready,
    Rebuild,
    Incremental { changed_paths: BTreeSet<String> },
}

#[derive(Clone, Debug)]
pub(crate) struct LexicalIndex {
    layout: MemoryStateLayout,
    backend_id: &'static str,
    default_relative_path: &'static str,
    schema_version: u32,
}

impl LexicalIndex {
    pub(crate) fn new(
        workspace_root: impl AsRef<Path>,
        backend_id: &'static str,
        default_relative_path: &'static str,
        schema_version: u32,
    ) -> Self {
        Self {
            layout: MemoryStateLayout::new(workspace_root),
            backend_id,
            default_relative_path,
            schema_version,
        }
    }

    pub(crate) fn artifact_path(&self) -> Result<ResolvedStatePath> {
        self.layout
            .resolve_index_path(None, Path::new(self.default_relative_path))
    }

    pub(crate) async fn ensure_ready(
        &self,
        config_fingerprint: &str,
        document_snapshots: &BTreeMap<String, String>,
        chunks: &[LexicalIndexChunk],
        exported_run_count: usize,
    ) -> Result<ResolvedStatePath> {
        let artifact_path = self.artifact_path()?;
        let lifecycle = self.layout.load_lifecycle(self.backend_id)?;
        match self.refresh_plan(
            &artifact_path,
            lifecycle.as_ref(),
            config_fingerprint,
            document_snapshots,
        ) {
            RefreshPlan::Ready => return Ok(artifact_path),
            RefreshPlan::Rebuild => {
                self.layout.write_lifecycle(
                    self.backend_id,
                    MemorySidecarLifecycle {
                        backend: self.backend_id.to_string(),
                        status: MemorySidecarStatus::Rebuilding,
                        schema_version: self.schema_version,
                        config_fingerprint: config_fingerprint.to_string(),
                        indexed_chunk_count: chunks.len(),
                        indexed_document_count: document_snapshots.len(),
                        exported_run_count,
                        artifact_path: artifact_path.relative_display(),
                        document_snapshots: document_snapshots.clone(),
                        ..MemorySidecarLifecycle::default()
                    },
                )?;
                replace_sqlite_index(
                    artifact_path.absolute_path().to_path_buf(),
                    chunks.to_vec(),
                    PersistedLexicalIndexMeta {
                        schema_version: self.schema_version,
                        config_fingerprint: config_fingerprint.to_string(),
                    },
                )
                .await?;
            }
            RefreshPlan::Incremental { changed_paths } => {
                self.layout.write_lifecycle(
                    self.backend_id,
                    MemorySidecarLifecycle {
                        backend: self.backend_id.to_string(),
                        status: MemorySidecarStatus::Rebuilding,
                        schema_version: self.schema_version,
                        config_fingerprint: config_fingerprint.to_string(),
                        indexed_chunk_count: chunks.len(),
                        indexed_document_count: document_snapshots.len(),
                        exported_run_count,
                        artifact_path: artifact_path.relative_display(),
                        document_snapshots: document_snapshots.clone(),
                        ..MemorySidecarLifecycle::default()
                    },
                )?;
                let changed_chunks = chunks_for_paths(chunks, &changed_paths);
                // Snapshot drift is usually a small document-local edit. Refreshing
                // only the affected paths keeps lexical sidecars hot while still
                // falling back to a full rebuild if the SQLite artifact is stale.
                if let Err(refresh_error) = refresh_sqlite_index(
                    artifact_path.absolute_path().to_path_buf(),
                    changed_paths.into_iter().collect(),
                    changed_chunks,
                    PersistedLexicalIndexMeta {
                        schema_version: self.schema_version,
                        config_fingerprint: config_fingerprint.to_string(),
                    },
                )
                .await
                {
                    replace_sqlite_index(
                        artifact_path.absolute_path().to_path_buf(),
                        chunks.to_vec(),
                        PersistedLexicalIndexMeta {
                            schema_version: self.schema_version,
                            config_fingerprint: config_fingerprint.to_string(),
                        },
                    )
                    .await
                    .map_err(|rebuild_error| {
                        crate::MemoryError::invalid(format!(
                            "memory lexical incremental refresh failed: {refresh_error}; \
                             full rebuild fallback also failed: {rebuild_error}"
                        ))
                    })?;
                }
            }
        }

        self.layout.write_lifecycle(
            self.backend_id,
            MemorySidecarLifecycle {
                backend: self.backend_id.to_string(),
                status: MemorySidecarStatus::Ready,
                schema_version: self.schema_version,
                config_fingerprint: config_fingerprint.to_string(),
                indexed_chunk_count: chunks.len(),
                indexed_document_count: document_snapshots.len(),
                exported_run_count,
                artifact_path: artifact_path.relative_display(),
                document_snapshots: document_snapshots.clone(),
                ..MemorySidecarLifecycle::default()
            },
        )?;

        Ok(artifact_path)
    }

    fn refresh_plan(
        &self,
        artifact_path: &ResolvedStatePath,
        lifecycle: Option<&MemorySidecarLifecycle>,
        config_fingerprint: &str,
        document_snapshots: &BTreeMap<String, String>,
    ) -> RefreshPlan {
        let Some(existing) = lifecycle else {
            return RefreshPlan::Rebuild;
        };
        if existing.backend != self.backend_id
            || existing.status != MemorySidecarStatus::Ready
            || existing.schema_version != self.schema_version
            || existing.config_fingerprint != config_fingerprint
            || existing.artifact_path != artifact_path.relative_display()
            || !artifact_path.absolute_path().exists()
        {
            return RefreshPlan::Rebuild;
        }
        if existing.document_snapshots == *document_snapshots {
            RefreshPlan::Ready
        } else {
            RefreshPlan::Incremental {
                changed_paths: changed_document_paths(
                    &existing.document_snapshots,
                    document_snapshots,
                ),
            }
        }
    }

    pub(crate) async fn search_ranked(
        &self,
        query: &str,
        path_prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<LexicalSearchMatch>> {
        self.search_hits(query, path_prefix, limit).await
    }

    pub(crate) async fn search_hits(
        &self,
        query: &str,
        path_prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<LexicalSearchMatch>> {
        let artifact_path = self.artifact_path()?;
        search_sqlite_index(
            artifact_path.absolute_path().to_path_buf(),
            query.to_string(),
            path_prefix.map(ToOwned::to_owned),
            limit,
        )
        .await
    }
}

pub(crate) fn config_fingerprint<T: Serialize>(value: &T) -> Result<String> {
    Ok(stable_hash(&serde_json::to_string(value)?))
}

async fn replace_sqlite_index(
    path: PathBuf,
    chunks: Vec<LexicalIndexChunk>,
    meta: PersistedLexicalIndexMeta,
) -> Result<()> {
    tokio::task::spawn_blocking(move || replace_sqlite_index_blocking(&path, &chunks, &meta))
        .await
        .map_err(|error| {
            crate::MemoryError::invalid(format!("memory lexical sqlite task failed: {error}"))
        })?
}

async fn refresh_sqlite_index(
    path: PathBuf,
    changed_paths: Vec<String>,
    chunks: Vec<LexicalIndexChunk>,
    meta: PersistedLexicalIndexMeta,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        refresh_sqlite_index_blocking(&path, &changed_paths, &chunks, &meta)
    })
    .await
    .map_err(|error| {
        crate::MemoryError::invalid(format!("memory lexical sqlite task failed: {error}"))
    })?
}

fn replace_sqlite_index_blocking(
    path: &Path,
    chunks: &[LexicalIndexChunk],
    meta: &PersistedLexicalIndexMeta,
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
    replace_sqlite_meta(&transaction, meta)?;
    insert_chunks(&transaction, chunks)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(())
}

fn refresh_sqlite_index_blocking(
    path: &Path,
    changed_paths: &[String],
    chunks: &[LexicalIndexChunk],
    meta: &PersistedLexicalIndexMeta,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = open_sqlite(path)?;
    let transaction = connection.transaction().map_err(map_sqlite_error)?;
    delete_chunks_for_paths(&transaction, changed_paths)?;
    replace_sqlite_meta(&transaction, meta)?;
    insert_chunks(&transaction, chunks)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(())
}

fn delete_chunks_for_paths(
    transaction: &rusqlite::Transaction<'_>,
    paths: &[String],
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut chunk_delete = transaction
        .prepare(&format!("DELETE FROM {CHUNKS_TABLE} WHERE path = ?1"))
        .map_err(map_sqlite_error)?;
    let mut fts_delete = transaction
        .prepare(&format!("DELETE FROM {FTS_TABLE} WHERE path = ?1"))
        .map_err(map_sqlite_error)?;
    for path in paths {
        chunk_delete.execute([path]).map_err(map_sqlite_error)?;
        fts_delete.execute([path]).map_err(map_sqlite_error)?;
    }
    Ok(())
}

fn insert_chunks(
    transaction: &rusqlite::Transaction<'_>,
    chunks: &[LexicalIndexChunk],
) -> Result<()> {
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
    Ok(())
}

fn replace_sqlite_meta(
    transaction: &rusqlite::Transaction<'_>,
    meta: &PersistedLexicalIndexMeta,
) -> Result<()> {
    transaction
        .execute(&format!("DELETE FROM {META_TABLE}"), [])
        .map_err(map_sqlite_error)?;
    let encoded_meta = serde_json::to_string(meta)?;
    transaction
        .execute(
            &format!("INSERT INTO {META_TABLE} (key, value) VALUES (?1, ?2)"),
            params![META_KEY, encoded_meta],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

async fn search_sqlite_index(
    path: PathBuf,
    query: String,
    path_prefix: Option<String>,
    limit: usize,
) -> Result<Vec<LexicalSearchMatch>> {
    tokio::task::spawn_blocking(move || {
        search_sqlite_index_blocking(&path, &query, path_prefix.as_deref(), limit)
    })
    .await
    .map_err(|error| {
        crate::MemoryError::invalid(format!("memory lexical sqlite task failed: {error}"))
    })?
}

fn search_sqlite_index_blocking(
    path: &Path,
    query: &str,
    path_prefix: Option<&str>,
    limit: usize,
) -> Result<Vec<LexicalSearchMatch>> {
    if !path.exists() || limit == 0 {
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
    let mut ranked = Vec::new();
    while let Some(row) = rows.next().map_err(map_sqlite_error)? {
        let path: String = row.get(1).map_err(map_sqlite_error)?;
        let raw_rank: f64 = row.get(7).map_err(map_sqlite_error)?;
        let lexical_score = bm25_rank_to_score(raw_rank);
        let score = if path == "MEMORY.md" {
            (lexical_score + 0.15).min(1.0)
        } else {
            lexical_score
        };
        let text: String = row.get(5).map_err(map_sqlite_error)?;
        let snippet: Option<String> = row.get(6).map_err(map_sqlite_error)?;
        ranked.push(LexicalSearchMatch {
            chunk_id: row.get(0).map_err(map_sqlite_error)?,
            path,
            snapshot_id: row.get(2).map_err(map_sqlite_error)?,
            start_line: row.get::<_, u32>(3).map_err(map_sqlite_error)? as usize,
            end_line: row.get::<_, u32>(4).map_err(map_sqlite_error)? as usize,
            snippet: snippet
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(text),
            score,
            raw_rank,
        });
    }
    Ok(ranked)
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
             CREATE INDEX IF NOT EXISTS idx_memory_lexical_chunks_path \
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
        return 1.0 / (1.0 + 999.0);
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
    crate::MemoryError::invalid(format!("memory lexical sqlite error: {error}"))
}

fn changed_document_paths(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut changed = current
        .iter()
        .filter(|(path, snapshot)| previous.get(*path) != Some(*snapshot))
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    changed.extend(
        previous
            .keys()
            .filter(|path| !current.contains_key(*path))
            .cloned(),
    );
    changed
}

fn chunks_for_paths(
    chunks: &[LexicalIndexChunk],
    paths: &BTreeSet<String>,
) -> Vec<LexicalIndexChunk> {
    chunks
        .iter()
        .filter(|chunk| paths.contains(&chunk.path))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{LexicalIndex, LexicalIndexChunk, changed_document_paths};
    use nanoclaw_test_support::run_current_thread_test;
    use rusqlite::Connection;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    const TEST_BACKEND: &str = "memory-test-lexical";
    const TEST_INDEX_PATH: &str = ".nanoclaw/memory/indexes/memory-test-lexical.sqlite";

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    #[test]
    fn changed_document_paths_tracks_added_removed_and_modified_documents() {
        let previous = BTreeMap::from([
            ("MEMORY.md".to_string(), "snap-a".to_string()),
            ("memory/keep.md".to_string(), "snap-b".to_string()),
            ("memory/remove.md".to_string(), "snap-c".to_string()),
        ]);
        let current = BTreeMap::from([
            ("MEMORY.md".to_string(), "snap-a2".to_string()),
            ("memory/keep.md".to_string(), "snap-b".to_string()),
            ("memory/add.md".to_string(), "snap-d".to_string()),
        ]);

        let changed = changed_document_paths(&previous, &current);

        assert_eq!(
            changed.into_iter().collect::<Vec<_>>(),
            vec![
                "MEMORY.md".to_string(),
                "memory/add.md".to_string(),
                "memory/remove.md".to_string(),
            ]
        );
    }

    bounded_async_test!(
        async fn ensure_ready_incrementally_refreshes_changed_paths_only() {
            let dir = tempdir().unwrap();
            let index = LexicalIndex::new(dir.path(), TEST_BACKEND, TEST_INDEX_PATH, 1);
            let config = "cfg-1";
            let initial_snapshots = BTreeMap::from([
                ("MEMORY.md".to_string(), "snap-a".to_string()),
                ("memory/keep.md".to_string(), "snap-b".to_string()),
                ("memory/remove.md".to_string(), "snap-c".to_string()),
            ]);
            let initial_chunks = vec![
                lexical_chunk("chunk-a", "MEMORY.md", "snap-a", 1, 1, "alpha only"),
                lexical_chunk("chunk-b", "memory/keep.md", "snap-b", 1, 1, "beta stable"),
                lexical_chunk(
                    "chunk-c",
                    "memory/remove.md",
                    "snap-c",
                    1,
                    1,
                    "gamma removed",
                ),
            ];

            let artifact = index
                .ensure_ready(config, &initial_snapshots, &initial_chunks, 0)
                .await
                .unwrap();
            let keep_rowid_before = rowid_for_chunk(artifact.absolute_path(), "chunk-b");

            let updated_snapshots = BTreeMap::from([
                ("MEMORY.md".to_string(), "snap-a2".to_string()),
                ("memory/keep.md".to_string(), "snap-b".to_string()),
                ("memory/add.md".to_string(), "snap-d".to_string()),
            ]);
            let updated_chunks = vec![
                lexical_chunk("chunk-a2", "MEMORY.md", "snap-a2", 1, 1, "alpha refreshed"),
                lexical_chunk("chunk-b", "memory/keep.md", "snap-b", 1, 1, "beta stable"),
                lexical_chunk("chunk-d", "memory/add.md", "snap-d", 1, 1, "delta added"),
            ];

            index
                .ensure_ready(config, &updated_snapshots, &updated_chunks, 0)
                .await
                .unwrap();

            assert_eq!(
                rowid_for_chunk(artifact.absolute_path(), "chunk-b"),
                keep_rowid_before
            );
            assert!(
                index
                    .search_hits("refreshed", None, 5)
                    .await
                    .unwrap()
                    .iter()
                    .any(|entry| entry.path == "MEMORY.md")
            );
            assert!(
                index
                    .search_hits("removed", None, 5)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(
                index
                    .search_hits("delta", None, 5)
                    .await
                    .unwrap()
                    .iter()
                    .any(|entry| entry.path == "memory/add.md")
            );
        }
    );

    fn lexical_chunk(
        chunk_id: &str,
        path: &str,
        snapshot_id: &str,
        start_line: usize,
        end_line: usize,
        text: &str,
    ) -> LexicalIndexChunk {
        LexicalIndexChunk {
            chunk_id: chunk_id.to_string(),
            path: path.to_string(),
            snapshot_id: snapshot_id.to_string(),
            start_line,
            end_line,
            text: text.to_string(),
        }
    }

    fn rowid_for_chunk(path: &std::path::Path, chunk_id: &str) -> i64 {
        let connection = Connection::open(path).unwrap();
        connection
            .query_row(
                "SELECT rowid FROM chunks WHERE chunk_id = ?1",
                [chunk_id],
                |row| row.get(0),
            )
            .unwrap()
    }
}
