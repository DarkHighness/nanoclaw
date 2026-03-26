use crate::{
    MemoryError, MemoryStateLayout, MemoryVectorStoreConfig, MemoryVectorStoreKind,
    ResolvedStatePath, Result,
};
use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, StringBuilder, UInt32Builder};
use arrow_array::{Array, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::DistanceType;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use rusqlite::ffi::sqlite3_auto_extension;
use rusqlite::{Connection, params, params_from_iter};
use sqlite_vec::sqlite3_vec_init;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

const SQLITE_CHUNKS_TABLE: &str = "chunk_embeddings";
const SQLITE_VEC_TABLE: &str = "chunk_vectors";
const LANCEDB_CHUNKS_TABLE: &str = "chunk_embeddings";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PersistedChunkEmbedding {
    pub(crate) chunk_id: String,
    pub(crate) path: String,
    pub(crate) snapshot_id: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) text: String,
    pub(crate) embedding: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CachedMemoryEmbedIndex {
    pub(crate) config_fingerprint: String,
    pub(crate) document_snapshots: BTreeMap<String, String>,
    pub(crate) chunks: BTreeMap<String, PersistedChunkEmbedding>,
}

enum VectorStoreBackend {
    Sqlite { path: PathBuf },
    Lancedb { path: PathBuf },
}

pub(crate) struct MemoryVectorStore {
    kind: MemoryVectorStoreKind,
    location: ResolvedStatePath,
    backend: VectorStoreBackend,
}

impl MemoryVectorStore {
    pub(crate) fn from_config(
        layout: &MemoryStateLayout,
        config: &MemoryVectorStoreConfig,
    ) -> Result<Self> {
        let location = layout.resolve_vector_store_path(config.path.as_deref(), config.kind)?;
        let backend = match config.kind {
            MemoryVectorStoreKind::Sqlite => VectorStoreBackend::Sqlite {
                path: location.absolute_path().to_path_buf(),
            },
            MemoryVectorStoreKind::Lancedb => VectorStoreBackend::Lancedb {
                path: location.absolute_path().to_path_buf(),
            },
        };
        Ok(Self {
            kind: config.kind,
            location,
            backend,
        })
    }

    #[must_use]
    pub(crate) fn kind(&self) -> MemoryVectorStoreKind {
        self.kind
    }

    #[must_use]
    pub(crate) fn artifact_path(&self) -> &ResolvedStatePath {
        &self.location
    }

    pub(crate) async fn load_chunks(&self) -> Result<BTreeMap<String, PersistedChunkEmbedding>> {
        let chunks = match &self.backend {
            VectorStoreBackend::Sqlite { path } => load_sqlite_rows(path).await?,
            VectorStoreBackend::Lancedb { path } => load_lancedb_rows(path).await?,
        };
        Ok(chunks
            .into_iter()
            .map(|entry| (entry.chunk_id.clone(), entry))
            .collect())
    }

    pub(crate) async fn replace_chunks(
        &self,
        chunks: &BTreeMap<String, PersistedChunkEmbedding>,
    ) -> Result<()> {
        let rows = chunks.values().cloned().collect::<Vec<_>>();
        match &self.backend {
            VectorStoreBackend::Sqlite { path } => replace_sqlite_rows(path, &rows).await,
            VectorStoreBackend::Lancedb { path } => replace_lancedb_rows(path, &rows).await,
        }
    }

    pub(crate) async fn upsert_chunks(
        &self,
        chunks: &BTreeMap<String, PersistedChunkEmbedding>,
    ) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let rows = chunks.values().cloned().collect::<Vec<_>>();
        match &self.backend {
            VectorStoreBackend::Sqlite { path } => upsert_sqlite_rows(path, &rows).await,
            VectorStoreBackend::Lancedb { path } => upsert_lancedb_rows(path, &rows).await,
        }
    }

    pub(crate) async fn delete_chunks(&self, chunk_ids: &BTreeSet<String>) -> Result<()> {
        if chunk_ids.is_empty() {
            return Ok(());
        }
        match &self.backend {
            VectorStoreBackend::Sqlite { path } => delete_sqlite_rows(path, chunk_ids).await,
            VectorStoreBackend::Lancedb { path } => delete_lancedb_rows(path, chunk_ids).await,
        }
    }

    pub(crate) async fn search(
        &self,
        query_vector: &[f32],
        path_prefix: Option<&str>,
        limit: usize,
    ) -> Result<Option<Vec<(String, f64)>>> {
        match &self.backend {
            // sqlite-vec cannot safely combine its MATCH clause with our
            // workspace path-prefix filtering without reshaping the virtual
            // table. Prefix-filtered queries therefore stay on the shared
            // in-process fallback scorer in `memory_embed`.
            VectorStoreBackend::Sqlite { path } => {
                if path_prefix.is_some() {
                    Ok(None)
                } else {
                    search_sqlite_rows(path, query_vector, limit)
                        .await
                        .map(Some)
                }
            }
            VectorStoreBackend::Lancedb { path } => {
                search_lancedb_rows(path, query_vector, path_prefix, limit)
                    .await
                    .map(Some)
            }
        }
    }
}

async fn load_sqlite_rows(path: &Path) -> Result<Vec<PersistedChunkEmbedding>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || load_sqlite_rows_blocking(&path))
        .await
        .map_err(|error| {
            MemoryError::invalid(format!("sqlite vector store task failed: {error}"))
        })?
}

async fn replace_sqlite_rows(path: &Path, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    let path = path.to_path_buf();
    let rows = chunks.to_vec();
    tokio::task::spawn_blocking(move || replace_sqlite_rows_blocking(&path, &rows))
        .await
        .map_err(|error| {
            MemoryError::invalid(format!("sqlite vector store task failed: {error}"))
        })?
}

async fn upsert_sqlite_rows(path: &Path, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    let path = path.to_path_buf();
    let rows = chunks.to_vec();
    tokio::task::spawn_blocking(move || upsert_sqlite_rows_blocking(&path, &rows))
        .await
        .map_err(|error| {
            MemoryError::invalid(format!("sqlite vector store task failed: {error}"))
        })?
}

async fn delete_sqlite_rows(path: &Path, chunk_ids: &BTreeSet<String>) -> Result<()> {
    let path = path.to_path_buf();
    let chunk_ids = chunk_ids.iter().cloned().collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || delete_sqlite_rows_blocking(&path, &chunk_ids))
        .await
        .map_err(|error| {
            MemoryError::invalid(format!("sqlite vector store task failed: {error}"))
        })?
}

async fn search_sqlite_rows(
    path: &Path,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    let path = path.to_path_buf();
    let query = query_vector.to_vec();
    tokio::task::spawn_blocking(move || search_sqlite_rows_blocking(&path, &query, limit))
        .await
        .map_err(|error| {
            MemoryError::invalid(format!("sqlite vector store task failed: {error}"))
        })?
}

fn load_sqlite_rows_blocking(path: &Path) -> Result<Vec<PersistedChunkEmbedding>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let connection = open_sqlite(path)?;
    load_sqlite_rows_from_connection(&connection)
}

fn replace_sqlite_rows_blocking(path: &Path, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = open_sqlite(path)?;
    let transaction = connection.transaction().map_err(map_sqlite_error)?;
    transaction
        .execute(&format!("DELETE FROM {SQLITE_CHUNKS_TABLE}"), [])
        .map_err(map_sqlite_error)?;
    insert_sqlite_rows(&transaction, chunks)?;
    transaction.commit().map_err(map_sqlite_error)?;
    rebuild_sqlite_vec_index(&connection, chunks)?;
    Ok(())
}

fn upsert_sqlite_rows_blocking(path: &Path, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut connection = open_sqlite(path)?;
    let transaction = connection.transaction().map_err(map_sqlite_error)?;
    let mut statement = transaction
        .prepare(&format!(
            "INSERT INTO {SQLITE_CHUNKS_TABLE} \
             (chunk_id, path, snapshot_id, start_line, end_line, text, embedding_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(chunk_id) DO UPDATE SET \
               path = excluded.path, \
               snapshot_id = excluded.snapshot_id, \
               start_line = excluded.start_line, \
               end_line = excluded.end_line, \
               text = excluded.text, \
               embedding_json = excluded.embedding_json"
        ))
        .map_err(map_sqlite_error)?;
    for chunk in chunks {
        statement
            .execute(params![
                chunk.chunk_id,
                chunk.path,
                chunk.snapshot_id,
                chunk.start_line as u32,
                chunk.end_line as u32,
                chunk.text,
                serde_json::to_string(&chunk.embedding)?,
            ])
            .map_err(map_sqlite_error)?;
    }
    drop(statement);
    transaction.commit().map_err(map_sqlite_error)?;
    let rows = load_sqlite_rows_from_connection(&connection)?;
    rebuild_sqlite_vec_index(&connection, &rows)?;
    Ok(())
}

fn delete_sqlite_rows_blocking(path: &Path, chunk_ids: &[String]) -> Result<()> {
    if chunk_ids.is_empty() || !path.exists() {
        return Ok(());
    }
    let connection = open_sqlite(path)?;
    let placeholders = sql_placeholders(chunk_ids.len());
    let sql = format!("DELETE FROM {SQLITE_CHUNKS_TABLE} WHERE chunk_id IN ({placeholders})");
    connection
        .execute(&sql, params_from_iter(chunk_ids.iter()))
        .map_err(map_sqlite_error)?;
    let rows = load_sqlite_rows_from_connection(&connection)?;
    rebuild_sqlite_vec_index(&connection, &rows)?;
    Ok(())
}

fn search_sqlite_rows_blocking(
    path: &Path,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let connection = open_sqlite(path)?;
    let table_exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [SQLITE_VEC_TABLE],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if table_exists == 0 {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(&format!(
            "SELECT chunk_id, distance
             FROM {SQLITE_VEC_TABLE}
             WHERE embedding MATCH vec_f32(?1) AND k = ?2
             ORDER BY distance ASC"
        ))
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(
            params![serde_json::to_string(query_vector)?, limit.max(1) as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let mut results = Vec::new();
    for row in rows {
        let (chunk_id, distance) = row.map_err(map_sqlite_error)?;
        let score = 1.0 - distance;
        if score > 0.0 {
            results.push((chunk_id, score));
        }
    }
    Ok(results)
}

fn insert_sqlite_rows(
    transaction: &rusqlite::Transaction<'_>,
    chunks: &[PersistedChunkEmbedding],
) -> Result<()> {
    let mut statement = transaction
        .prepare(&format!(
            "INSERT INTO {SQLITE_CHUNKS_TABLE} \
             (chunk_id, path, snapshot_id, start_line, end_line, text, embedding_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ))
        .map_err(map_sqlite_error)?;
    for chunk in chunks {
        statement
            .execute(params![
                chunk.chunk_id,
                chunk.path,
                chunk.snapshot_id,
                chunk.start_line as u32,
                chunk.end_line as u32,
                chunk.text,
                serde_json::to_string(&chunk.embedding)?,
            ])
            .map_err(map_sqlite_error)?;
    }
    Ok(())
}

fn load_sqlite_rows_from_connection(conn: &Connection) -> Result<Vec<PersistedChunkEmbedding>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT chunk_id, path, snapshot_id, start_line, end_line, text, embedding_json \
             FROM {SQLITE_CHUNKS_TABLE} ORDER BY chunk_id"
        ))
        .map_err(map_sqlite_error)?;
    let mut rows = statement.query([]).map_err(map_sqlite_error)?;
    let mut chunks = Vec::new();
    while let Some(row) = rows.next().map_err(map_sqlite_error)? {
        let embedding_json: String = row.get(6).map_err(map_sqlite_error)?;
        chunks.push(PersistedChunkEmbedding {
            chunk_id: row.get(0).map_err(map_sqlite_error)?,
            path: row.get(1).map_err(map_sqlite_error)?,
            snapshot_id: row.get(2).map_err(map_sqlite_error)?,
            start_line: row.get::<_, u32>(3).map_err(map_sqlite_error)? as usize,
            end_line: row.get::<_, u32>(4).map_err(map_sqlite_error)? as usize,
            text: row.get(5).map_err(map_sqlite_error)?,
            embedding: serde_json::from_str(&embedding_json)?,
        });
    }
    Ok(chunks)
}

fn open_sqlite(path: &Path) -> Result<Connection> {
    register_sqlite_vec_extension();
    let connection = Connection::open(path).map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(map_sqlite_error)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(map_sqlite_error)?;
    connection
        .execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {SQLITE_CHUNKS_TABLE} (
                chunk_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                snapshot_id TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                text TEXT NOT NULL,
                embedding_json TEXT NOT NULL
            );"
        ))
        .map_err(map_sqlite_error)?;
    Ok(connection)
}

fn rebuild_sqlite_vec_index(conn: &Connection, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    conn.execute(&format!("DROP TABLE IF EXISTS {SQLITE_VEC_TABLE}"), [])
        .map_err(map_sqlite_error)?;
    if chunks.is_empty() {
        return Ok(());
    }
    let dimension = chunks
        .first()
        .map(|entry| entry.embedding.len())
        .unwrap_or_default();
    if dimension == 0 {
        return Err(MemoryError::invalid(
            "sqlite-vec backend cannot persist zero-dimension embeddings",
        ));
    }
    if chunks
        .iter()
        .any(|entry| entry.embedding.len() != dimension)
    {
        return Err(MemoryError::invalid(
            "sqlite-vec backend cannot persist mixed embedding dimensions",
        ));
    }
    conn.execute(
        &format!(
            "CREATE VIRTUAL TABLE {SQLITE_VEC_TABLE} USING vec0(
                chunk_id TEXT PRIMARY KEY,
                embedding FLOAT[{dimension}] distance_metric=cosine
            )"
        ),
        [],
    )
    .map_err(map_sqlite_error)?;
    let mut insert = conn
        .prepare(&format!(
            "INSERT INTO {SQLITE_VEC_TABLE}(chunk_id, embedding)
             VALUES (?1, vec_f32(?2))"
        ))
        .map_err(map_sqlite_error)?;
    for chunk in chunks {
        insert
            .execute(params![
                chunk.chunk_id,
                serde_json::to_string(&chunk.embedding)?,
            ])
            .map_err(map_sqlite_error)?;
    }
    Ok(())
}

async fn load_lancedb_rows(path: &Path) -> Result<Vec<PersistedChunkEmbedding>> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(Vec::new());
    }
    let db = open_lancedb(path).await?;
    if !has_lancedb_table(&db).await? {
        return Ok(Vec::new());
    }
    let table = db
        .open_table(LANCEDB_CHUNKS_TABLE)
        .execute()
        .await
        .map_err(map_lancedb_error)?;
    let batches = table
        .query()
        .execute()
        .await
        .map_err(map_lancedb_error)?
        .try_collect::<Vec<_>>()
        .await
        .map_err(map_lancedb_error)?;
    parse_lancedb_chunks(&batches)
}

async fn replace_lancedb_rows(path: &Path, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    if tokio::fs::try_exists(path).await? {
        tokio::fs::remove_dir_all(path).await?;
    }
    if chunks.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let db = open_lancedb(path).await?;
    let batch = build_lancedb_batch(chunks)?;
    db.create_table(LANCEDB_CHUNKS_TABLE, batch)
        .execute()
        .await
        .map_err(map_lancedb_error)?;
    Ok(())
}

async fn upsert_lancedb_rows(path: &Path, chunks: &[PersistedChunkEmbedding]) -> Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }
    let mut current = load_lancedb_rows(path)
        .await?
        .into_iter()
        .map(|entry| (entry.chunk_id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    for chunk in chunks {
        current.insert(chunk.chunk_id.clone(), chunk.clone());
    }
    let rows = current.into_values().collect::<Vec<_>>();
    replace_lancedb_rows(path, &rows).await
}

async fn delete_lancedb_rows(path: &Path, chunk_ids: &BTreeSet<String>) -> Result<()> {
    if chunk_ids.is_empty() {
        return Ok(());
    }
    let rows = load_lancedb_rows(path)
        .await?
        .into_iter()
        .filter(|entry| !chunk_ids.contains(&entry.chunk_id))
        .collect::<Vec<_>>();
    replace_lancedb_rows(path, &rows).await
}

async fn search_lancedb_rows(
    path: &Path,
    query_vector: &[f32],
    path_prefix: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, f64)>> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(Vec::new());
    }
    let db = open_lancedb(path).await?;
    if !has_lancedb_table(&db).await? {
        return Ok(Vec::new());
    }
    let table = db
        .open_table(LANCEDB_CHUNKS_TABLE)
        .execute()
        .await
        .map_err(map_lancedb_error)?;
    let mut query = table
        .vector_search(query_vector)
        .map_err(map_lancedb_error)?
        .distance_type(DistanceType::Cosine)
        .bypass_vector_index()
        .limit(limit.max(1))
        .select(Select::columns(&["chunk_id", "_distance"]));
    if let Some(prefix) = path_prefix.filter(|value| !value.is_empty()) {
        query = query.only_if(format!(
            "path LIKE '{}%' ESCAPE '\\\\'",
            escape_sql_like_prefix(prefix)
        ));
    }
    let batches = query
        .execute()
        .await
        .map_err(map_lancedb_error)?
        .try_collect::<Vec<_>>()
        .await
        .map_err(map_lancedb_error)?;
    parse_lancedb_search_results(&batches)
}

fn build_lancedb_batch(chunks: &[PersistedChunkEmbedding]) -> Result<RecordBatch> {
    let dims = chunks
        .first()
        .map(|entry| entry.embedding.len())
        .ok_or_else(|| MemoryError::invalid("cannot encode empty LanceDB snapshot"))?;
    if dims == 0 {
        return Err(MemoryError::invalid(
            "cannot encode LanceDB snapshot with zero-dimension vectors",
        ));
    }
    if chunks.iter().any(|entry| entry.embedding.len() != dims) {
        return Err(MemoryError::invalid(
            "all embeddings in a LanceDB snapshot must share the same dimension",
        ));
    }

    let mut chunk_id_builder = StringBuilder::new();
    let mut path_builder = StringBuilder::new();
    let mut snapshot_builder = StringBuilder::new();
    let mut start_line_builder = UInt32Builder::new();
    let mut end_line_builder = UInt32Builder::new();
    let mut text_builder = StringBuilder::new();
    let mut embedding_builder = FixedSizeListBuilder::new(
        Float32Builder::new(),
        dims.try_into()
            .map_err(|_| MemoryError::invalid("embedding dimension does not fit LanceDB schema"))?,
    );

    for chunk in chunks {
        chunk_id_builder.append_value(&chunk.chunk_id);
        path_builder.append_value(&chunk.path);
        snapshot_builder.append_value(&chunk.snapshot_id);
        start_line_builder.append_value(chunk.start_line as u32);
        end_line_builder.append_value(chunk.end_line as u32);
        text_builder.append_value(&chunk.text);
        for value in &chunk.embedding {
            embedding_builder.values().append_value(*value);
        }
        embedding_builder.append(true);
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("snapshot_id", DataType::Utf8, false),
        Field::new("start_line", DataType::UInt32, false),
        Field::new("end_line", DataType::UInt32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dims as i32,
            ),
            false,
        ),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(chunk_id_builder.finish()),
            Arc::new(path_builder.finish()),
            Arc::new(snapshot_builder.finish()),
            Arc::new(start_line_builder.finish()),
            Arc::new(end_line_builder.finish()),
            Arc::new(text_builder.finish()),
            Arc::new(embedding_builder.finish()),
        ],
    )
    .map_err(|error| MemoryError::invalid(format!("failed to build LanceDB record batch: {error}")))
}

fn parse_lancedb_chunks(batches: &[RecordBatch]) -> Result<Vec<PersistedChunkEmbedding>> {
    let mut rows = Vec::new();
    for batch in batches {
        rows.extend(decode_record_batch(batch)?);
    }
    Ok(rows)
}

fn decode_record_batch(batch: &RecordBatch) -> Result<Vec<PersistedChunkEmbedding>> {
    let chunk_ids = string_column(batch, "chunk_id")?;
    let paths = string_column(batch, "path")?;
    let snapshot_ids = string_column(batch, "snapshot_id")?;
    let start_lines = u32_column(batch, "start_line")?;
    let end_lines = u32_column(batch, "end_line")?;
    let texts = string_column(batch, "text")?;
    let vectors = batch
        .column_by_name("vector")
        .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
        .ok_or_else(|| MemoryError::invalid("LanceDB batch missing `vector` column"))?;
    let values = vectors
        .values()
        .as_any()
        .downcast_ref::<Float32Array>()
        .ok_or_else(|| MemoryError::invalid("LanceDB vector column is not Float32"))?;
    let dims = vectors.value_length() as usize;
    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let start = row.saturating_mul(dims);
        let end = start.saturating_add(dims);
        rows.push(PersistedChunkEmbedding {
            chunk_id: chunk_ids.value(row).to_string(),
            path: paths.value(row).to_string(),
            snapshot_id: snapshot_ids.value(row).to_string(),
            start_line: start_lines.value(row) as usize,
            end_line: end_lines.value(row) as usize,
            text: texts.value(row).to_string(),
            embedding: (start..end).map(|index| values.value(index)).collect(),
        });
    }
    Ok(rows)
}

fn parse_lancedb_search_results(batches: &[RecordBatch]) -> Result<Vec<(String, f64)>> {
    let mut results = Vec::new();
    for batch in batches {
        let chunk_ids = batch
            .column_by_name("chunk_id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or_else(|| MemoryError::invalid("LanceDB batch missing `chunk_id` column"))?;
        let distances = batch
            .column_by_name("_distance")
            .and_then(|column| column.as_any().downcast_ref::<Float32Array>())
            .ok_or_else(|| MemoryError::invalid("LanceDB batch missing `_distance` column"))?;
        for row in 0..batch.num_rows() {
            let score = 1.0 - f64::from(distances.value(row));
            if score > 0.0 {
                results.push((chunk_ids.value(row).to_string(), score));
            }
        }
    }
    Ok(results)
}

async fn open_lancedb(path: &Path) -> Result<lancedb::Connection> {
    lancedb::connect(path.to_string_lossy().as_ref())
        .execute()
        .await
        .map_err(map_lancedb_error)
}

async fn has_lancedb_table(db: &lancedb::Connection) -> Result<bool> {
    Ok(db
        .table_names()
        .execute()
        .await
        .map_err(map_lancedb_error)?
        .iter()
        .any(|name| name == LANCEDB_CHUNKS_TABLE))
}

fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| MemoryError::invalid(format!("LanceDB batch missing `{name}` column")))
}

fn u32_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt32Array> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
        .ok_or_else(|| MemoryError::invalid(format!("LanceDB batch missing `{name}` column")))
}

fn sql_placeholders(count: usize) -> String {
    let mut out = String::new();
    for index in 0..count {
        if index > 0 {
            out.push_str(", ");
        }
        out.push('?');
    }
    out
}

fn register_sqlite_vec_extension() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        // sqlite-vec exposes a loadable-extension entrypoint. Register it once
        // for the process so every later `Connection::open` can create/use the
        // `vec0` virtual table without per-connection dynamic loading.
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    });
}

fn escape_sql_like_prefix(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .replace('\'', "''")
}

fn map_sqlite_error(error: rusqlite::Error) -> MemoryError {
    MemoryError::invalid(format!("sqlite vector store error: {error}"))
}

fn map_lancedb_error(error: lancedb::Error) -> MemoryError {
    MemoryError::invalid(format!("lancedb vector store error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{MemoryVectorStore, PersistedChunkEmbedding};
    use crate::{MemoryStateLayout, MemoryVectorStoreConfig, MemoryVectorStoreKind};
    use std::collections::{BTreeMap, BTreeSet};
    use tempfile::tempdir;

    fn sample_chunks() -> BTreeMap<String, PersistedChunkEmbedding> {
        BTreeMap::from([
            (
                "chunk-a".to_string(),
                PersistedChunkEmbedding {
                    chunk_id: "chunk-a".to_string(),
                    path: "MEMORY.md".to_string(),
                    snapshot_id: "snap-a".to_string(),
                    start_line: 1,
                    end_line: 3,
                    text: "semantic rollout".to_string(),
                    embedding: vec![1.0, 0.0, 0.0],
                },
            ),
            (
                "chunk-b".to_string(),
                PersistedChunkEmbedding {
                    chunk_id: "chunk-b".to_string(),
                    path: "memory/other.md".to_string(),
                    snapshot_id: "snap-b".to_string(),
                    start_line: 4,
                    end_line: 6,
                    text: "fallback procedure".to_string(),
                    embedding: vec![0.0, 1.0, 0.0],
                },
            ),
        ])
    }

    #[tokio::test]
    async fn sqlite_supports_replace_upsert_delete() {
        let dir = tempdir().unwrap();
        let layout = MemoryStateLayout::new(dir.path());
        let store =
            MemoryVectorStore::from_config(&layout, &MemoryVectorStoreConfig::default()).unwrap();
        let mut chunks = sample_chunks();

        store.replace_chunks(&chunks).await.unwrap();
        let hits = store.search(&[1.0, 0.0, 0.0], None, 2).await.unwrap();
        assert_eq!(
            hits.as_ref()
                .and_then(|rows| rows.first())
                .map(|entry| entry.0.as_str()),
            Some("chunk-a")
        );

        chunks.insert(
            "chunk-c".to_string(),
            PersistedChunkEmbedding {
                chunk_id: "chunk-c".to_string(),
                path: "memory/new.md".to_string(),
                snapshot_id: "snap-c".to_string(),
                start_line: 1,
                end_line: 2,
                text: "new note".to_string(),
                embedding: vec![0.0, 0.0, 1.0],
            },
        );
        let mut only_new = BTreeMap::new();
        only_new.insert("chunk-c".to_string(), chunks["chunk-c"].clone());
        store.upsert_chunks(&only_new).await.unwrap();
        assert_eq!(store.load_chunks().await.unwrap().len(), 3);

        let mut delete = BTreeSet::new();
        delete.insert("chunk-b".to_string());
        store.delete_chunks(&delete).await.unwrap();
        let remaining = store.load_chunks().await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.contains_key("chunk-b"));
    }

    #[tokio::test]
    async fn lancedb_supports_replace_upsert_delete() {
        let dir = tempdir().unwrap();
        let layout = MemoryStateLayout::new(dir.path());
        let store = MemoryVectorStore::from_config(
            &layout,
            &MemoryVectorStoreConfig {
                kind: MemoryVectorStoreKind::Lancedb,
                path: None,
            },
        )
        .unwrap();
        let mut chunks = sample_chunks();

        store.replace_chunks(&chunks).await.unwrap();
        let hits = store
            .search(&[1.0, 0.0, 0.0], Some("MEMORY"), 2)
            .await
            .unwrap();
        assert_eq!(
            hits.as_ref()
                .and_then(|rows| rows.first())
                .map(|entry| entry.0.as_str()),
            Some("chunk-a")
        );

        chunks.insert(
            "chunk-c".to_string(),
            PersistedChunkEmbedding {
                chunk_id: "chunk-c".to_string(),
                path: "memory/new.md".to_string(),
                snapshot_id: "snap-c".to_string(),
                start_line: 1,
                end_line: 2,
                text: "new note".to_string(),
                embedding: vec![0.0, 0.0, 1.0],
            },
        );
        let mut only_new = BTreeMap::new();
        only_new.insert("chunk-c".to_string(), chunks["chunk-c"].clone());
        store.upsert_chunks(&only_new).await.unwrap();
        assert_eq!(store.load_chunks().await.unwrap().len(), 3);

        let mut delete = BTreeSet::new();
        delete.insert("chunk-b".to_string());
        store.delete_chunks(&delete).await.unwrap();
        let remaining = store.load_chunks().await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.contains_key("chunk-b"));
    }
}
