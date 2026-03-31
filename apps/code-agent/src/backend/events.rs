use crate::tool_render::tool_arguments_preview_lines;
use agent::runtime::{Result as RuntimeResult, RuntimeObserver, RuntimeProgressEvent};
use agent::types::{TokenLedgerSnapshot, TokenUsagePhase, ToolLifecycleEventKind};
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
    },
    ModelRequestStarted {
        iteration: usize,
    },
    TokenUsageUpdated {
        phase: TokenUsagePhase,
        ledger: TokenLedgerSnapshot,
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

#[derive(Clone, Default)]
pub(crate) struct SessionEventStream(Arc<Mutex<VecDeque<SessionEvent>>>);

impl SessionEventStream {
    pub(crate) fn drain(&self) -> Vec<SessionEvent> {
        self.0.lock().unwrap().drain(..).collect()
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
}

impl SessionEventObserver {
    pub(crate) fn new(stream: SessionEventStream) -> Self {
        Self { stream }
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
            } => SessionEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                summary,
            },
            RuntimeProgressEvent::ModelRequestStarted { iteration, .. } => {
                SessionEvent::ModelRequestStarted { iteration }
            }
            RuntimeProgressEvent::TokenUsageUpdated { phase, ledger } => {
                SessionEvent::TokenUsageUpdated { phase, ledger }
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
        self.stream.push(session_event);
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
    fn session_tool_call_formats_bash_commands_for_tui_previews() {
        let call = ToolCall {
            id: ToolCallId::from("tool-call-1"),
            call_id: ToolCallId::from("tool-call-1").into(),
            tool_name: "bash".into(),
            arguments: json!({"command": "cargo test -p code-agent"}),
            origin: ToolOrigin::Local,
        };

        let projected = session_tool_call(&call);

        assert_eq!(
            projected.arguments_preview,
            vec!["$ cargo test -p code-agent"]
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
