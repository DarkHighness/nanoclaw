use crate::annotations::mcp_tool_annotations;
use crate::fs::{
    WriteExistingBehavior, WriteMissingBehavior, WriteRequest, apply_write, commit_text_file,
    load_optional_text_file, resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub if_exists: Option<WriteExistingBehavior>,
    #[serde(default)]
    pub if_missing: Option<WriteMissingBehavior>,
    #[serde(default)]
    pub expected_snapshot: Option<String>,
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
            description: "Create or fully replace a UTF-8 text file. Supports overwrite/create policies plus optional expected_snapshot guards when replacing an existing file.".to_string(),
            input_schema: serde_json::to_value(schema_for!(WriteToolInput)).expect("write schema"),
            output_mode: ToolOutputMode::Text,
            output_schema: None,
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
        let external_call_id = types::CallId::from(&call_id);
        let input: WriteToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            ctx.assert_path_allowed(&resolved)?;
        }

        let existing = load_optional_text_file(&resolved).await?;
        let outcome = apply_write(
            existing.as_deref(),
            &input.path,
            &WriteRequest {
                content: input.content,
                if_exists: input.if_exists.unwrap_or_default(),
                if_missing: input.if_missing.unwrap_or_default(),
                expected_snapshot: input.expected_snapshot,
            },
        );

        if outcome.is_error {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "write".to_string(),
                parts: vec![MessagePart::text(outcome.summary)],
                structured_content: None,
                metadata: Some(outcome.metadata),
                is_error: true,
            });
        }

        commit_text_file(&resolved, outcome.next_content.as_deref()).await?;
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "write".to_string(),
            parts: vec![MessagePart::text(format!(
                "{}\n[snapshot {} -> {}]",
                outcome.summary,
                outcome.snapshot_before.as_deref().unwrap_or("missing"),
                outcome.snapshot_after.as_deref().unwrap_or("missing"),
            ))],
            structured_content: None,
            metadata: Some(serde_json::json!({
                "path": resolved,
                "snapshot_before": outcome.snapshot_before,
                "snapshot_after": outcome.snapshot_after,
                "write": outcome.metadata,
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{WriteExistingBehavior, WriteMissingBehavior, WriteTool, WriteToolInput};
    use crate::{Tool, ToolExecutionContext};
    use types::ToolCallId;

    #[tokio::test]
    async fn write_tool_creates_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WriteTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WriteToolInput {
                    path: "sample.txt".to_string(),
                    content: "hello\n".to_string(),
                    if_exists: None,
                    if_missing: None,
                    expected_snapshot: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("sample.txt"))
                .await
                .unwrap(),
            "hello\n"
        );
    }

    #[tokio::test]
    async fn write_tool_can_refuse_overwrite_without_policy() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "hello\n")
            .await
            .unwrap();
        let tool = WriteTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WriteToolInput {
                    path: "sample.txt".to_string(),
                    content: "next\n".to_string(),
                    if_exists: Some(WriteExistingBehavior::Error),
                    if_missing: Some(WriteMissingBehavior::Create),
                    expected_snapshot: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("sample.txt"))
                .await
                .unwrap(),
            "hello\n"
        );
    }
}
