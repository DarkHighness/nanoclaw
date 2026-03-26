use crate::{
    CallId, EventId, HookEvent, HookOutput, Message, MessageId, Reasoning, ResponseId, RunId,
    SessionId, ToolCall, ToolCallId, ToolSpec, TurnId,
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
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ToolLifecycleEventKind {
    Started {
        call: ToolCall,
    },
    Completed {
        call: ToolCall,
        output: crate::ToolResult,
    },
    Failed {
        call: ToolCall,
        error: String,
    },
    Cancelled {
        call: ToolCall,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolLifecycleEventEnvelope {
    pub id: EventId,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: ToolCallId,
    pub call_id: CallId,
    pub tool_name: String,
    pub timestamp_ms: u128,
    pub event: ToolLifecycleEventKind,
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

    #[must_use]
    pub fn tool_lifecycle_event(&self) -> Option<ToolLifecycleEventEnvelope> {
        // Live host observers and persisted run history need to agree on the
        // same event identity. This projection reuses the stored RunEventEnvelope
        // ids instead of manufacturing a second tool-event namespace.
        let lifecycle = match &self.event {
            RunEventKind::ToolCallStarted { call } => {
                ToolLifecycleEventKind::Started { call: call.clone() }
            }
            RunEventKind::ToolCallCompleted { call, output } => ToolLifecycleEventKind::Completed {
                call: call.clone(),
                output: output.clone(),
            },
            RunEventKind::ToolCallFailed { call, error } => ToolLifecycleEventKind::Failed {
                call: call.clone(),
                error: error.clone(),
            },
            _ => return None,
        };

        let (tool_call_id, call_id, tool_name) = match &lifecycle {
            ToolLifecycleEventKind::Started { call }
            | ToolLifecycleEventKind::Completed { call, .. }
            | ToolLifecycleEventKind::Failed { call, .. }
            | ToolLifecycleEventKind::Cancelled { call, .. } => (
                self.tool_call_id.clone().unwrap_or_else(|| call.id.clone()),
                call.call_id.clone(),
                call.tool_name.clone(),
            ),
        };

        Some(ToolLifecycleEventEnvelope {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
            tool_call_id,
            call_id,
            tool_name,
            timestamp_ms: self.timestamp_ms,
            event: lifecycle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RunEventEnvelope, RunEventKind, ToolLifecycleEventKind};
    use crate::{ToolCall, ToolCallId, ToolOrigin, ToolResult};

    #[test]
    fn run_event_maps_tool_completion_to_lifecycle_envelope() {
        let call = ToolCall {
            id: ToolCallId::new(),
            call_id: "call-read-1".into(),
            tool_name: "read".to_string(),
            arguments: serde_json::json!({"path":"sample.txt"}),
            origin: ToolOrigin::Local,
        };
        let output =
            ToolResult::text(call.id.clone(), "read", "ok").with_call_id(call.call_id.clone());
        let envelope = RunEventEnvelope::new(
            "run_1".into(),
            "session_1".into(),
            Some("turn_1".into()),
            Some(call.id.clone()),
            RunEventKind::ToolCallCompleted {
                call: call.clone(),
                output: output.clone(),
            },
        );

        let lifecycle = envelope
            .tool_lifecycle_event()
            .expect("tool lifecycle event");
        assert_eq!(lifecycle.id, envelope.id);
        assert_eq!(lifecycle.tool_call_id, call.id);
        assert_eq!(lifecycle.call_id, call.call_id);
        assert_eq!(lifecycle.tool_name, "read");
        assert!(matches!(
            lifecycle.event,
            ToolLifecycleEventKind::Completed { call: mapped_call, output: mapped_output }
                if mapped_call.tool_name == "read" && mapped_output.text_content() == "ok"
        ));
    }
}
