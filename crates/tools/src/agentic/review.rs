use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, SessionReviewResult, SessionReviewScope, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use types::{CallId, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

const REVIEW_START_TOOL_NAME: &str = "review_start";

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReviewStartToolInput {
    #[serde(default)]
    pub scope: SessionReviewScope,
}

#[derive(Clone, Debug, Default)]
pub struct ReviewStartTool;

impl ReviewStartTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReviewStartTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            REVIEW_START_TOOL_NAME,
            "Start a structured review of recently completed tool activity in the current session. Use latest_turn to inspect the current turn or since_checkpoint to review work after the most recent checkpoint boundary.",
            serde_json::to_value(schema_for!(ReviewStartToolInput))
                .expect("review_start schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(SessionReviewResult))
                .expect("review_start output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = CallId::from(&call_id);
        let input: ReviewStartToolInput = serde_json::from_value(arguments)?;
        let handler = ctx.session_control_handler.as_ref().ok_or_else(|| {
            ToolError::invalid_state(
                "review_start is unavailable without a host session-control handler",
            )
        })?;
        let result = handler.start_review(ctx, input.scope).await?;
        Ok(ToolResult::text(
            call_id,
            REVIEW_START_TOOL_NAME,
            render_review_start_text(&result),
        )
        .with_structured_content(json!(result))
        .with_call_id(external_call_id))
    }
}

fn render_review_start_text(result: &SessionReviewResult) -> String {
    if result.items.is_empty() {
        return result.summary.clone();
    }

    let mut lines = vec![result.summary.clone()];
    if let Some(boundary) = result.boundary.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!("boundary {boundary}"));
    }
    lines.push(format!("scope {}", result.scope.as_str()));
    lines.push(format!("items {}", result.items.len()));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{ReviewStartTool, ReviewStartToolInput};
    use crate::{
        Result, SessionCompactionResult, SessionControlHandler, SessionReviewItem,
        SessionReviewItemKind, SessionReviewResult, SessionReviewScope, Tool, ToolExecutionContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use types::ToolCallId;

    #[derive(Default)]
    struct MockSessionControlHandler;

    #[async_trait]
    impl SessionControlHandler for MockSessionControlHandler {
        async fn compact_now(
            &self,
            _ctx: &ToolExecutionContext,
            _notes: Option<String>,
        ) -> Result<SessionCompactionResult> {
            Ok(SessionCompactionResult { compacted: false })
        }

        async fn start_review(
            &self,
            _ctx: &ToolExecutionContext,
            scope: SessionReviewScope,
        ) -> Result<SessionReviewResult> {
            Ok(SessionReviewResult {
                scope,
                summary: "reviewed 1 tool call".to_string(),
                tool_call_count: 1,
                boundary: Some("latest prompt".to_string()),
                items: vec![SessionReviewItem {
                    title: "exec_command · Command".to_string(),
                    kind: SessionReviewItemKind::Command,
                    preview_lines: vec!["$ cargo test".to_string()],
                }],
            })
        }
    }

    #[tokio::test]
    async fn review_start_returns_structured_result() {
        let tool = ReviewStartTool::new();
        let ctx = ToolExecutionContext {
            session_control_handler: Some(Arc::new(MockSessionControlHandler)),
            ..ToolExecutionContext::default()
        };
        let result = tool
            .execute(
                ToolCallId::from("call_review"),
                json!(ReviewStartToolInput {
                    scope: SessionReviewScope::SinceCheckpoint,
                }),
                &ctx,
            )
            .await
            .expect("tool should succeed");

        let structured = result
            .structured_content
            .expect("review_start should return structured output");
        assert_eq!(structured["scope"], "since_checkpoint");
        assert_eq!(structured["tool_call_count"], 1);
        assert_eq!(structured["items"][0]["kind"], "command");
    }
}
