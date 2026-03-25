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
pub struct EditToolInput {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub replace_all: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct EditTool;

impl EditTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Apply an exact text replacement inside a UTF-8 file. Fails if the old text is missing or ambiguous unless replace_all=true.".to_string(),
            input_schema: serde_json::to_value(schema_for!(EditToolInput)).expect("edit schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Edit File", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: EditToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            assert_path_inside_root(&resolved, ctx.effective_root())?;
        }

        let content = fs::read_to_string(&resolved).await?;
        let occurrences = content.matches(&input.old_text).count();
        if occurrences == 0 {
            return Ok(ToolResult::error(
                call_id,
                "edit",
                format!("No exact match found in {}", input.path),
            ));
        }
        if occurrences > 1 && !input.replace_all.unwrap_or(false) {
            return Ok(ToolResult::error(
                call_id,
                "edit",
                format!(
                    "Found {occurrences} matches in {}. Re-run with replace_all=true or provide a more specific old_text",
                    input.path
                ),
            ));
        }

        let updated = if input.replace_all.unwrap_or(false) {
            content.replace(&input.old_text, &input.new_text)
        } else {
            content.replacen(&input.old_text, &input.new_text, 1)
        };
        fs::write(&resolved, updated.as_bytes()).await?;

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "edit".to_string(),
            parts: vec![MessagePart::text(format!(
                "Edited {} ({} replacement{})",
                input.path,
                if input.replace_all.unwrap_or(false) {
                    occurrences
                } else {
                    1
                },
                if input.replace_all.unwrap_or(false) && occurrences != 1 {
                    "s"
                } else {
                    ""
                }
            ))],
            metadata: Some(serde_json::json!({
                "path": resolved,
                "occurrences": occurrences,
                "replace_all": input.replace_all.unwrap_or(false),
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{EditTool, EditToolInput};
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;

    #[tokio::test]
    async fn edit_tool_replaces_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, "hello world\n").await.unwrap();

        let tool = EditTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(EditToolInput {
                    path: "sample.txt".to_string(),
                    old_text: "world".to_string(),
                    new_text: "agent".to_string(),
                    replace_all: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    sandbox_root: None,
                    workspace_only: true,
                    container_workdir: None,
                    model_context_window_tokens: None,
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "hello agent\n"
        );
    }
}
