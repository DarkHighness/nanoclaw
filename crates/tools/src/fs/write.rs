use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::file_activity::FileActivityObserver;
use crate::fs::{
    WriteExistingBehavior, WriteMissingBehavior, WriteRequest, apply_write, commit_text_file,
    compute_diff_preview, load_optional_text_file, resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

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

#[derive(Clone, Default)]
pub struct WriteTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

impl WriteTool {
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
enum WriteToolOutput {
    Success {
        requested_path: String,
        resolved_path: String,
        summary: String,
        snapshot_before: Option<String>,
        snapshot_after: Option<String>,
        file_diffs: Vec<Value>,
        write: Value,
    },
    Error {
        requested_path: String,
        resolved_path: String,
        summary: String,
        write: Value,
    },
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "write",
            "Create or fully replace a UTF-8 text file. Supports overwrite/create policies plus optional expected_snapshot guards when replacing an existing file.",
            serde_json::to_value(schema_for!(WriteToolInput)).expect("write schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(WriteToolOutput)).expect("write output schema"),
        )
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
        ctx.assert_path_write_allowed(&resolved)?;

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
            let structured_output = WriteToolOutput::Error {
                requested_path: input.path.clone(),
                resolved_path: resolved.display().to_string(),
                summary: outcome.summary.clone(),
                write: outcome.metadata.clone(),
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "write".into(),
                parts: vec![MessagePart::text(outcome.summary)],
                structured_content: Some(
                    serde_json::to_value(structured_output).expect("write error output"),
                ),
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
        if let Some(observer) = &self.activity_observer {
            observer.did_change(resolved.clone());
            observer.did_save(resolved.clone());
        }
        let structured_output = WriteToolOutput::Success {
            requested_path: input.path.clone(),
            resolved_path: resolved.display().to_string(),
            summary: outcome.summary.clone(),
            snapshot_before: outcome.snapshot_before.clone(),
            snapshot_after: outcome.snapshot_after.clone(),
            file_diffs: file_diffs.clone(),
            write: outcome.metadata.clone(),
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
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "write".into(),
            parts: vec![MessagePart::text(text)],
            structured_content: Some(
                serde_json::to_value(structured_output).expect("write success output"),
            ),
            metadata: Some(serde_json::json!({
                "path": resolved,
                "snapshot_before": outcome.snapshot_before,
                "snapshot_after": outcome.snapshot_after,
                "file_diffs": file_diffs,
                "write": outcome.metadata,
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
    use super::{WriteExistingBehavior, WriteMissingBehavior, WriteTool, WriteToolInput};
    use crate::{Tool, ToolExecutionContext};
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
            let text_output = result.text_content();
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["kind"], "success");
            assert_eq!(structured["write"]["created"], true);
            assert_eq!(structured["file_diffs"].as_array().map_or(0, Vec::len), 1);
            assert!(
                structured["file_diffs"][0]["preview"]
                    .as_str()
                    .unwrap()
                    .contains("+++ sample.txt")
            );
            assert!(text_output.contains("[diff_preview]"));
            assert_eq!(
                tokio::fs::read_to_string(dir.path().join("sample.txt"))
                    .await
                    .unwrap(),
                "hello\n"
            );
        }
    );

    bounded_async_test!(
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
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["kind"], "error");
            assert_eq!(structured["write"]["if_exists"], "error");
            assert_eq!(
                tokio::fs::read_to_string(dir.path().join("sample.txt"))
                    .await
                    .unwrap(),
                "hello\n"
            );
        }
    );

    bounded_async_test!(
        async fn write_tool_rejects_protected_workspace_state_paths() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::create_dir_all(dir.path().join(".nanoclaw"))
                .await
                .unwrap();
            let tool = WriteTool::new();

            let err = tool
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(WriteToolInput {
                        path: ".nanoclaw/state.toml".to_string(),
                        content: "x = 1\n".to_string(),
                        if_exists: None,
                        if_missing: None,
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
            assert!(
                !tokio::fs::try_exists(dir.path().join(".nanoclaw/state.toml"))
                    .await
                    .unwrap()
            );
        }
    );
}
