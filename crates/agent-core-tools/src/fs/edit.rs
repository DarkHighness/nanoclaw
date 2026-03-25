use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{
    TextEditOperation, apply_text_edits, assert_path_inside_root, commit_text_file,
    load_optional_text_file, resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct EditToolInput {
    pub path: String,
    #[serde(default)]
    pub operation: Option<TextEditOperation>,
    #[serde(default)]
    pub old_text: Option<String>,
    #[serde(default)]
    pub new_text: Option<String>,
    #[serde(default)]
    pub replace_all: Option<bool>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default, alias = "after_line")]
    pub insert_line: Option<usize>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub expected_snapshot: Option<String>,
    #[serde(default)]
    pub expected_selection_hash: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EditTool;

impl EditTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn resolve_operation(input: &EditToolInput) -> Result<TextEditOperation> {
    if let Some(operation) = &input.operation {
        return Ok(operation.clone());
    }
    if input.old_text.is_some() || input.new_text.is_some() {
        return Ok(TextEditOperation::StrReplace {
            old_text: input
                .old_text
                .clone()
                .ok_or_else(|| anyhow!("str_replace requires old_text"))?,
            new_text: input
                .new_text
                .clone()
                .ok_or_else(|| anyhow!("str_replace requires new_text"))?,
            replace_all: input.replace_all.unwrap_or(false),
        });
    }
    if input.start_line.is_some() || input.end_line.is_some() {
        return Ok(TextEditOperation::ReplaceLines {
            start_line: input
                .start_line
                .ok_or_else(|| anyhow!("replace_lines requires start_line"))?,
            end_line: input
                .end_line
                .ok_or_else(|| anyhow!("replace_lines requires end_line"))?,
            text: input
                .text
                .clone()
                .or_else(|| input.new_text.clone())
                .ok_or_else(|| anyhow!("replace_lines requires text"))?,
            expected_selection_hash: input.expected_selection_hash.clone(),
        });
    }
    if input.insert_line.is_some() {
        return Ok(TextEditOperation::Insert {
            after_line: input
                .insert_line
                .ok_or_else(|| anyhow!("insert requires insert_line"))?,
            text: input
                .text
                .clone()
                .or_else(|| input.new_text.clone())
                .ok_or_else(|| anyhow!("insert requires text"))?,
        });
    }
    bail!("edit requires operation, or legacy old_text/new_text, line-range, or insert fields")
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Modify an existing UTF-8 file using one precise text edit operation. Use expected_snapshot or expected_selection_hash to guard against stale reads.".to_string(),
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

        let existing = load_optional_text_file(&resolved).await?;
        let operation = resolve_operation(&input)?;
        let outcome = apply_text_edits(
            existing.as_deref(),
            &input.path,
            input.expected_snapshot.as_deref(),
            &[operation],
        )?;

        if outcome.is_error {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "edit".to_string(),
                parts: vec![MessagePart::text(outcome.summary)],
                metadata: Some(outcome.metadata),
                is_error: true,
            });
        }

        commit_text_file(&resolved, outcome.next_content.as_deref()).await?;
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "edit".to_string(),
            parts: vec![MessagePart::text(format!(
                "{}\n[snapshot {} -> {}]",
                outcome.summary,
                outcome.snapshot_before.as_deref().unwrap_or("missing"),
                outcome.snapshot_after.as_deref().unwrap_or("missing"),
            ))],
            metadata: Some(serde_json::json!({
                "path": resolved,
                "snapshot_before": outcome.snapshot_before,
                "snapshot_after": outcome.snapshot_after,
                "edit": outcome.metadata,
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{EditTool, EditToolInput};
    use crate::{TextEditOperation, Tool, ToolExecutionContext, stable_text_hash};
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
                    operation: Some(TextEditOperation::StrReplace {
                        old_text: "world".to_string(),
                        new_text: "agent".to_string(),
                        replace_all: false,
                    }),
                    old_text: None,
                    new_text: None,
                    replace_all: None,
                    start_line: None,
                    end_line: None,
                    insert_line: None,
                    text: None,
                    expected_snapshot: None,
                    expected_selection_hash: None,
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

    #[tokio::test]
    async fn edit_tool_replaces_line_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let tool = EditTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(EditToolInput {
                    path: "sample.txt".to_string(),
                    operation: Some(TextEditOperation::ReplaceLines {
                        start_line: 2,
                        end_line: 3,
                        text: "middle\ntail".to_string(),
                        expected_selection_hash: None,
                    }),
                    old_text: None,
                    new_text: None,
                    replace_all: None,
                    start_line: None,
                    end_line: None,
                    insert_line: None,
                    text: None,
                    expected_snapshot: None,
                    expected_selection_hash: None,
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
            "alpha\nmiddle\ntail\n"
        );
    }

    #[tokio::test]
    async fn edit_tool_checks_snapshot_guards() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();

        let tool = EditTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(EditToolInput {
                    path: "sample.txt".to_string(),
                    operation: Some(TextEditOperation::StrReplace {
                        old_text: "beta".to_string(),
                        new_text: "gamma".to_string(),
                        replace_all: false,
                    }),
                    old_text: None,
                    new_text: None,
                    replace_all: None,
                    start_line: None,
                    end_line: None,
                    insert_line: None,
                    text: None,
                    expected_snapshot: Some(stable_text_hash("other")),
                    expected_selection_hash: None,
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

        assert!(result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "alpha\nbeta\n"
        );
    }
}
