use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{assert_path_inside_root, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default)]
pub struct WriteTool;

impl WriteTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_string(),
            description: "Write content to a file. Creates parent directories and overwrites any existing file.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WriteToolInput)).expect("write schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Write File", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: WriteToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            assert_path_inside_root(&resolved, ctx.effective_root())?;
        }
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&resolved, input.content.as_bytes()).await?;
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "write".to_string(),
            parts: vec![MessagePart::text(format!(
                "Successfully wrote {} bytes to {}",
                input.content.len(),
                input.path
            ))],
            metadata: Some(serde_json::json!({ "path": resolved })),
            is_error: false,
        })
    }
}
