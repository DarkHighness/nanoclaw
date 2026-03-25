use crate::{Tool, ToolExecutionContext};
use agent_core_types::{ToolCallId, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type McpToolFuture = Pin<Box<dyn Future<Output = Result<ToolResult>> + Send>>;

#[derive(Clone)]
pub struct McpToolAdapter {
    spec: ToolSpec,
    handler: Arc<dyn Fn(ToolCallId, Value) -> McpToolFuture + Send + Sync>,
}

impl McpToolAdapter {
    #[must_use]
    pub fn new(
        spec: ToolSpec,
        handler: Arc<dyn Fn(ToolCallId, Value) -> McpToolFuture + Send + Sync>,
    ) -> Self {
        Self { spec, handler }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        (self.handler)(call_id, arguments).await
    }
}
