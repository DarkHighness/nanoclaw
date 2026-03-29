use crate::{
    AgentId, AgentSessionId, CallId, EnvelopeId, EventId, HookEvent, HookResult, Message,
    MessageId, Reasoning, ResponseId, RunId, TokenLedgerSnapshot, TokenUsage, TokenUsagePhase,
    ToolCall, ToolCallId, ToolName, ToolSpec, TurnId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderContinuation {
    OpenAiResponses { response_id: ResponseId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Queued,
    Running,
    WaitingApproval,
    WaitingMessage,
    Completed,
    Failed,
    Cancelled,
}

impl AgentStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingMessage => "waiting_message",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentHandle {
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    pub run_id: RunId,
    pub agent_session_id: AgentSessionId,
    pub task_id: String,
    pub role: String,
    pub status: AgentStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentArtifact {
    pub kind: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTaskSpec {
    pub task_id: String,
    pub role: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<ToolName>,
    #[serde(default)]
    pub requested_write_set: Vec<String>,
    #[serde(default)]
    pub dependency_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentResultEnvelope {
    pub agent_id: AgentId,
    pub task_id: String,
    pub status: AgentStatus,
    pub summary: String,
    pub text: String,
    #[serde(default)]
    pub artifacts: Vec<AgentArtifact>,
    #[serde(default)]
    pub claimed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_payload: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEnvelopeKind {
    SpawnRequested { task: AgentTaskSpec },
    Started { task: AgentTaskSpec },
    StatusChanged { status: AgentStatus },
    Message { channel: String, payload: Value },
    Artifact { artifact: AgentArtifact },
    ClaimRequested { files: Vec<String> },
    ClaimGranted { files: Vec<String> },
    ClaimRejected { files: Vec<String>, owner: AgentId },
    Result { result: AgentResultEnvelope },
    Failed { error: String },
    Cancelled { reason: Option<String> },
    Heartbeat,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentEnvelope {
    pub envelope_id: EnvelopeId,
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    pub run_id: RunId,
    pub agent_session_id: AgentSessionId,
    pub timestamp_ms: u64,
    pub kind: AgentEnvelopeKind,
}

impl AgentEnvelope {
    #[must_use]
    pub fn new(
        agent_id: AgentId,
        parent_agent_id: Option<AgentId>,
        run_id: RunId,
        agent_session_id: AgentSessionId,
        kind: AgentEnvelopeKind,
    ) -> Self {
        Self {
            envelope_id: EnvelopeId::new(),
            agent_id,
            parent_agent_id,
            run_id,
            agent_session_id,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| {
                    value
                        .as_millis()
                        .min(u128::from(u64::MAX))
                        .try_into()
                        .unwrap_or(u64::MAX)
                }),
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentWaitMode {
    Any,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWaitRequest {
    pub agent_ids: Vec<AgentId>,
    #[serde(default = "default_agent_wait_mode")]
    pub mode: AgentWaitMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWaitResponse {
    pub completed: Vec<AgentHandle>,
    pub pending: Vec<AgentHandle>,
    #[serde(default)]
    pub results: Vec<AgentResultEnvelope>,
}

fn default_agent_wait_mode() -> AgentWaitMode {
    AgentWaitMode::All
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub run_id: RunId,
    pub agent_session_id: AgentSessionId,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
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
    pub agent_session_id: AgentSessionId,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: ToolCallId,
    pub call_id: CallId,
    pub tool_name: ToolName,
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
    TokenUsageUpdated {
        phase: TokenUsagePhase,
        ledger: TokenLedgerSnapshot,
    },
    HookInvoked {
        hook_name: String,
        event: HookEvent,
    },
    HookCompleted {
        hook_name: String,
        event: HookEvent,
        output: HookResult,
    },
    TranscriptMessage {
        message: Message,
    },
    TranscriptMessagePatched {
        message_id: MessageId,
        message: Message,
    },
    TranscriptMessageRemoved {
        message_id: MessageId,
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
    TaskCreated {
        task: AgentTaskSpec,
        parent_agent_id: Option<AgentId>,
    },
    TaskCompleted {
        task_id: String,
        agent_id: AgentId,
        status: AgentStatus,
    },
    SubagentStart {
        handle: AgentHandle,
        task: AgentTaskSpec,
    },
    AgentEnvelope {
        envelope: AgentEnvelope,
    },
    SubagentStop {
        handle: AgentHandle,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<AgentResultEnvelope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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
    pub agent_session_id: AgentSessionId,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: Option<ToolCallId>,
    pub timestamp_ms: u128,
    pub event: RunEventKind,
}

impl RunEventEnvelope {
    #[must_use]
    pub fn new(
        run_id: RunId,
        agent_session_id: AgentSessionId,
        turn_id: Option<TurnId>,
        tool_call_id: Option<ToolCallId>,
        event: RunEventKind,
    ) -> Self {
        Self {
            id: EventId::new(),
            run_id,
            agent_session_id,
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
            agent_session_id: self.agent_session_id.clone(),
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
    use super::{
        AgentEnvelope, AgentEnvelopeKind, AgentResultEnvelope, AgentStatus, AgentTaskSpec,
        RunEventEnvelope, RunEventKind, ToolLifecycleEventKind,
    };
    use crate::{AgentId, ToolCall, ToolCallId, ToolName, ToolOrigin, ToolResult};

    #[test]
    fn run_event_maps_tool_completion_to_lifecycle_envelope() {
        let call = ToolCall {
            id: ToolCallId::new(),
            call_id: "call-read-1".into(),
            tool_name: "read".into(),
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
        assert_eq!(lifecycle.tool_name, ToolName::from("read"));
        assert!(matches!(
            lifecycle.event,
            ToolLifecycleEventKind::Completed { call: mapped_call, output: mapped_output }
                if mapped_call.tool_name == ToolName::from("read") && mapped_output.text_content() == "ok"
        ));
    }

    #[test]
    fn agent_envelope_retains_parent_relation_and_result_payload() {
        let task = AgentTaskSpec {
            task_id: "task_1".to_string(),
            role: "reviewer".to_string(),
            prompt: "inspect".to_string(),
            steer: None,
            allowed_tools: vec!["read".into()],
            requested_write_set: vec!["src/lib.rs".to_string()],
            dependency_ids: Vec::new(),
            timeout_seconds: Some(30),
        };
        let envelope = AgentEnvelope::new(
            AgentId::from("agent_child"),
            Some(AgentId::from("agent_parent")),
            "run_child".into(),
            "session_child".into(),
            AgentEnvelopeKind::Result {
                result: AgentResultEnvelope {
                    agent_id: AgentId::from("agent_child"),
                    task_id: "task_1".to_string(),
                    status: AgentStatus::Completed,
                    summary: "done".to_string(),
                    text: "ok".to_string(),
                    artifacts: Vec::new(),
                    claimed_files: vec!["src/lib.rs".to_string()],
                    structured_payload: Some(serde_json::json!({"task_id": task.task_id})),
                },
            },
        );

        assert_eq!(
            envelope.parent_agent_id,
            Some(AgentId::from("agent_parent"))
        );
        match envelope.kind {
            AgentEnvelopeKind::Result { result } => {
                assert_eq!(result.status, AgentStatus::Completed);
                assert_eq!(result.structured_payload.unwrap()["task_id"], "task_1");
            }
            other => panic!("unexpected envelope kind: {other:?}"),
        }
    }

    #[test]
    fn task_completed_event_keeps_agent_status() {
        let event = RunEventEnvelope::new(
            "run_1".into(),
            "session_1".into(),
            None,
            None,
            RunEventKind::TaskCompleted {
                task_id: "task_1".to_string(),
                agent_id: "agent_1".into(),
                status: AgentStatus::Cancelled,
            },
        );

        assert!(matches!(
            event.event,
            RunEventKind::TaskCompleted {
                status: AgentStatus::Cancelled,
                ..
            }
        ));
    }
}
