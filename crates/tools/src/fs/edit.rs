use crate::Result;
use crate::ToolExecutionContext;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::file_activity::FileActivityObserver;
use crate::fs::{
    TextEditOperation, apply_text_edits, commit_text_file, compute_diff_preview,
    load_optional_text_file, resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use crate::{CheckpointFileMutation, CheckpointMutationRequest};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

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

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum EditToolOutput {
    Success {
        requested_path: String,
        resolved_path: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<types::CheckpointId>,
        snapshot_before: Option<String>,
        snapshot_after: Option<String>,
        file_diffs: Vec<Value>,
        edit: Value,
    },
    Error {
        requested_path: String,
        resolved_path: String,
        summary: String,
        edit: Value,
    },
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "edit",
            "Modify an existing UTF-8 file using one precise text edit operation. Use expected_snapshot or expected_selection_hash to guard against stale reads.",
            serde_json::to_value(schema_for!(EditToolInput)).expect("edit schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(EditToolOutput)).expect("edit output schema"),
        )
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
        ctx.assert_path_write_allowed(&resolved)?;

        let existing = load_optional_text_file(&resolved).await?;
        let outcome = apply_text_edits(
            existing.as_deref(),
            &input.path,
            input.expected_snapshot.as_deref(),
            &[input.operation],
        )?;

        if outcome.is_error {
            let structured_output = EditToolOutput::Error {
                requested_path: input.path.clone(),
                resolved_path: resolved.display().to_string(),
                summary: outcome.summary.clone(),
                edit: outcome.metadata.clone(),
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "edit".into(),
                parts: vec![MessagePart::text(outcome.summary)],
                attachments: Vec::new(),
                structured_content: Some(
                    serde_json::to_value(structured_output).expect("edit error output"),
                ),
                continuation: None,
                metadata: Some(outcome.metadata),
                is_error: true,
            });
        }

        commit_text_file(&resolved, outcome.next_content.as_deref()).await?;
        let file_diffs = compute_diff_preview(
            &input.path,
            existing.as_deref(),
            outcome.next_content.as_deref(),
        )
        .into_iter()
        .collect::<Vec<_>>();
        let checkpoint = if existing.as_deref() != outcome.next_content.as_deref() {
            match &ctx.checkpoint_handler {
                Some(handler) => Some(
                    handler
                        .record_mutation(
                            ctx,
                            CheckpointMutationRequest {
                                summary: outcome.summary.clone(),
                                changed_files: vec![CheckpointFileMutation {
                                    requested_path: input.path.clone(),
                                    resolved_path: resolved.clone(),
                                    before_text: existing.clone(),
                                    after_text: outcome.next_content.clone(),
                                }],
                            },
                        )
                        .await?,
                ),
                None => None,
            }
        } else {
            None
        };
        if let Some(observer) = &self.activity_observer {
            observer.did_change(resolved.clone());
            observer.did_save(resolved.clone());
        }
        let structured_output = EditToolOutput::Success {
            requested_path: input.path.clone(),
            resolved_path: resolved.display().to_string(),
            summary: outcome.summary.clone(),
            checkpoint_id: checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            snapshot_before: outcome.snapshot_before.clone(),
            snapshot_after: outcome.snapshot_after.clone(),
            file_diffs: file_diffs.clone(),
            edit: outcome.metadata.clone(),
        };
        let mut text = format!(
            "{}\n[snapshot {} -> {}]",
            outcome.summary,
            outcome.snapshot_before.as_deref().unwrap_or("missing"),
            outcome.snapshot_after.as_deref().unwrap_or("missing"),
        );
        if let Some(previews) = diff_preview_section(&file_diffs) {
            text.push_str("\n\n[diff_preview]\n");
            text.push_str(&previews);
        }
        if let Some(checkpoint) = checkpoint.as_ref() {
            text.push_str(&format!("\n\n[checkpoint {}]", checkpoint.checkpoint_id));
        }
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "edit".into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("edit success output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "path": resolved,
                "checkpoint_id": checkpoint.as_ref().map(|checkpoint| checkpoint.checkpoint_id.clone()),
                "snapshot_before": outcome.snapshot_before,
                "snapshot_after": outcome.snapshot_after,
                "file_diffs": file_diffs,
                "edit": outcome.metadata,
            })),
            is_error: false,
        })
    }
}

fn diff_preview_section(file_diffs: &[Value]) -> Option<String> {
    let previews = file_diffs
        .iter()
        .filter_map(|entry| entry.get("preview").and_then(Value::as_str))
        .collect::<Vec<_>>();
    (!previews.is_empty()).then(|| previews.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::{EditTool, EditToolInput};
    use crate::{TextEditOperation, Tool, ToolExecutionContext, stable_text_hash};
    use nanoclaw_test_support::run_current_thread_test;
    use types::ToolCallId;

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    bounded_async_test!(
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
            let text_output = result.text_content();
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["kind"], "success");
            assert_eq!(structured["edit"]["command"], "edit");
            assert_eq!(structured["file_diffs"].as_array().map_or(0, Vec::len), 1);
            assert!(
                structured["file_diffs"][0]["preview"]
                    .as_str()
                    .unwrap()
                    .contains("+hello agent")
            );
            assert!(text_output.contains("[diff_preview]"));
            assert_eq!(
                tokio::fs::read_to_string(path).await.unwrap(),
                "hello agent\n"
            );
        }
    );

    bounded_async_test!(
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
    );

    bounded_async_test!(
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
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["kind"], "error");
            assert_eq!(
                structured["edit"]["expected_snapshot"],
                stable_text_hash("other")
            );
            assert_eq!(
                tokio::fs::read_to_string(path).await.unwrap(),
                "alpha\nbeta\n"
            );
        }
    );

    bounded_async_test!(
        async fn edit_tool_rejects_protected_workspace_state_paths() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".nanoclaw").join("state.toml");
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&path, "alpha = 1\n").await.unwrap();

            let err = EditTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(EditToolInput {
                        path: ".nanoclaw/state.toml".to_string(),
                        operation: TextEditOperation::StrReplace {
                            old_text: "1".to_string(),
                            new_text: "2".to_string(),
                            replace_all: false,
                        },
                        expected_snapshot: None,
                    })
                    .unwrap(),
                    &ToolExecutionContext {
                        workspace_root: dir.path().to_path_buf(),
                        worktree_root: Some(dir.path().to_path_buf()),
                        workspace_only: true,
                        ..Default::default()
                    },
                )
                .await
                .unwrap_err();

            assert!(err.to_string().contains("protected path"));
            assert_eq!(
                tokio::fs::read_to_string(path).await.unwrap(),
                "alpha = 1\n"
            );
        }
    );
}
