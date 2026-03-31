use crate::{AgentSessionId, EventId, SessionId, SignalId, ToolCallId, ToolName, TurnId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalSource {
    Turn,
    Tool,
    Approval,
    Hook,
    History,
    Subagent,
    Usage,
    Runtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelfImproveSignalKind {
    TurnFailed,
    TurnStopFailure,
    ToolCallFailure,
    ToolApprovalDenied,
    RetryChurn,
    HighTokenUsage,
    HighTurnLatency,
    HookStop,
    HistoryRollback,
    SubagentFailure,
    SubagentCancelled,
    LoopDetectorWarning,
    LoopDetectorCritical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SelfImproveSignalRecord {
    pub signal_id: SignalId,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    pub timestamp_ms: u128,
    pub source: SignalSource,
    pub kind: SelfImproveSignalKind,
    pub severity: SignalSeverity,
    pub summary: String,
    #[serde(default)]
    pub event_ids: Vec<EventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<ToolName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl SelfImproveSignalRecord {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        agent_session_id: AgentSessionId,
        turn_id: Option<TurnId>,
        tool_call_id: Option<ToolCallId>,
        timestamp_ms: u128,
        source: SignalSource,
        kind: SelfImproveSignalKind,
        severity: SignalSeverity,
        summary: impl Into<String>,
        event_ids: Vec<EventId>,
    ) -> Self {
        Self {
            signal_id: SignalId::new(),
            session_id,
            agent_session_id,
            turn_id,
            tool_call_id,
            timestamp_ms,
            source,
            kind,
            severity,
            summary: summary.into(),
            event_ids,
            tool_name: None,
            task_id: None,
            details: None,
            metadata: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SelfImproveSignalKind, SelfImproveSignalRecord, SignalSeverity, SignalSource};

    #[test]
    fn signal_record_constructor_keeps_context() {
        let record = SelfImproveSignalRecord::new(
            "session-signal".into(),
            "agent-signal".into(),
            Some("turn-signal".into()),
            Some("tool-call-signal".into()),
            99,
            SignalSource::Tool,
            SelfImproveSignalKind::ToolCallFailure,
            SignalSeverity::Error,
            "bash failed",
            vec!["event-signal".into()],
        );

        assert_eq!(record.summary, "bash failed");
        assert_eq!(record.event_ids.len(), 1);
        assert_eq!(record.timestamp_ms, 99);
        assert!(record.tool_call_id.is_some());
    }
}
