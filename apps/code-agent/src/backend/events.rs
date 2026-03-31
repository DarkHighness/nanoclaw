use crate::tool_render::tool_arguments_preview_lines;
use agent::runtime::{Result as RuntimeResult, RuntimeObserver, RuntimeProgressEvent};
use agent::types::{MessageId, TokenLedgerSnapshot, TokenUsagePhase, ToolLifecycleEventKind};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionToolCall {
    pub(crate) call_id: String,
    pub(crate) tool_name: String,
    pub(crate) origin: String,
    pub(crate) arguments_preview: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SessionEvent {
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
    pub(crate) fn tui_info_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "info",
            message: message.into(),
        }
    }

    pub(crate) fn tui_success_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "success",
            message: message.into(),
        }
    }

    pub(crate) fn tui_warning_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "warning",
            message: message.into(),
        }
    }

    pub(crate) fn tui_error_toast(message: impl Into<String>) -> Self {
        Self::TuiToastShow {
            variant: "error",
            message: message.into(),
        }
    }

    pub(crate) fn tui_prompt_append(text: impl Into<String>, only_when_empty: bool) -> Self {
        Self::TuiPromptAppend {
            text: text.into(),
            only_when_empty,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SessionEventStream(Arc<Mutex<VecDeque<SessionEvent>>>);

impl SessionEventStream {
    pub(crate) fn drain(&self) -> Vec<SessionEvent> {
        self.0.lock().unwrap().drain(..).collect()
    }

    pub(crate) fn publish(&self, event: SessionEvent) {
        self.push(event);
    }

    fn push(&self, event: SessionEvent) {
        let mut inner = self.0.lock().unwrap();
        inner.push_back(event);
        // Live frontends poll this queue opportunistically, so cap retained
        // events to avoid unbounded growth if the renderer stalls temporarily.
        while inner.len() > 512 {
            inner.pop_front();
        }
    }
}

pub(crate) struct SessionEventObserver {
    stream: SessionEventStream,
    captured: Vec<SessionEvent>,
}

impl SessionEventObserver {
    pub(crate) fn new(stream: SessionEventStream) -> Self {
        Self {
            stream,
            captured: Vec::new(),
        }
    }

    pub(crate) fn latest_compaction_summary(&self) -> Option<String> {
        self.captured.iter().rev().find_map(|event| match event {
            SessionEvent::CompactionCompleted { summary, .. } => Some(summary.clone()),
            _ => None,
        })
    }

    pub(crate) fn latest_compaction_summary_message_id(&self) -> Option<MessageId> {
        self.captured.iter().rev().find_map(|event| match event {
            SessionEvent::CompactionCompleted {
                summary_message_id, ..
            } => Some(summary_message_id.clone()),
            _ => None,
        })
    }

    pub(crate) fn completed_turn_count(&self) -> usize {
        self.captured
            .iter()
            .filter(|event| matches!(event, SessionEvent::TurnCompleted { .. }))
            .count()
    }

    pub(crate) fn requested_tool_call_count(&self) -> usize {
        self.captured
            .iter()
            .filter(|event| matches!(event, SessionEvent::ToolCallRequested { .. }))
            .count()
    }
}

impl RuntimeObserver for SessionEventObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> RuntimeResult<()> {
        let session_event = match event {
            RuntimeProgressEvent::SteerApplied { message, reason } => {
                SessionEvent::SteerApplied { message, reason }
            }
            RuntimeProgressEvent::UserPromptAdded { prompt } => {
                SessionEvent::UserPromptAdded { prompt }
            }
            RuntimeProgressEvent::AssistantTextDelta { delta } => {
                SessionEvent::AssistantTextDelta { delta }
            }
            RuntimeProgressEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                summary,
                compacted_through_message_id,
                summary_message_id,
            } => SessionEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                summary,
                compacted_through_message_id,
                summary_message_id,
            },
            RuntimeProgressEvent::ModelRequestStarted { iteration, .. } => {
                SessionEvent::ModelRequestStarted { iteration }
            }
            RuntimeProgressEvent::TokenUsageUpdated { phase, ledger } => {
                SessionEvent::TokenUsageUpdated { phase, ledger }
            }
            RuntimeProgressEvent::Notification { source, message } => {
                SessionEvent::Notification { source, message }
            }
            RuntimeProgressEvent::ModelResponseCompleted {
                assistant_text,
                tool_calls,
            } => SessionEvent::ModelResponseCompleted {
                assistant_text,
                tool_call_count: tool_calls.len(),
            },
            RuntimeProgressEvent::ToolCallRequested { call } => SessionEvent::ToolCallRequested {
                call: session_tool_call(&call),
            },
            RuntimeProgressEvent::ToolApprovalRequested { call, reasons } => {
                SessionEvent::ToolApprovalRequested {
                    call: session_tool_call(&call),
                    reasons,
                }
            }
            RuntimeProgressEvent::ToolApprovalResolved {
                call,
                approved,
                reason,
            } => SessionEvent::ToolApprovalResolved {
                call: session_tool_call(&call),
                approved,
                reason,
            },
            RuntimeProgressEvent::ToolLifecycle { event } => match event.event {
                ToolLifecycleEventKind::Started { call } => SessionEvent::ToolLifecycleStarted {
                    call: session_tool_call(&call),
                },
                ToolLifecycleEventKind::Completed { call, output } => {
                    SessionEvent::ToolLifecycleCompleted {
                        call: session_tool_call(&call),
                        output_preview: output.text_content(),
                        structured_output_preview: output
                            .structured_content
                            .as_ref()
                            .map(ToString::to_string),
                    }
                }
                ToolLifecycleEventKind::Failed { call, error } => {
                    SessionEvent::ToolLifecycleFailed {
                        call: session_tool_call(&call),
                        error,
                    }
                }
                ToolLifecycleEventKind::Cancelled { call, reason } => {
                    SessionEvent::ToolLifecycleCancelled {
                        call: session_tool_call(&call),
                        reason,
                    }
                }
            },
            RuntimeProgressEvent::TurnCompleted { assistant_text, .. } => {
                SessionEvent::TurnCompleted { assistant_text }
            }
        };
        self.stream.push(session_event.clone());
        self.captured.push(session_event);
        Ok(())
    }
}

fn session_tool_call(call: &agent::types::ToolCall) -> SessionToolCall {
    SessionToolCall {
        call_id: call.call_id.to_string(),
        tool_name: call.tool_name.to_string(),
        origin: match &call.origin {
            agent::ToolOrigin::Local => "local".to_string(),
            agent::ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
            agent::ToolOrigin::Provider { provider } => format!("provider:{provider}"),
        },
        arguments_preview: tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments),
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionEvent, SessionEventObserver, SessionEventStream, session_tool_call};
    use agent::runtime::{RuntimeObserver, RuntimeProgressEvent};
    use agent::types::{ToolCall, ToolCallId, ToolOrigin};
    use serde_json::json;

    #[test]
    fn observer_records_runtime_events_into_stream() {
        let stream = SessionEventStream::default();
        let mut observer = SessionEventObserver::new(stream.clone());

        observer
            .on_event(RuntimeProgressEvent::UserPromptAdded {
                prompt: "hello".to_string(),
            })
            .expect("event should record");

        assert_eq!(
            stream.drain(),
            vec![SessionEvent::UserPromptAdded {
                prompt: "hello".to_string(),
            }]
        );
    }

    #[test]
    fn observer_projects_runtime_notifications_into_session_events() {
        let stream = SessionEventStream::default();
        let mut observer = SessionEventObserver::new(stream.clone());

        observer
            .on_event(RuntimeProgressEvent::Notification {
                source: "loop_detector".to_string(),
                message: "loop detector warning".to_string(),
            })
            .expect("notification should record");

        assert_eq!(
            stream.drain(),
            vec![SessionEvent::Notification {
                source: "loop_detector".to_string(),
                message: "loop detector warning".to_string(),
            }]
        );
    }

    #[test]
    fn manual_tui_events_are_published_into_the_stream() {
        let stream = SessionEventStream::default();

        stream.publish(SessionEvent::tui_warning_toast("toast"));
        stream.publish(SessionEvent::tui_prompt_append("queued follow-up", true));

        assert_eq!(
            stream.drain(),
            vec![
                SessionEvent::tui_warning_toast("toast"),
                SessionEvent::tui_prompt_append("queued follow-up", true),
            ]
        );
    }

    #[test]
    fn session_tool_call_formats_exec_commands_for_tui_previews() {
        let call = ToolCall {
            id: ToolCallId::from("tool-call-2"),
            call_id: ToolCallId::from("tool-call-2").into(),
            tool_name: "exec_command".into(),
            arguments: json!({"cmd": "cargo test -p code-agent"}),
            origin: ToolOrigin::Local,
        };

        let projected = session_tool_call(&call);

        assert_eq!(
            projected.arguments_preview,
            vec!["$ cargo test -p code-agent"]
        );
    }
}
