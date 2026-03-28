use crate::{
    CompactionRequest, CompactionResult, ConversationCompactor, ModelBackend,
    ModelBackendCapabilities, Result, RuntimeObserver, RuntimeProgressEvent, ToolApprovalHandler,
    ToolApprovalOutcome, ToolApprovalRequest,
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tools::{Tool, ToolError, ToolExecutionContext, mcp_tool_annotations};
use types::{
    AgentCoreError, HookContext, HookOutput, Message, ModelEvent, ModelRequest,
    ProviderContinuation, ToolCall, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec,
};

pub(super) struct MockBackend;

#[derive(Clone, Default)]
pub(super) struct RecordingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingBackend {
    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[derive(Clone, Default)]
pub(super) struct ContinuingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    fail_first_continuation: Arc<Mutex<bool>>,
}

impl ContinuingBackend {
    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub(super) fn with_failed_continuation() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            fail_first_continuation: Arc::new(Mutex::new(true)),
        }
    }
}

pub(super) struct StaticPromptEvaluator;

pub(super) struct StaticCompactor;

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
pub(super) struct FailingTool;

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
pub(super) struct DangerousTool;

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
pub(super) struct MockApprovalHandler {
    requests: Mutex<Vec<ToolApprovalRequest>>,
    outcomes: Mutex<VecDeque<ToolApprovalOutcome>>,
}

#[derive(Default)]
pub(super) struct RecordingObserver {
    events: Vec<RuntimeProgressEvent>,
}

impl RecordingObserver {
    pub(super) fn events(&self) -> &[RuntimeProgressEvent] {
        &self.events
    }
}

impl RuntimeObserver for RecordingObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()> {
        self.events.push(event);
        Ok(())
    }
}

impl MockApprovalHandler {
    pub(super) fn with_outcomes(outcomes: impl IntoIterator<Item = ToolApprovalOutcome>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes.into_iter().collect()),
        }
    }

    pub(super) fn requests(&self) -> Vec<ToolApprovalRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolApprovalHandler for MockApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> Result<ToolApprovalOutcome> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ToolApprovalOutcome::Approve))
    }
}

#[async_trait]
impl ModelBackend for MockBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        let user_text = request
            .messages
            .last()
            .map(Message::text_content)
            .unwrap_or_default();
        if user_text.contains("tool")
            && !request.messages.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
            })
        {
            let call = ToolCall {
                id: ToolCallId::new(),
                call_id: "call-read-1".into(),
                tool_name: "read".into(),
                arguments: serde_json::json!({"path":"sample.txt","line_count":1}),
                origin: ToolOrigin::Local,
            };
            Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        } else {
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }
}

#[async_trait]
impl ModelBackend for RecordingBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: "ok".to_string(),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[async_trait]
impl ModelBackend for ContinuingBackend {
    fn capabilities(&self) -> ModelBackendCapabilities {
        ModelBackendCapabilities {
            provider_managed_history: true,
            provider_native_compaction: true,
        }
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request.clone());
        if request.continuation.is_some() {
            let mut fail_first = self.fail_first_continuation.lock().unwrap();
            if *fail_first {
                *fail_first = false;
                return Err(AgentCoreError::ProviderContinuationLost(
                    "provider lost previous_response_id".to_string(),
                )
                .into());
            }
        }

        let response_index = self.requests.lock().unwrap().len();
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: format!("response {response_index}"),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: Some(format!("msg_{response_index}").into()),
                continuation: Some(ProviderContinuation::OpenAiResponses {
                    response_id: format!("resp_{response_index}").into(),
                }),
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}
