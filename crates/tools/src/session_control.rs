use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionCompactionResult {
    pub compacted: bool,
}

#[async_trait]
pub trait SessionControlHandler: Send + Sync {
    async fn compact_now(
        &self,
        ctx: &ToolExecutionContext,
        notes: Option<String>,
    ) -> Result<SessionCompactionResult>;
}
