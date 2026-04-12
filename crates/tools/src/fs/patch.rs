use crate::ToolExecutionContext;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::file_activity::FileActivityObserver;
use crate::fs::{
    TextEditOperation, WriteExistingBehavior, WriteMissingBehavior, WriteRequest, apply_delete,
    apply_text_edits, apply_write, commit_text_file, compute_diff_preview, load_optional_text_file,
    resolve_tool_path_against_workspace_root,
};
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use types::{MessagePart, ToolAvailability, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

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
    Move {
        from_path: String,
        to_path: String,
        #[serde(default)]
        expected_snapshot: Option<String>,
        #[serde(default)]
        if_destination_exists: Option<WriteExistingBehavior>,
        #[serde(default)]
        ignore_missing: Option<bool>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PatchToolInput {
    pub operations: Vec<PatchOperation>,
}

#[derive(Clone, Default)]
pub struct PatchTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

#[derive(Clone, Default)]
pub struct PatchFilesTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

#[derive(Clone, Debug)]
struct StagedFile {
    display_path: String,
    resolved_path: PathBuf,
    baseline_content: Option<String>,
    content: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PatchToolOutput {
    Success {
        operation_count: usize,
        changed_files: Vec<String>,
        operations: Vec<Value>,
        file_diffs: Vec<Value>,
    },
    Error {
        failed_path: String,
        failed_operation_index: usize,
        applied_operations: Vec<Value>,
        failed_operation: Value,
    },
}

impl PatchTool {
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

impl PatchFilesTool {
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

impl PatchOperation {
    fn command_name(&self) -> &'static str {
        match self {
            Self::Write { .. } => "write",
            Self::Edit { .. } => "edit",
            Self::Delete { .. } => "delete",
            Self::Move { .. } => "move",
        }
    }

    fn primary_path(&self) -> &str {
        match self {
            Self::Write { path, .. } | Self::Edit { path, .. } | Self::Delete { path, .. } => path,
            Self::Move { from_path, .. } => from_path,
        }
    }
}

#[async_trait]
impl Tool for PatchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "patch",
            "Legacy compatibility wrapper for staged multi-file patch application. Prefer patch_files as the canonical model-visible mutation surface.",
            serde_json::to_value(schema_for!(PatchToolInput)).expect("patch schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(PatchToolOutput)).expect("patch output schema"),
        )
        .with_availability(ToolAvailability {
            hidden_from_model: true,
            provider_allowlist: vec!["anthropic".to_string()],
            ..ToolAvailability::default()
        })
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: PatchToolInput = serde_json::from_value(arguments)?;
        execute_patch_operations(
            "patch",
            self.activity_observer.clone(),
            call_id,
            input.operations,
            ctx,
        )
        .await
    }
}

#[async_trait]
impl Tool for PatchFilesTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "patch_files",
            "Apply a staged multi-file patch made of write, edit, delete, and move operations. Operations are validated against staged content first so a failed operation does not partially apply earlier changes.",
            serde_json::to_value(schema_for!(PatchToolInput)).expect("patch_files schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(PatchToolOutput)).expect("patch_files output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: PatchToolInput = serde_json::from_value(arguments)?;
        execute_patch_operations(
            "patch_files",
            self.activity_observer.clone(),
            call_id,
            input.operations,
            ctx,
        )
        .await
    }
}

pub(crate) async fn execute_patch_operations(
    tool_name: &str,
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
    call_id: ToolCallId,
    operations: Vec<PatchOperation>,
    ctx: &ToolExecutionContext,
) -> Result<ToolResult> {
    if operations.is_empty() {
        return Err(ToolError::invalid(format!(
            "{tool_name} requires at least one operation"
        )));
    }

    let external_call_id = types::CallId::from(&call_id);
    let mut staged = BTreeMap::<PathBuf, StagedFile>::new();
    let mut operation_summaries = Vec::with_capacity(operations.len());
    let mut operation_metadata = Vec::with_capacity(operations.len());

    // Stage mutations in memory first so a failing later operation does not partially
    // commit an earlier file change. Later operations against the same path observe the
    // already-staged content, which matches normal patch application semantics.
    for (index, operation) in operations.iter().enumerate() {
        let operation_index = index + 1;
        match operation {
            PatchOperation::Write {
                path,
                content,
                if_exists,
                if_missing,
                expected_snapshot,
            } => {
                let mut entry = stage_entry(path, ctx, &mut staged).await?;
                let outcome = apply_write(
                    entry.content.as_deref(),
                    path,
                    &WriteRequest {
                        content: content.clone(),
                        if_exists: if_exists.unwrap_or_default(),
                        if_missing: if_missing.unwrap_or_default(),
                        expected_snapshot: expected_snapshot.clone(),
                    },
                );
                if outcome.is_error {
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        outcome.summary,
                        outcome.metadata,
                    ));
                }

                entry.content = outcome.next_content;
                operation_summaries.push(format!(
                    "- op#{operation_index} {} [{} -> {}]",
                    outcome.summary,
                    outcome.snapshot_before.as_deref().unwrap_or("missing"),
                    outcome.snapshot_after.as_deref().unwrap_or("deleted"),
                ));
                operation_metadata.push(json!({
                    "index": operation_index,
                    "command": operation.command_name(),
                    "path": path,
                    "summary": outcome.summary,
                    "snapshot_before": outcome.snapshot_before,
                    "snapshot_after": outcome.snapshot_after,
                    "operation": outcome.metadata,
                }));
                staged.insert(entry.resolved_path.clone(), entry);
            }
            PatchOperation::Edit {
                path,
                edits,
                expected_snapshot,
            } => {
                let mut entry = stage_entry(path, ctx, &mut staged).await?;
                let outcome = apply_text_edits(
                    entry.content.as_deref(),
                    path,
                    expected_snapshot.as_deref(),
                    edits,
                )?;
                if outcome.is_error {
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        outcome.summary,
                        outcome.metadata,
                    ));
                }

                entry.content = outcome.next_content;
                operation_summaries.push(format!(
                    "- op#{operation_index} {} [{} -> {}]",
                    outcome.summary,
                    outcome.snapshot_before.as_deref().unwrap_or("missing"),
                    outcome.snapshot_after.as_deref().unwrap_or("deleted"),
                ));
                operation_metadata.push(json!({
                    "index": operation_index,
                    "command": operation.command_name(),
                    "path": path,
                    "summary": outcome.summary,
                    "snapshot_before": outcome.snapshot_before,
                    "snapshot_after": outcome.snapshot_after,
                    "operation": outcome.metadata,
                }));
                staged.insert(entry.resolved_path.clone(), entry);
            }
            PatchOperation::Delete {
                path,
                expected_snapshot,
                ignore_missing,
            } => {
                let mut entry = stage_entry(path, ctx, &mut staged).await?;
                let outcome = apply_delete(
                    entry.content.as_deref(),
                    path,
                    expected_snapshot.as_deref(),
                    ignore_missing.unwrap_or(false),
                );
                if outcome.is_error {
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        outcome.summary,
                        outcome.metadata,
                    ));
                }

                entry.content = outcome.next_content;
                operation_summaries.push(format!(
                    "- op#{operation_index} {} [{} -> {}]",
                    outcome.summary,
                    outcome.snapshot_before.as_deref().unwrap_or("missing"),
                    outcome.snapshot_after.as_deref().unwrap_or("deleted"),
                ));
                operation_metadata.push(json!({
                    "index": operation_index,
                    "command": operation.command_name(),
                    "path": path,
                    "summary": outcome.summary,
                    "snapshot_before": outcome.snapshot_before,
                    "snapshot_after": outcome.snapshot_after,
                    "operation": outcome.metadata,
                }));
                staged.insert(entry.resolved_path.clone(), entry);
            }
            PatchOperation::Move {
                from_path,
                to_path,
                expected_snapshot,
                if_destination_exists,
                ignore_missing,
            } => {
                let from_resolved = resolve_tool_path_against_workspace_root(
                    from_path,
                    ctx.effective_root(),
                    ctx.container_workdir.as_deref(),
                )?;
                let to_resolved = resolve_tool_path_against_workspace_root(
                    to_path,
                    ctx.effective_root(),
                    ctx.container_workdir.as_deref(),
                )?;
                ctx.assert_path_write_allowed(&from_resolved)?;
                ctx.assert_path_write_allowed(&to_resolved)?;
                if from_resolved == to_resolved {
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        format!(
                            "Cannot move {from_path} to itself. Provide a different destination path."
                        ),
                        json!({
                            "command": "move",
                            "from_path": from_path,
                            "to_path": to_path,
                        }),
                    ));
                }

                let mut from_entry =
                    stage_entry_by_resolved(from_path, from_resolved.clone(), &mut staged).await?;
                let source_snapshot_before =
                    from_entry.content.as_deref().map(crate::stable_text_hash);

                if from_entry.content.is_none() {
                    if ignore_missing.unwrap_or(false) {
                        operation_summaries.push(format!(
                            "- op#{operation_index} Skipped move for missing source {from_path}"
                        ));
                        operation_metadata.push(json!({
                            "index": operation_index,
                            "command": operation.command_name(),
                            "from_path": from_path,
                            "to_path": to_path,
                            "summary": format!("Skipped move for missing source {from_path}"),
                            "moved": false,
                            "ignore_missing": true,
                        }));
                        staged.insert(from_resolved, from_entry);
                        continue;
                    }
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        format!(
                            "{from_path} does not exist. Re-run with ignore_missing=true to treat as a no-op."
                        ),
                        json!({
                            "command": "move",
                            "from_path": from_path,
                            "to_path": to_path,
                            "ignore_missing": false,
                        }),
                    ));
                }

                if let Some(expected_snapshot) = expected_snapshot.as_deref()
                    && source_snapshot_before.as_deref() != Some(expected_snapshot)
                {
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        format!(
                            "Snapshot mismatch for {from_path}. Expected {expected_snapshot}, re-read before moving."
                        ),
                        json!({
                            "command": "move",
                            "from_path": from_path,
                            "to_path": to_path,
                            "expected_snapshot": expected_snapshot,
                            "snapshot_before": source_snapshot_before,
                        }),
                    ));
                }

                let mut to_entry =
                    stage_entry_by_resolved(to_path, to_resolved.clone(), &mut staged).await?;
                if to_entry.content.is_some()
                    && matches!(
                        if_destination_exists.unwrap_or_default(),
                        WriteExistingBehavior::Error
                    )
                {
                    staged.insert(from_resolved, from_entry);
                    staged.insert(to_resolved, to_entry);
                    return Ok(patch_error_result(
                        tool_name,
                        call_id.clone(),
                        external_call_id,
                        operation,
                        operation_index,
                        operation_metadata,
                        format!(
                            "{to_path} already exists. Re-run with if_destination_exists=overwrite."
                        ),
                        json!({
                            "command": "move",
                            "from_path": from_path,
                            "to_path": to_path,
                            "if_destination_exists": if_destination_exists.unwrap_or_default(),
                        }),
                    ));
                }

                let destination_snapshot_before =
                    to_entry.content.as_deref().map(crate::stable_text_hash);
                // A move is modeled as delete+create in the staged map so later operations in
                // the same patch observe the renamed state before anything is committed.
                to_entry.content = from_entry.content.take();
                let destination_snapshot_after =
                    to_entry.content.as_deref().map(crate::stable_text_hash);
                let source_snapshot_after =
                    from_entry.content.as_deref().map(crate::stable_text_hash);

                operation_summaries.push(format!(
                    "- op#{operation_index} Moved {from_path} -> {to_path} [{} -> {}]",
                    source_snapshot_before.as_deref().unwrap_or("missing"),
                    destination_snapshot_after.as_deref().unwrap_or("missing"),
                ));
                operation_metadata.push(json!({
                    "index": operation_index,
                    "command": operation.command_name(),
                    "from_path": from_path,
                    "to_path": to_path,
                    "summary": format!("Moved {from_path} -> {to_path}"),
                    "source_snapshot_before": source_snapshot_before,
                    "source_snapshot_after": source_snapshot_after,
                    "destination_snapshot_before": destination_snapshot_before,
                    "destination_snapshot_after": destination_snapshot_after,
                    "if_destination_exists": if_destination_exists.unwrap_or_default(),
                    "ignore_missing": ignore_missing.unwrap_or(false),
                }));

                staged.insert(from_resolved, from_entry);
                staged.insert(to_resolved, to_entry);
            }
        }
    }

    let changed_entries: Vec<&StagedFile> = staged
        .values()
        .filter(|entry| entry.baseline_content != entry.content)
        .collect();

    for entry in &changed_entries {
        commit_text_file(&entry.resolved_path, entry.content.as_deref()).await?;
    }
    if let Some(observer) = &activity_observer {
        for entry in &changed_entries {
            if entry.content.is_some() {
                observer.did_change(entry.resolved_path.clone());
                observer.did_save(entry.resolved_path.clone());
            } else {
                observer.did_remove(entry.resolved_path.clone());
            }
        }
    }

    let diff_previews: Vec<Value> = changed_entries
        .iter()
        .filter_map(|entry| {
            compute_diff_preview(
                &entry.display_path,
                entry.baseline_content.as_deref(),
                entry.content.as_deref(),
            )
        })
        .collect();

    let mut text = format!(
        "[{tool_name} operations={} changed_files={}]\n{}",
        operations.len(),
        changed_entries.len(),
        operation_summaries.join("\n")
    );
    if !diff_previews.is_empty() {
        let previews = diff_previews
            .iter()
            .filter_map(|entry| entry["preview"].as_str())
            .collect::<Vec<_>>();
        if !previews.is_empty() {
            text.push_str("\n\n[diff_preview]");
            text.push('\n');
            text.push_str(&previews.join("\n\n"));
        }
    }
    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: tool_name.into(),
        parts: vec![MessagePart::text(text)],
        attachments: Vec::new(),
        structured_content: Some(
            serde_json::to_value(PatchToolOutput::Success {
                operation_count: operations.len(),
                changed_files: changed_entries
                    .iter()
                    .map(|entry| entry.display_path.clone())
                    .collect(),
                operations: operation_metadata.clone(),
                file_diffs: diff_previews.clone(),
            })
            .expect("patch success output"),
        ),
        continuation: None,
        metadata: Some(json!({
            "operation_count": operations.len(),
            "changed_files": changed_entries
                .iter()
                .map(|entry| entry.display_path.clone())
                .collect::<Vec<_>>(),
            "operations": operation_metadata,
            "file_diffs": diff_previews,
        })),
        is_error: false,
    })
}

async fn stage_entry(
    path: &str,
    ctx: &ToolExecutionContext,
    staged: &mut BTreeMap<PathBuf, StagedFile>,
) -> Result<StagedFile> {
    let resolved = resolve_tool_path_against_workspace_root(
        path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_write_allowed(&resolved)?;
    stage_entry_by_resolved(path, resolved, staged).await
}

async fn stage_entry_by_resolved(
    display_path: &str,
    resolved: PathBuf,
    staged: &mut BTreeMap<PathBuf, StagedFile>,
) -> Result<StagedFile> {
    Ok(match staged.remove(&resolved) {
        Some(entry) => entry,
        None => {
            let content = load_optional_text_file(&resolved).await?;
            StagedFile {
                display_path: display_path.to_string(),
                resolved_path: resolved,
                baseline_content: content.clone(),
                content,
            }
        }
    })
}

fn patch_error_result(
    tool_name: &str,
    call_id: ToolCallId,
    external_call_id: types::CallId,
    operation: &PatchOperation,
    operation_index: usize,
    operation_metadata: Vec<Value>,
    summary: String,
    diagnostic: Value,
) -> ToolResult {
    let failed_operation = json!({
        "index": operation_index,
        "command": operation.command_name(),
        "input": compact_operation(operation),
        "diagnostic": diagnostic,
    });
    ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: tool_name.into(),
        parts: vec![MessagePart::text(summary)],
        attachments: Vec::new(),
        structured_content: Some(
            serde_json::to_value(PatchToolOutput::Error {
                failed_path: operation.primary_path().to_string(),
                failed_operation_index: operation_index,
                applied_operations: operation_metadata.clone(),
                failed_operation: failed_operation.clone(),
            })
            .expect("patch error output"),
        ),
        continuation: None,
        metadata: Some(json!({
            "failed_path": operation.primary_path(),
            "failed_operation_index": operation_index,
            "applied_operations": operation_metadata,
            "failed_operation": failed_operation,
        })),
        is_error: true,
    }
}

fn compact_operation(operation: &PatchOperation) -> Value {
    match operation {
        PatchOperation::Write {
            path,
            content,
            if_exists,
            if_missing,
            expected_snapshot,
        } => json!({
            "command": "write",
            "path": path,
            "content_bytes": content.len(),
            "if_exists": if_exists.unwrap_or_default(),
            "if_missing": if_missing.unwrap_or_default(),
            "has_expected_snapshot": expected_snapshot.is_some(),
        }),
        PatchOperation::Edit {
            path,
            edits,
            expected_snapshot,
        } => json!({
            "command": "edit",
            "path": path,
            "edit_count": edits.len(),
            "has_expected_snapshot": expected_snapshot.is_some(),
        }),
        PatchOperation::Delete {
            path,
            expected_snapshot,
            ignore_missing,
        } => json!({
            "command": "delete",
            "path": path,
            "ignore_missing": ignore_missing.unwrap_or(false),
            "has_expected_snapshot": expected_snapshot.is_some(),
        }),
        PatchOperation::Move {
            from_path,
            to_path,
            expected_snapshot,
            if_destination_exists,
            ignore_missing,
        } => json!({
            "command": "move",
            "from_path": from_path,
            "to_path": to_path,
            "if_destination_exists": if_destination_exists.unwrap_or_default(),
            "ignore_missing": ignore_missing.unwrap_or(false),
            "has_expected_snapshot": expected_snapshot.is_some(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{PatchOperation, PatchTool, PatchToolInput};
    use crate::{TextEditOperation, Tool, ToolExecutionContext};
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
                        workspace_only: true,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert!(!result.is_error);
            let structured = result.structured_content.unwrap();
            assert_eq!(structured["kind"], "success");
            assert_eq!(structured["operation_count"], 2);
            assert_eq!(structured["changed_files"].as_array().unwrap().len(), 2);
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
    );

    bounded_async_test!(
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
                        workspace_only: true,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert!(result.is_error);
            let structured = result.structured_content.clone().unwrap();
            assert_eq!(structured["kind"], "error");
            assert_eq!(structured["failed_operation_index"], 2);
            let metadata = result.metadata.unwrap();
            assert_eq!(metadata["failed_operation_index"].as_u64().unwrap(), 2);
            assert_eq!(
                metadata["failed_operation"]["command"].as_str().unwrap(),
                "delete"
            );
            assert_eq!(
                tokio::fs::read_to_string(dir.path().join("sample.txt"))
                    .await
                    .unwrap(),
                "alpha\nbeta\n"
            );
        }
    );

    bounded_async_test!(
        async fn patch_tool_can_move_files_atomically() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("old.txt"), "payload\n")
                .await
                .unwrap();

            let result = PatchTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(PatchToolInput {
                        operations: vec![PatchOperation::Move {
                            from_path: "old.txt".to_string(),
                            to_path: "new.txt".to_string(),
                            expected_snapshot: None,
                            if_destination_exists: None,
                            ignore_missing: None,
                        }],
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
            let structured = result.structured_content.clone().unwrap();
            assert_eq!(structured["operations"][0]["command"], "move");
            assert!(
                !tokio::fs::try_exists(dir.path().join("old.txt"))
                    .await
                    .unwrap()
            );
            assert_eq!(
                tokio::fs::read_to_string(dir.path().join("new.txt"))
                    .await
                    .unwrap(),
                "payload\n"
            );
            let metadata = result.metadata.unwrap();
            assert_eq!(
                metadata["operations"][0]["command"].as_str().unwrap(),
                "move"
            );
        }
    );

    bounded_async_test!(
        async fn patch_tool_returns_hunk_preview_for_changes() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n")
                .await
                .unwrap();

            let result = PatchTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(PatchToolInput {
                        operations: vec![PatchOperation::Edit {
                            path: "sample.txt".to_string(),
                            edits: vec![TextEditOperation::StrReplace {
                                old_text: "beta".to_string(),
                                new_text: "gamma".to_string(),
                                replace_all: false,
                            }],
                            expected_snapshot: None,
                        }],
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

            let output = result.text_content();
            assert!(output.contains("[diff_preview]"));
            assert!(output.contains("@@ -2,1 +2,1 @@"));
            assert!(output.contains("-beta"));
            assert!(output.contains("+gamma"));
        }
    );

    bounded_async_test!(
        async fn patch_tool_rejects_protected_workspace_state_paths() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::create_dir_all(dir.path().join(".nanoclaw"))
                .await
                .unwrap();

            let err = PatchTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(PatchToolInput {
                        operations: vec![PatchOperation::Write {
                            path: ".nanoclaw/state.toml".to_string(),
                            content: "x = 1\n".to_string(),
                            if_exists: None,
                            if_missing: None,
                            expected_snapshot: None,
                        }],
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
        }
    );
}
