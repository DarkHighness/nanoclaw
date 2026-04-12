use agent::types::{
    MessageId, MonitorEventRecord, MonitorSummaryRecord, TokenLedgerSnapshot, TokenUsagePhase,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionToolOrigin {
    Local,
    Mcp { server_name: String },
    Provider { provider: String },
}

impl std::fmt::Display for SessionToolOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Mcp { server_name } => write!(f, "mcp:{server_name}"),
            Self::Provider { provider } => write!(f, "provider:{provider}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionNotificationSource {
    LoopDetector,
    ProviderState,
    Other(String),
}

impl SessionNotificationSource {
    pub fn from_runtime(source: impl Into<String>) -> Self {
        match source.into().trim() {
            "loop_detector" => Self::LoopDetector,
            "provider_state" => Self::ProviderState,
            other => Self::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for SessionNotificationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoopDetector => f.write_str("loop_detector"),
            Self::ProviderState => f.write_str("provider_state"),
            Self::Other(source) => f.write_str(source),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionToastVariant {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub origin: SessionToolOrigin,
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
        source: SessionNotificationSource,
        message: String,
    },
    TuiToastShow {
        variant: SessionToastVariant,
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
    MonitorStarted {
        summary: MonitorSummaryRecord,
    },
    MonitorEvent {
        event: MonitorEventRecord,
    },
    MonitorUpdated {
        summary: MonitorSummaryRecord,
    },
    TurnCompleted {
        assistant_text: String,
    },
}

impl SessionEvent {
    pub fn tui_info_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: SessionToastVariant::Info,
            message: message.into(),
        }
    }

    pub fn tui_success_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: SessionToastVariant::Success,
            message: message.into(),
        }
    }

    pub fn tui_warning_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: SessionToastVariant::Warning,
            message: message.into(),
        }
    }

    pub fn tui_error_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: SessionToastVariant::Error,
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
