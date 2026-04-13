use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use std::path::PathBuf;
use types::{CheckpointId, CheckpointRecord, CheckpointRestoreMode, CheckpointRestoreRecord};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointFileMutation {
    pub requested_path: String,
    pub resolved_path: PathBuf,
    pub before_text: Option<String>,
    pub after_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointMutationRequest {
    pub summary: String,
    pub changed_files: Vec<CheckpointFileMutation>,
}

#[async_trait]
pub trait CheckpointHandler: Send + Sync {
    async fn record_mutation(
        &self,
        ctx: &ToolExecutionContext,
        request: CheckpointMutationRequest,
    ) -> Result<CheckpointRecord>;

    async fn list_checkpoints(&self, ctx: &ToolExecutionContext) -> Result<Vec<CheckpointRecord>>;

    async fn restore_checkpoint(
        &self,
        ctx: &ToolExecutionContext,
        checkpoint_id: &CheckpointId,
        mode: CheckpointRestoreMode,
    ) -> Result<CheckpointRestoreRecord>;
}
