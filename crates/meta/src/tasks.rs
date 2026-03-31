use serde::{Deserialize, Serialize};
use types::{
    AgentSessionId, EventId, SelfImproveSignalKind, SessionId, SignalId, ToolName, TurnId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfImproveTaskKind {
    PromptRegressionFix,
    ToolSelectionFix,
    SubagentRoutingFix,
    HookPolicyFix,
    CostLatencyOptimization,
    RuntimeBugfix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfImproveTaskPriority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfImproveTask {
    pub task_id: String,
    pub kind: SelfImproveTaskKind,
    pub priority: SelfImproveTaskPriority,
    pub summary: String,
    pub objective: String,
    pub expected_outcome: String,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default)]
    pub source_signal_ids: Vec<SignalId>,
    #[serde(default)]
    pub source_event_ids: Vec<EventId>,
    #[serde(default)]
    pub source_signal_kinds: Vec<SelfImproveSignalKind>,
    #[serde(default)]
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<ToolName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_task_id: Option<String>,
    #[serde(default)]
    pub details: Vec<String>,
}
