use agent::types::{
    AgentArtifact, AgentResultEnvelope, AgentTaskSpec, Message, TaskId, TaskOrigin, TaskStatus,
    WorktreeId,
};
use std::path::PathBuf;
use store::TokenUsageRecord;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTaskSummary {
    pub agent_id: String,
    pub task_id: TaskId,
    pub role: String,
    pub origin: TaskOrigin,
    pub status: TaskStatus,
    pub session_ref: String,
    pub agent_session_ref: String,
    pub worktree_id: Option<WorktreeId>,
    pub worktree_root: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTaskSpawnOutcome {
    pub task: LiveTaskSummary,
    pub prompt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveTaskControlAction {
    Cancelled,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTaskControlOutcome {
    pub requested_ref: String,
    pub agent_id: String,
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub action: LiveTaskControlAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveTaskMessageAction {
    Sent,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTaskMessageOutcome {
    pub requested_ref: String,
    pub agent_id: String,
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub action: LiveTaskMessageAction,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTaskWaitOutcome {
    pub requested_ref: String,
    pub agent_id: String,
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub summary: String,
    pub claimed_files: Vec<String>,
    pub remaining_live_tasks: Vec<LiveTaskSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveTaskAttentionAction {
    QueuedPrompt,
    ScheduledSteer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTaskAttentionOutcome {
    pub action: LiveTaskAttentionAction,
    pub control_id: String,
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedTaskSummary {
    pub task_id: TaskId,
    pub session_ref: String,
    pub parent_agent_session_ref: String,
    pub child_session_ref: Option<String>,
    pub child_agent_session_ref: Option<String>,
    pub role: String,
    pub origin: TaskOrigin,
    pub status: TaskStatus,
    pub first_timestamp_ms: u128,
    pub last_timestamp_ms: u128,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedTaskMessage {
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedTask {
    pub summary: PersistedTaskSummary,
    pub spec: AgentTaskSpec,
    pub child_transcript: Vec<Message>,
    pub result: Option<AgentResultEnvelope>,
    pub error: Option<String>,
    pub artifacts: Vec<AgentArtifact>,
    pub messages: Vec<LoadedTaskMessage>,
    pub token_usage: Option<TokenUsageRecord>,
}
