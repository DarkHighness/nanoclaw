use crate::{
    CompactionRequest, CompactionResult, ConversationCompactor, Result, RuntimeObserver,
    RuntimeProgressEvent,
};
use async_trait::async_trait;
use serde_json::Value;
use tools::{Tool, ToolError, ToolExecutionContext, mcp_tool_annotations};
use types::{
    HookContext, HookOutput, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec,
};

pub(in crate::runtime::tests) struct StaticPromptEvaluator;

pub(in crate::runtime::tests) struct StaticCompactor;

#[async_trait]
impl crate::PromptHookEvaluator for StaticPromptEvaluator {
    async fn evaluate(&self, _prompt: &str, _context: HookContext) -> Result<HookOutput> {
        Ok(HookOutput {
            system_message: Some("hook system message".to_string()),
            additional_context: vec!["hook additional context".to_string()],
            ..HookOutput::default()
        })
    }
}

#[async_trait]
impl ConversationCompactor for StaticCompactor {
    async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult> {
        Ok(CompactionResult {
            summary: format!("summary for {} messages", request.messages.len()),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::runtime::tests) struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fail".into(),
            description: "Always fails".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            output_mode: ToolOutputMode::Text,
            output_schema: None,
            origin: ToolOrigin::Local,
            annotations: Default::default(),
        }
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        _arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        Err(ToolError::invalid_state("boom"))
    }
}

#[derive(Clone, Debug, Default)]
pub(in crate::runtime::tests) struct DangerousTool;

#[async_trait]
impl Tool for DangerousTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "danger".into(),
            description: "Mutates files".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            output_mode: ToolOutputMode::Text,
            output_schema: None,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Dangerous Tool", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        _arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        Ok(ToolResult::text(call_id, "danger", "mutated"))
    }
}

#[derive(Default)]
pub(in crate::runtime::tests) struct RecordingObserver {
    events: Vec<RuntimeProgressEvent>,
}

impl RecordingObserver {
    pub(in crate::runtime::tests) fn events(&self) -> &[RuntimeProgressEvent] {
        &self.events
    }
}

impl RuntimeObserver for RecordingObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()> {
        self.events.push(event);
        Ok(())
    }
}
