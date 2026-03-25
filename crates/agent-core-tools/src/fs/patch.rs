use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{
    TextEditOperation, WriteExistingBehavior, WriteMissingBehavior, WriteRequest, apply_delete,
    apply_text_edits, apply_write, assert_path_inside_root, commit_text_file,
    load_optional_text_file, resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::{Result, bail};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum PatchOperation {
    Write {
        path: String,
        content: String,
        #[serde(default)]
        if_exists: Option<WriteExistingBehavior>,
        #[serde(default)]
        if_missing: Option<WriteMissingBehavior>,
        #[serde(default)]
        expected_snapshot: Option<String>,
    },
    Edit {
        path: String,
        edits: Vec<TextEditOperation>,
        #[serde(default)]
        expected_snapshot: Option<String>,
    },
    Delete {
        path: String,
        #[serde(default)]
        expected_snapshot: Option<String>,
        #[serde(default)]
        ignore_missing: Option<bool>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PatchToolInput {
    pub operations: Vec<PatchOperation>,
}

#[derive(Clone, Debug, Default)]
pub struct PatchTool;

#[derive(Clone, Debug)]
struct StagedFile {
    display_path: String,
    resolved_path: PathBuf,
    content: Option<String>,
    touched: bool,
}

impl PatchTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl PatchOperation {
    fn path(&self) -> &str {
        match self {
            Self::Write { path, .. } | Self::Edit { path, .. } | Self::Delete { path, .. } => path,
        }
    }
}

#[async_trait]
impl Tool for PatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "patch".to_string(),
            description: "Apply a staged multi-file patch made of write, edit, and delete operations. Operations are validated against staged content first so a failed operation does not partially apply earlier changes.".to_string(),
            input_schema: serde_json::to_value(schema_for!(PatchToolInput)).expect("patch schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Apply Patch", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: PatchToolInput = serde_json::from_value(arguments)?;
        if input.operations.is_empty() {
            bail!("patch requires at least one operation");
        }

        let mut staged = BTreeMap::<PathBuf, StagedFile>::new();
        let mut operation_summaries = Vec::with_capacity(input.operations.len());
        let mut operation_metadata = Vec::with_capacity(input.operations.len());

        // Stage mutations in memory first so a failing later operation does not partially
        // commit an earlier file change. Later operations against the same path observe the
        // already-staged content, which matches normal patch application semantics.
        for operation in &input.operations {
            let display_path = operation.path().to_string();
            let resolved = resolve_tool_path_against_workspace_root(
                &display_path,
                ctx.effective_root(),
                ctx.container_workdir.as_deref(),
            )?;
            if ctx.workspace_only {
                assert_path_inside_root(&resolved, ctx.effective_root())?;
            }

            let mut entry = match staged.remove(&resolved) {
                Some(entry) => entry,
                None => StagedFile {
                    display_path: display_path.clone(),
                    resolved_path: resolved.clone(),
                    content: load_optional_text_file(&resolved).await?,
                    touched: false,
                },
            };

            let outcome = match operation {
                PatchOperation::Write {
                    content,
                    if_exists,
                    if_missing,
                    expected_snapshot,
                    ..
                } => apply_write(
                    entry.content.as_deref(),
                    &display_path,
                    &WriteRequest {
                        content: content.clone(),
                        if_exists: if_exists.unwrap_or_default(),
                        if_missing: if_missing.unwrap_or_default(),
                        expected_snapshot: expected_snapshot.clone(),
                    },
                ),
                PatchOperation::Edit {
                    edits,
                    expected_snapshot,
                    ..
                } => apply_text_edits(
                    entry.content.as_deref(),
                    &display_path,
                    expected_snapshot.as_deref(),
                    edits,
                )?,
                PatchOperation::Delete {
                    expected_snapshot,
                    ignore_missing,
                    ..
                } => apply_delete(
                    entry.content.as_deref(),
                    &display_path,
                    expected_snapshot.as_deref(),
                    ignore_missing.unwrap_or(false),
                ),
            };

            if outcome.is_error {
                return Ok(ToolResult {
                    id: call_id,
                    call_id: external_call_id,
                    tool_name: "patch".to_string(),
                    parts: vec![MessagePart::text(outcome.summary)],
                    metadata: Some(json!({
                        "failed_path": display_path,
                        "applied_operations": operation_metadata,
                        "failed_operation": outcome.metadata,
                    })),
                    is_error: true,
                });
            }

            entry.content = outcome.next_content;
            entry.touched = true;
            operation_summaries.push(format!(
                "- {} [{} -> {}]",
                outcome.summary,
                outcome.snapshot_before.as_deref().unwrap_or("missing"),
                outcome.snapshot_after.as_deref().unwrap_or("deleted"),
            ));
            operation_metadata.push(json!({
                "path": display_path,
                "summary": outcome.summary,
                "snapshot_before": outcome.snapshot_before,
                "snapshot_after": outcome.snapshot_after,
                "operation": outcome.metadata,
            }));
            staged.insert(resolved, entry);
        }

        for entry in staged.values().filter(|entry| entry.touched) {
            commit_text_file(&entry.resolved_path, entry.content.as_deref()).await?;
        }

        let text = format!(
            "[patch operations={} changed_files={}]\n{}",
            input.operations.len(),
            staged.values().filter(|entry| entry.touched).count(),
            operation_summaries.join("\n")
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "patch".to_string(),
            parts: vec![MessagePart::text(text)],
            metadata: Some(json!({
                "operation_count": input.operations.len(),
                "changed_files": staged
                    .values()
                    .filter(|entry| entry.touched)
                    .map(|entry| entry.display_path.clone())
                    .collect::<Vec<_>>(),
                "operations": operation_metadata,
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PatchOperation, PatchTool, PatchToolInput};
    use crate::{TextEditOperation, Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;

    #[tokio::test]
    async fn patch_tool_applies_multiple_operations_without_partial_commits() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n")
            .await
            .unwrap();

        let tool = PatchTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(PatchToolInput {
                    operations: vec![
                        PatchOperation::Edit {
                            path: "sample.txt".to_string(),
                            edits: vec![TextEditOperation::StrReplace {
                                old_text: "beta".to_string(),
                                new_text: "gamma".to_string(),
                                replace_all: false,
                            }],
                            expected_snapshot: None,
                        },
                        PatchOperation::Write {
                            path: "created.txt".to_string(),
                            content: "new\n".to_string(),
                            if_exists: None,
                            if_missing: None,
                            expected_snapshot: None,
                        },
                    ],
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
            tokio::fs::read_to_string(dir.path().join("sample.txt"))
                .await
                .unwrap(),
            "alpha\ngamma\n"
        );
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("created.txt"))
                .await
                .unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn patch_tool_aborts_before_commit_on_failed_operation() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n")
            .await
            .unwrap();

        let tool = PatchTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(PatchToolInput {
                    operations: vec![
                        PatchOperation::Edit {
                            path: "sample.txt".to_string(),
                            edits: vec![TextEditOperation::StrReplace {
                                old_text: "beta".to_string(),
                                new_text: "gamma".to_string(),
                                replace_all: false,
                            }],
                            expected_snapshot: None,
                        },
                        PatchOperation::Delete {
                            path: "missing.txt".to_string(),
                            expected_snapshot: None,
                            ignore_missing: Some(false),
                        },
                    ],
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
            tokio::fs::read_to_string(dir.path().join("sample.txt"))
                .await
                .unwrap(),
            "alpha\nbeta\n"
        );
    }
}
