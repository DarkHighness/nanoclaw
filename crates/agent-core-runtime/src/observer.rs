use crate::Result;
use agent_core_types::{ToolCall, ToolResult, TurnId};

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
    },
    ModelRequestStarted {
        turn_id: TurnId,
        iteration: usize,
    },
    AssistantTextDelta {
        delta: String,
        accumulated_text: String,
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
    ToolCallStarted {
        call: ToolCall,
    },
    ToolCallCompleted {
        call: ToolCall,
        output: ToolResult,
    },
    ToolCallFailed {
        call: ToolCall,
        error: String,
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
