use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use types::{
    CallId, CheckpointRecord, CheckpointRestoreMode, CheckpointRestoreRecord, ToolCallId,
    ToolOutputMode, ToolResult, ToolSpec,
};

const CHECKPOINT_LIST_TOOL_NAME: &str = "checkpoint_list";
const CHECKPOINT_RESTORE_TOOL_NAME: &str = "checkpoint_restore";

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CheckpointListToolInput {}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CheckpointRestoreToolInput {
    pub checkpoint_id: types::CheckpointId,
    #[serde(default)]
    pub restore_mode: Option<CheckpointRestoreMode>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CheckpointListToolOutput {
    result_count: usize,
    checkpoints: Vec<CheckpointRecord>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CheckpointRestoreToolOutput {
    result: CheckpointRestoreRecord,
}

#[derive(Clone, Debug, Default)]
pub struct CheckpointListTool;

impl CheckpointListTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone, Debug, Default)]
pub struct CheckpointRestoreTool;

impl CheckpointRestoreTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for CheckpointListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            CHECKPOINT_LIST_TOOL_NAME,
            "List durable workspace checkpoints captured before file mutations in the current session. Use this before checkpoint_restore to inspect restore points and affected files.",
            serde_json::to_value(schema_for!(CheckpointListToolInput))
                .expect("checkpoint_list schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CheckpointListToolOutput))
                .expect("checkpoint_list output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = CallId::from(&call_id);
        let _input: CheckpointListToolInput = serde_json::from_value(arguments)?;
        let handler = ctx.checkpoint_handler.as_ref().ok_or_else(|| {
            ToolError::invalid_state(
                "checkpoint_list is unavailable without a host checkpoint handler",
            )
        })?;
        let checkpoints = handler.list_checkpoints(ctx).await?;
        Ok(ToolResult::text(
            call_id,
            CHECKPOINT_LIST_TOOL_NAME,
            render_checkpoint_list_text(&checkpoints),
        )
        .with_structured_content(json!(CheckpointListToolOutput {
            result_count: checkpoints.len(),
            checkpoints,
        }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for CheckpointRestoreTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            CHECKPOINT_RESTORE_TOOL_NAME,
            "Restore workspace code to the state captured before a recorded checkpoint mutation. The model-visible tool supports code_only restores; transcript rewind remains a host/operator surface.",
            serde_json::to_value(schema_for!(CheckpointRestoreToolInput))
                .expect("checkpoint_restore schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CheckpointRestoreToolOutput))
                .expect("checkpoint_restore output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = CallId::from(&call_id);
        let input: CheckpointRestoreToolInput = serde_json::from_value(arguments)?;
        let restore_mode = input
            .restore_mode
            .unwrap_or(CheckpointRestoreMode::CodeOnly);
        if restore_mode != CheckpointRestoreMode::CodeOnly {
            return Err(ToolError::invalid_state(
                "checkpoint_restore only supports restore_mode=code_only inside model tool execution; transcript rewind remains a host/operator surface",
            ));
        }
        let handler = ctx.checkpoint_handler.as_ref().ok_or_else(|| {
            ToolError::invalid_state(
                "checkpoint_restore is unavailable without a host checkpoint handler",
            )
        })?;
        let result = handler
            .restore_checkpoint(ctx, &input.checkpoint_id, restore_mode)
            .await?;
        Ok(ToolResult::text(
            call_id,
            CHECKPOINT_RESTORE_TOOL_NAME,
            render_checkpoint_restore_text(&result),
        )
        .with_structured_content(json!(CheckpointRestoreToolOutput { result }))
        .with_call_id(external_call_id))
    }
}

fn render_checkpoint_list_text(checkpoints: &[CheckpointRecord]) -> String {
    if checkpoints.is_empty() {
        return "No checkpoints recorded in the current session.".to_string();
    }

    let mut lines = vec![format!(
        "checkpoint_list result_count={}",
        checkpoints.len()
    )];
    for checkpoint in checkpoints {
        let origin = match &checkpoint.origin {
            types::CheckpointOrigin::FileTool { tool_name } => tool_name.to_string(),
            types::CheckpointOrigin::Restore {
                restored_from,
                restore_mode,
            } => format!("restore {restored_from} ({restore_mode})"),
        };
        lines.push(format!(
            "- {} [{}] files={} {}",
            checkpoint.checkpoint_id,
            origin,
            checkpoint.changed_files.len(),
            checkpoint.summary
        ));
    }
    lines.join("\n")
}

fn render_checkpoint_restore_text(result: &CheckpointRestoreRecord) -> String {
    if result.restored_file_count == 0 {
        return format!(
            "Checkpoint {} already matches the current workspace for restore_mode={}.",
            result.restored_from, result.restore_mode
        );
    }

    let mut text = format!(
        "Restored {} file(s) to checkpoint {} using restore_mode={}.",
        result.restored_file_count, result.restored_from, result.restore_mode
    );
    if !result.restored_files.is_empty() {
        text.push_str("\n[files]\n");
        text.push_str(&result.restored_files.join("\n"));
    }
    if let Some(restore_checkpoint_id) = result.restore_checkpoint_id.as_ref() {
        text.push_str(&format!("\n[checkpoint_saved {restore_checkpoint_id}]"));
    }
    text
}
