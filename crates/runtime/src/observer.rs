use crate::Result;
use types::{
    MessageId, TokenLedgerSnapshot, TokenUsagePhase, ToolCall, ToolLifecycleEventEnvelope, TurnId,
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
    TurnCompleted {
        turn_id: TurnId,
        assistant_text: String,
    },
}

pub trait RuntimeObserver: Send {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()>;
}

#[derive(Default)]
pub struct NoopRuntimeObserver;

impl RuntimeObserver for NoopRuntimeObserver {
    fn on_event(&mut self, _event: RuntimeProgressEvent) -> Result<()> {
        Ok(())
    }
}
