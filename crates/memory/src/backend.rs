use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    pub limit: Option<usize>,
    pub path_prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchHit {
    pub hit_id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    pub snippet: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchResponse {
    pub backend: String,
    pub hits: Vec<MemorySearchHit>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryGetRequest {
    pub path: String,
    pub start_line: Option<usize>,
    pub line_count: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDocument {
    pub path: String,
    pub snapshot_id: String,
    pub requested_start_line: usize,
    pub resolved_start_line: usize,
    pub resolved_end_line: usize,
    pub total_lines: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySyncStatus {
    pub backend: String,
    pub indexed_documents: usize,
    pub indexed_lines: usize,
}

#[async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn sync(&self) -> Result<MemorySyncStatus>;
    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse>;
    async fn get(&self, req: MemoryGetRequest) -> Result<MemoryDocument>;
}
