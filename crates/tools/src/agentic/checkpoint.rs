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
const CHECKPOINT_SUMMARIZE_TOOL_NAME: &str = "checkpoint_summarize";

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CheckpointListToolInput {}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct CheckpointSummarizeToolInput {
    #[serde(default)]
    pub notes: Option<String>,
}

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
struct CheckpointSummarizeToolOutput {
    compacted: bool,
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
pub struct CheckpointSummarizeTool;

impl CheckpointSummarizeTool {
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
impl Tool for CheckpointSummarizeTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            CHECKPOINT_SUMMARIZE_TOOL_NAME,
            "Compact earlier conversation context in the current session without changing workspace files. Use this to summarize verbose history while preserving the current code state.",
            serde_json::to_value(schema_for!(CheckpointSummarizeToolInput))
                .expect("checkpoint_summarize schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CheckpointSummarizeToolOutput))
                .expect("checkpoint_summarize output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = CallId::from(&call_id);
        let input: CheckpointSummarizeToolInput = serde_json::from_value(arguments)?;
        let handler = ctx.session_control_handler.as_ref().ok_or_else(|| {
            ToolError::invalid_state(
                "checkpoint_summarize is unavailable without a host session-control handler",
            )
        })?;
        let result = handler.compact_now(ctx, input.notes).await?;
        Ok(ToolResult::text(
            call_id,
            CHECKPOINT_SUMMARIZE_TOOL_NAME,
            render_checkpoint_summarize_text(result.compacted),
        )
        .with_structured_content(json!(CheckpointSummarizeToolOutput {
            compacted: result.compacted,
        }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for CheckpointRestoreTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            CHECKPOINT_RESTORE_TOOL_NAME,
            "Restore workspace code, conversation state, or both to the boundary captured before a recorded checkpoint mutation.",
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

fn render_checkpoint_summarize_text(compacted: bool) -> String {
    if compacted {
        "Compacted earlier conversation context without changing workspace files.".to_string()
    } else {
        "Skipped compaction because the current session did not have enough visible history to summarize.".to_string()
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

#[cfg(test)]
mod tests {
    use super::{CheckpointRestoreTool, CheckpointSummarizeTool};
    use crate::{
        CheckpointHandler, CheckpointMutationRequest, Result, SessionCompactionResult,
        SessionControlHandler, SessionReviewResult, SessionReviewScope, Tool, ToolExecutionContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use types::{
        AgentSessionId, CheckpointFileRecord, CheckpointId, CheckpointOrigin, CheckpointRecord,
        CheckpointRestoreMode, CheckpointRestoreRecord, CheckpointScope, SessionId, ToolCallId,
        ToolName,
    };

    #[derive(Clone)]
    struct StaticCheckpointHandler {
        checkpoints: Vec<CheckpointRecord>,
        restore: CheckpointRestoreRecord,
    }

    #[async_trait]
    impl CheckpointHandler for StaticCheckpointHandler {
        async fn record_mutation(
            &self,
            _ctx: &ToolExecutionContext,
            _request: CheckpointMutationRequest,
        ) -> Result<CheckpointRecord> {
            unreachable!("record_mutation is not used in these tests");
        }

        async fn list_checkpoints(
            &self,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CheckpointRecord>> {
            Ok(self.checkpoints.clone())
        }

        async fn restore_checkpoint(
            &self,
            _ctx: &ToolExecutionContext,
            _checkpoint_id: &CheckpointId,
            _mode: CheckpointRestoreMode,
        ) -> Result<CheckpointRestoreRecord> {
            Ok(self.restore.clone())
        }
    }

    struct StaticSessionControlHandler {
        compacted: bool,
    }

    #[async_trait]
    impl SessionControlHandler for StaticSessionControlHandler {
        async fn compact_now(
            &self,
            _ctx: &ToolExecutionContext,
            _notes: Option<String>,
        ) -> Result<SessionCompactionResult> {
            Ok(SessionCompactionResult {
                compacted: self.compacted,
            })
        }

        async fn start_review(
            &self,
            _ctx: &ToolExecutionContext,
            scope: SessionReviewScope,
        ) -> Result<SessionReviewResult> {
            Ok(SessionReviewResult {
                scope,
                summary: "review unavailable in checkpoint tests".to_string(),
                tool_call_count: 0,
                boundary: None,
                items: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn checkpoint_summarize_returns_structured_result() {
        let tool = CheckpointSummarizeTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({"notes": "focus on failing tests"}),
                &ToolExecutionContext {
                    session_control_handler: Some(Arc::new(StaticSessionControlHandler {
                        compacted: true,
                    })),
                    ..ToolExecutionContext::default()
                },
            )
            .await
            .expect("checkpoint_summarize should succeed");

        assert!(result.parts.iter().any(|part| matches!(
            part,
            types::MessagePart::Text { text } if text.contains("Compacted earlier")
        )));
        let structured = result
            .structured_content
            .expect("checkpoint_summarize structured output");
        assert_eq!(structured["compacted"], json!(true));
    }

    #[tokio::test]
    async fn checkpoint_restore_accepts_conversation_restore_modes() {
        let tool = CheckpointRestoreTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "checkpoint_id": "checkpoint_123",
                    "restore_mode": "both"
                }),
                &ToolExecutionContext {
                    checkpoint_handler: Some(Arc::new(StaticCheckpointHandler {
                        checkpoints: vec![CheckpointRecord {
                            checkpoint_id: CheckpointId::from("checkpoint_123"),
                            session_id: SessionId::from("session_1"),
                            agent_session_id: AgentSessionId::from("agent_session_1"),
                            scope: CheckpointScope::Both,
                            origin: CheckpointOrigin::FileTool {
                                tool_name: ToolName::from("write"),
                            },
                            summary: "saved".to_string(),
                            created_at_unix_s: 1,
                            rollback_message_id: None,
                            prompt_message_id: None,
                            changed_files: vec![CheckpointFileRecord {
                                requested_path: "src/lib.rs".to_string(),
                                resolved_path: "src/lib.rs".into(),
                                before_text: Some("before".to_string()),
                                after_text: Some("after".to_string()),
                            }],
                        }],
                        restore: CheckpointRestoreRecord {
                            restored_from: CheckpointId::from("checkpoint_123"),
                            restore_mode: CheckpointRestoreMode::Both,
                            restored_file_count: 1,
                            restored_files: vec!["src/lib.rs".to_string()],
                            restore_checkpoint_id: None,
                            rollback_message_id: None,
                            prompt_message_id: None,
                        },
                    })),
                    ..ToolExecutionContext::default()
                },
            )
            .await
            .expect("checkpoint_restore should allow both mode");

        let structured = result
            .structured_content
            .expect("checkpoint_restore structured output");
        assert_eq!(structured["result"]["restore_mode"], json!("both"));
    }
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
