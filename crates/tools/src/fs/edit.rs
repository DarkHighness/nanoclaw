use crate::Result;
use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::file_activity::FileActivityObserver;
use crate::fs::{
    TextEditOperation, apply_text_edits, commit_text_file, load_optional_text_file,
    resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct EditToolInput {
    pub path: String,
    pub operation: TextEditOperation,
    #[serde(default)]
    pub expected_snapshot: Option<String>,
}

#[derive(Clone, Default)]
pub struct EditTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

impl EditTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            activity_observer: None,
        }
    }

    #[must_use]
    pub fn with_file_activity_observer(activity_observer: Arc<dyn FileActivityObserver>) -> Self {
        Self {
            activity_observer: Some(activity_observer),
        }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
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
        let external_call_id = types::CallId::from(&call_id);
        let input: EditToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            ctx.assert_path_allowed(&resolved)?;
        }

        let existing = load_optional_text_file(&resolved).await?;
        let outcome = apply_text_edits(
            existing.as_deref(),
            &input.path,
            input.expected_snapshot.as_deref(),
            &[input.operation],
        )?;

        if outcome.is_error {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "edit".into(),
                parts: vec![MessagePart::text(outcome.summary)],
                metadata: Some(outcome.metadata),
                is_error: true,
            });
        }

        commit_text_file(&resolved, outcome.next_content.as_deref()).await?;
        if let Some(observer) = &self.activity_observer {
            observer.did_change(resolved.clone());
        }
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "edit".into(),
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
    use types::ToolCallId;

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
                    operation: TextEditOperation::StrReplace {
                        old_text: "world".to_string(),
                        new_text: "agent".to_string(),
                        replace_all: false,
                    },
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
                    operation: TextEditOperation::ReplaceLines {
                        start_line: 2,
                        end_line: 3,
                        text: "middle\ntail".to_string(),
                        expected_selection_hash: None,
                    },
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
                    operation: TextEditOperation::StrReplace {
                        old_text: "beta".to_string(),
                        new_text: "gamma".to_string(),
                        replace_all: false,
                    },
                    expected_snapshot: Some(stable_text_hash("other")),
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
            tokio::fs::read_to_string(path).await.unwrap(),
            "alpha\nbeta\n"
        );
    }
}
