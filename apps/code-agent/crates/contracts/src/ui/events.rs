use agent::types::{MessageId, TokenLedgerSnapshot, TokenUsagePhase};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub origin: String,
    pub arguments_preview: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    SteerApplied {
        message: String,
        reason: Option<String>,
    },
    UserPromptAdded {
        prompt: String,
    },
    AssistantTextDelta {
        delta: String,
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
    TuiToastShow {
        variant: &'static str,
        message: String,
    },
    TuiPromptAppend {
        text: String,
        only_when_empty: bool,
    },
    ModelResponseCompleted {
        assistant_text: String,
        tool_call_count: usize,
    },
    ToolCallRequested {
        call: SessionToolCall,
    },
    ToolApprovalRequested {
        call: SessionToolCall,
        reasons: Vec<String>,
    },
    ToolApprovalResolved {
        call: SessionToolCall,
        approved: bool,
        reason: Option<String>,
    },
    ToolLifecycleStarted {
        call: SessionToolCall,
    },
    ToolLifecycleCompleted {
        call: SessionToolCall,
        output_preview: String,
        structured_output_preview: Option<String>,
    },
    ToolLifecycleFailed {
        call: SessionToolCall,
        error: String,
    },
    ToolLifecycleCancelled {
        call: SessionToolCall,
        reason: Option<String>,
    },
    TurnCompleted {
        assistant_text: String,
    },
}

impl SessionEvent {
    pub fn tui_info_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "info",
            message: message.into(),
        }
    }

    pub fn tui_success_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "success",
            message: message.into(),
        }
    }

    pub fn tui_warning_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "warning",
            message: message.into(),
        }
    }

    pub fn tui_error_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "error",
            message: message.into(),
        }
    }

    pub fn tui_prompt_append(text: impl Into<String>, only_when_empty: bool) -> Self {
        Self::TuiPromptAppend {
            text: text.into(),
            only_when_empty,
        }
    }
}
