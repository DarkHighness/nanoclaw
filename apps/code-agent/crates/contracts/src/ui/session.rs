use super::mcp::StartupDiagnosticsSnapshot;
use crate::display::TuiDisplayConfig;
use crate::interaction::SessionPermissionMode;
use crate::motion::TuiMotionConfig;
use crate::statusline::StatusLineConfig;
use agent::types::{
    AgentHandle, AgentSessionId, AgentStatus, AgentTaskSpec, CheckpointId, CheckpointRestoreRecord,
    Message, MessageId, SessionEventEnvelope, SessionId, SessionSummaryTokenUsage, ToolSpec,
};
use std::path::PathBuf;
use store::{SessionSummary, SessionTokenUsageReport, TokenUsageRecord};

#[derive(Clone, Debug, Default)]
pub struct SessionStartupSnapshot {
    pub workspace_name: String,
    pub workspace_root: PathBuf,
    pub active_session_ref: String,
    pub root_agent_session_id: String,
    pub provider_label: String,
    pub model: String,
    pub model_reasoning_effort: Option<String>,
    pub supported_model_reasoning_efforts: Vec<String>,
    pub supports_image_input: bool,
    pub tool_names: Vec<String>,
    pub tool_specs: Vec<ToolSpec>,
    pub disabled_tool_names: Vec<String>,
    pub store_label: String,
    pub store_warning: Option<String>,
    pub stored_session_count: usize,
    pub default_sandbox_summary: String,
    pub sandbox_summary: String,
    pub permission_mode: SessionPermissionMode,
    pub host_process_surfaces_allowed: bool,
    pub startup_diagnostics: StartupDiagnosticsSnapshot,
    pub display: TuiDisplayConfig,
    pub statusline: StatusLineConfig,
    pub motion: TuiMotionConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionOperation {
    StartFresh,
    ResumeAgentSession { agent_session_ref: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionOperationAction {
    StartedFresh,
    AlreadyAttached,
    Reattached,
}

#[derive(Clone, Debug)]
pub struct SessionOperationOutcome {
    pub action: SessionOperationAction,
    pub session_ref: String,
    pub active_agent_session_ref: String,
    pub requested_agent_session_ref: Option<String>,
    pub startup: SessionStartupSnapshot,
    pub transcript: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRollbackOutcome {
    pub transcript: Vec<Message>,
    pub removed_message_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRollbackCheckpoint {
    pub checkpoint_id: CheckpointId,
    pub summary: String,
    pub changed_file_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRollbackRound {
    pub rollback_message_id: MessageId,
    pub prompt_message: Message,
    pub round_messages: Vec<Message>,
    pub removed_turn_count: usize,
    pub removed_message_count: usize,
    pub checkpoint: Option<HistoryRollbackCheckpoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckpointRestoreOutcome {
    pub restore: CheckpointRestoreRecord,
    pub transcript: Vec<Message>,
    pub removed_message_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideQuestionOutcome {
    pub question: String,
    pub response: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeSupport {
    AttachedToActiveRuntime,
    Reattachable,
    NotYetSupported { reason: String },
}

impl ResumeSupport {
    pub fn label(&self) -> &'static str {
        match self {
            Self::AttachedToActiveRuntime => "attached",
            Self::Reattachable => "reattachable",
            Self::NotYetSupported { .. } => "history-only",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedSessionSummary {
    pub session_ref: String,
    pub first_timestamp_ms: u128,
    pub last_timestamp_ms: u128,
    pub event_count: usize,
    pub worker_session_count: usize,
    pub transcript_message_count: usize,
    pub session_title: Option<String>,
    pub last_user_prompt: Option<String>,
    pub token_usage: Option<SessionSummaryTokenUsage>,
    pub resume_support: ResumeSupport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedSessionSearchMatch {
    pub summary: PersistedSessionSummary,
    pub matched_event_count: usize,
    pub preview_matches: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedAgentSessionSummary {
    pub agent_session_ref: String,
    pub session_ref: String,
    pub label: String,
    pub first_timestamp_ms: u128,
    pub last_timestamp_ms: u128,
    pub event_count: usize,
    pub transcript_message_count: usize,
    pub session_title: Option<String>,
    pub last_user_prompt: Option<String>,
    pub resume_support: ResumeSupport,
}

#[derive(Clone, Debug)]
pub struct LoadedSession {
    pub summary: SessionSummary,
    pub agent_session_ids: Vec<AgentSessionId>,
    pub transcript: Vec<Message>,
    pub events: Vec<SessionEventEnvelope>,
    pub token_usage: SessionTokenUsageReport,
}

#[derive(Clone, Debug)]
pub struct LoadedAgentSession {
    pub summary: PersistedAgentSessionSummary,
    pub transcript: Vec<Message>,
    pub events: Vec<SessionEventEnvelope>,
    pub token_usage: Option<TokenUsageRecord>,
    pub subagents: Vec<LoadedSubagentSession>,
}

#[derive(Clone, Debug)]
pub struct LoadedSubagentSession {
    pub handle: AgentHandle,
    pub task: AgentTaskSpec,
    pub status: AgentStatus,
    pub summary: String,
    pub token_usage: Option<TokenUsageRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionExportKind {
    EventsJsonl,
    TranscriptText,
}

#[derive(Clone, Debug)]
pub struct SessionExportArtifact {
    pub kind: SessionExportKind,
    pub session_id: SessionId,
    pub output_path: PathBuf,
    pub item_count: usize,
}
