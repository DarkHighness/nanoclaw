use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use types::{ArtifactId, ArtifactKind, ArtifactVersionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeCommandStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCommandSpec {
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCommandTrace {
    pub argv: Vec<String>,
    pub status: WorktreeCommandStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorktreeMutation {
    WriteFile {
        relative_path: PathBuf,
        content: String,
    },
    RemoveFile {
        relative_path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRunTrace {
    pub artifact_id: ArtifactId,
    pub version_id: ArtifactVersionId,
    pub artifact_kind: ArtifactKind,
    pub baseline_ref: String,
    pub worktree_path: PathBuf,
    #[serde(default)]
    pub mutations: Vec<WorktreeMutation>,
    #[serde(default)]
    pub commands: Vec<WorktreeCommandTrace>,
    pub git_diff: String,
    pub cleanup_performed: bool,
}
