use crate::Result;
use std::path::PathBuf;
use types::{
    AgentHandle, AgentId, AgentResultEnvelope, AgentTaskSpec, MessageId, TaskId, TaskStatus,
    TokenLedgerSnapshot, TokenUsagePhase, ToolCall, ToolLifecycleEventEnvelope, TurnId, WorktreeId,
};

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeProgressEvent {
    SteerApplied {
        message: String,
        reason: Option<String>,
    },
    UserPromptAdded {
        prompt: String,
    },
    CompactionCompleted {
        reason: String,
        source_message_count: usize,
        retained_message_count: usize,
        summary: String,
        compacted_through_message_id: MessageId,
        summary_message_id: MessageId,
    },
    ModelRequestStarted {
        turn_id: TurnId,
        iteration: usize,
    },
    TokenUsageUpdated {
        phase: TokenUsagePhase,
        ledger: TokenLedgerSnapshot,
    },
    Notification {
        source: String,
        message: String,
    },
    // Hook-emitted UI cues stay on the live observer plane instead of entering
    // durable transcript history, so hosts can surface them without
    // reinterpreting provider-facing messages.
    TuiToastShow {
        variant: String,
        message: String,
    },
    TuiPromptAppend {
        text: String,
        only_when_empty: bool,
    },
    AssistantTextDelta {
        delta: String,
    },
    ToolCallRequested {
        call: ToolCall,
    },
    ModelResponseCompleted {
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
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
    // Host-facing tool lifecycle events reuse the same event ids and normalized
    // call ids that are persisted in the session store, so live UIs can
    // correlate streaming updates with durable history without reparsing
    // transcript text.
    ToolLifecycle {
        event: ToolLifecycleEventEnvelope,
    },
    TaskCreated {
        task: AgentTaskSpec,
        parent_agent_id: Option<AgentId>,
        status: TaskStatus,
        summary: Option<String>,
        worktree_id: Option<WorktreeId>,
        worktree_root: Option<PathBuf>,
    },
    TaskUpdated {
        task_id: TaskId,
        status: TaskStatus,
        summary: Option<String>,
    },
    TaskCompleted {
        task_id: TaskId,
        agent_id: AgentId,
        status: TaskStatus,
    },
    SubagentStarted {
        handle: AgentHandle,
        task: AgentTaskSpec,
    },
    SubagentStopped {
        handle: AgentHandle,
        result: Option<AgentResultEnvelope>,
        error: Option<String>,
    },
    TurnCompleted {
        turn_id: TurnId,
        assistant_text: String,
    },
}

pub trait RuntimeObserver: Send {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()>;
}

/// Long-lived host integrations such as a TUI session event stream need a
/// cloneable publish surface instead of a per-turn mutable observer borrow.
pub trait RuntimeProgressSink: Send + Sync {
    fn emit(&self, event: RuntimeProgressEvent) -> Result<()>;
}

#[derive(Default)]
pub struct NoopRuntimeObserver;

impl RuntimeObserver for NoopRuntimeObserver {
    fn on_event(&mut self, _event: RuntimeProgressEvent) -> Result<()> {
        Ok(())
    }
}
