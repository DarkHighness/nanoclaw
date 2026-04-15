use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionCompactionResult {
    pub compacted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionReviewScope {
    LatestTurn,
    SinceCheckpoint,
}

impl Default for SessionReviewScope {
    fn default() -> Self {
        Self::LatestTurn
    }
}

impl SessionReviewScope {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LatestTurn => "latest_turn",
            Self::SinceCheckpoint => "since_checkpoint",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionReviewItemKind {
    Neutral,
    Command,
    Stdout,
    Stderr,
    Diff,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SessionReviewItem {
    pub title: String,
    pub kind: SessionReviewItemKind,
    #[serde(default)]
    pub preview_lines: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SessionReviewResult {
    pub scope: SessionReviewScope,
    pub summary: String,
    pub tool_call_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<String>,
    #[serde(default)]
    pub items: Vec<SessionReviewItem>,
}

#[async_trait]
pub trait SessionControlHandler: Send + Sync {
    async fn compact_now(
        &self,
        ctx: &ToolExecutionContext,
        notes: Option<String>,
    ) -> Result<SessionCompactionResult>;

    async fn start_review(
        &self,
        ctx: &ToolExecutionContext,
        scope: SessionReviewScope,
    ) -> Result<SessionReviewResult>;
}
