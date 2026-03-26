use crate::{
    EventId, HookEvent, HookOutput, Message, MessageId, Reasoning, ResponseId, RunId, SessionId,
    ToolCall, ToolCallId, ToolSpec, TurnId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderContinuation {
    OpenAiResponses { response_id: ResponseId },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub instructions: Vec<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub additional_context: Vec<String>,
    #[serde(default)]
    pub continuation: Option<ProviderContinuation>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    TextDelta {
        delta: String,
    },
    ToolCallRequested {
        call: ToolCall,
    },
    ResponseComplete {
        stop_reason: Option<String>,
        #[serde(default)]
        message_id: Option<MessageId>,
        #[serde(default)]
        continuation: Option<ProviderContinuation>,
        reasoning: Vec<Reasoning>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEventKind {
    SessionStart {
        reason: Option<String>,
    },
    InstructionsLoaded {
        count: usize,
    },
    SteerApplied {
        message: String,
        reason: Option<String>,
    },
    UserPromptSubmit {
        prompt: String,
    },
    ModelRequestStarted {
        request: ModelRequest,
    },
    CompactionCompleted {
        reason: String,
        source_message_count: usize,
        retained_message_count: usize,
        summary_chars: usize,
    },
    ModelResponseCompleted {
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        #[serde(default)]
        continuation: Option<ProviderContinuation>,
    },
    HookInvoked {
        hook_name: String,
        event: HookEvent,
    },
    HookCompleted {
        hook_name: String,
        event: HookEvent,
        output: HookOutput,
    },
    TranscriptMessage {
        message: Message,
    },
    ToolApprovalRequested {
        call: ToolCall,
        reasons: Vec<String>,
    },
    ToolApprovalResolved {
        call: ToolCall,
        approved: bool,
        reason: Option<String>,
    },
    ToolCallStarted {
        call: ToolCall,
    },
    ToolCallCompleted {
        call: ToolCall,
        output: crate::ToolResult,
    },
    ToolCallFailed {
        call: ToolCall,
        error: String,
    },
    Notification {
        source: String,
        message: String,
    },
    Stop {
        reason: Option<String>,
    },
    StopFailure {
        reason: Option<String>,
    },
    SessionEnd {
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunEventEnvelope {
    pub id: EventId,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: Option<ToolCallId>,
    pub timestamp_ms: u128,
    pub event: RunEventKind,
}

impl RunEventEnvelope {
    #[must_use]
    pub fn new(
        run_id: RunId,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        tool_call_id: Option<ToolCallId>,
        event: RunEventKind,
    ) -> Self {
        Self {
            id: EventId::new(),
            run_id,
            session_id,
            turn_id,
            tool_call_id,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| value.as_millis()),
            event,
        }
    }
}
