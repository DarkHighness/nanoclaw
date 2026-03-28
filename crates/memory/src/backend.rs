use crate::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use types::{RunId, SessionId};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryScope {
    Procedural,
    #[default]
    Semantic,
    Episodic,
    Working,
    Coordination,
}

impl MemoryScope {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Procedural => "procedural",
            Self::Semantic => "semantic",
            Self::Episodic => "episodic",
            Self::Working => "working",
            Self::Coordination => "coordination",
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryStatus {
    #[default]
    Ready,
    Stale,
    Superseded,
    Archived,
}

impl MemoryStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Superseded => "superseded",
            Self::Archived => "archived",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDocumentMetadata {
    #[serde(default)]
    pub scope: MemoryScope,
    #[serde(default)]
    pub layer: String,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
    #[serde(default)]
    pub promoted_from: Option<String>,
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: MemoryStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    pub limit: Option<usize>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<MemoryScope>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub include_stale: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchHit {
    pub hit_id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    pub snippet: String,
    pub document_metadata: MemoryDocumentMetadata,
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
    pub title: String,
    pub requested_start_line: usize,
    pub resolved_start_line: usize,
    pub resolved_end_line: usize,
    pub total_lines: usize,
    pub text: String,
    pub metadata: MemoryDocumentMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySyncStatus {
    pub backend: String,
    pub indexed_documents: usize,
    pub indexed_lines: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryListRequest {
    pub limit: Option<usize>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<MemoryScope>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub include_stale: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryListEntry {
    pub path: String,
    pub title: String,
    pub snapshot_id: String,
    pub total_lines: usize,
    pub metadata: MemoryDocumentMetadata,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryListResponse {
    pub entries: Vec<MemoryListEntry>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecordRequest {
    pub scope: MemoryScope,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub run_id: Option<RunId>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPromoteRequest {
    pub source_path: String,
    pub target_scope: MemoryScope,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryForgetRequest {
    pub path: String,
    pub status: MemoryStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryMutationResponse {
    pub action: String,
    pub path: String,
    pub snapshot_id: String,
    pub metadata: MemoryDocumentMetadata,
}

#[async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn sync(&self) -> Result<MemorySyncStatus>;
    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse>;
    async fn get(&self, req: MemoryGetRequest) -> Result<MemoryDocument>;
    async fn list(&self, req: MemoryListRequest) -> Result<MemoryListResponse>;
    async fn record(&self, req: MemoryRecordRequest) -> Result<MemoryMutationResponse>;
    async fn promote(&self, req: MemoryPromoteRequest) -> Result<MemoryMutationResponse>;
    async fn forget(&self, req: MemoryForgetRequest) -> Result<MemoryMutationResponse>;
}
