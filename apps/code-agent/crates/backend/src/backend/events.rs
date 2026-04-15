use crate::tool_render::tool_arguments_preview_lines;
use crate::ui::{
    SessionEvent, SessionNotificationSource, SessionToastVariant, SessionToolCall,
    SessionToolOrigin,
};
use agent::runtime::{
    Result as RuntimeResult, RuntimeObserver, RuntimeProgressEvent, RuntimeProgressSink,
};
use agent::types::{MessageId, ToolLifecycleEventKind};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct SessionEventStream(Arc<Mutex<VecDeque<SessionEvent>>>);

impl SessionEventStream {
    pub fn drain(&self) -> Vec<SessionEvent> {
        self.0.lock().unwrap().drain(..).collect()
    }

    pub fn publish(&self, event: SessionEvent) {
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

pub struct SessionEventObserver {
    stream: SessionEventStream,
    captured: Vec<SessionEvent>,
}

#[derive(Clone)]
pub struct SessionEventPublisher {
    stream: SessionEventStream,
}

impl SessionEventPublisher {
    #[must_use]
    pub fn new(stream: SessionEventStream) -> Self {
        Self { stream }
    }
}

impl SessionEventObserver {
    pub fn new(stream: SessionEventStream) -> Self {
        Self {
            stream,
            captured: Vec::new(),
        }
    }

    pub fn latest_compaction_summary(&self) -> Option<String> {
        self.captured.iter().rev().find_map(|event| match event {
            SessionEvent::CompactionCompleted { summary, .. } => Some(summary.clone()),
            _ => None,
        })
    }

    pub fn latest_compaction_summary_message_id(&self) -> Option<MessageId> {
        self.captured.iter().rev().find_map(|event| match event {
            SessionEvent::CompactionCompleted {
                summary_message_id, ..
            } => Some(summary_message_id.clone()),
            _ => None,
        })
    }
}

impl RuntimeObserver for SessionEventObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> RuntimeResult<()> {
        let session_event = project_session_event(event);
        self.stream.push(session_event.clone());
        self.captured.push(session_event);
        Ok(())
    }
}

impl RuntimeProgressSink for SessionEventPublisher {
    fn emit(&self, event: RuntimeProgressEvent) -> RuntimeResult<()> {
        self.stream.push(project_session_event(event));
        Ok(())
    }
}

fn project_session_event(event: RuntimeProgressEvent) -> SessionEvent {
    match event {
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
        RuntimeProgressEvent::ProviderRetryScheduled {
            iteration,
            status_code,
            retry_count,
            max_retries,
            remaining_retries,
            next_retry_at_ms,
            ..
        } => SessionEvent::ProviderRetryScheduled {
            iteration,
            status_code,
            retry_count,
            max_retries,
            remaining_retries,
            next_retry_at_ms,
        },
        RuntimeProgressEvent::TokenUsageUpdated { phase, ledger } => {
            SessionEvent::TokenUsageUpdated { phase, ledger }
        }
        RuntimeProgressEvent::Notification { source, message } => SessionEvent::Notification {
            source: SessionNotificationSource::from_runtime(source),
            message,
        },
        RuntimeProgressEvent::TuiToastShow { variant, message } => SessionEvent::TuiToastShow {
            variant: session_toast_variant(&variant),
            message,
        },
        RuntimeProgressEvent::TuiPromptAppend {
            text,
            only_when_empty,
        } => SessionEvent::TuiPromptAppend {
            text,
            only_when_empty,
        },
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
            ToolLifecycleEventKind::Failed { call, error } => SessionEvent::ToolLifecycleFailed {
                call: session_tool_call(&call),
                error,
            },
            ToolLifecycleEventKind::Cancelled { call, reason } => {
                SessionEvent::ToolLifecycleCancelled {
                    call: session_tool_call(&call),
                    reason,
                }
            }
        },
        RuntimeProgressEvent::TaskCreated {
            task,
            parent_agent_id,
            status,
            summary,
            worktree_id,
            worktree_root,
        } => SessionEvent::TaskCreated {
            task,
            parent_agent_id,
            status,
            summary,
            worktree_id,
            worktree_root,
        },
        RuntimeProgressEvent::TaskUpdated {
            task_id,
            status,
            summary,
        } => SessionEvent::TaskUpdated {
            task_id,
            status,
            summary,
        },
        RuntimeProgressEvent::TaskCompleted {
            task_id,
            agent_id,
            status,
        } => SessionEvent::TaskCompleted {
            task_id,
            agent_id,
            status,
        },
        RuntimeProgressEvent::SubagentStarted { handle, task } => {
            SessionEvent::SubagentStarted { handle, task }
        }
        RuntimeProgressEvent::SubagentStopped {
            handle,
            result,
            error,
        } => SessionEvent::SubagentStopped {
            handle,
            result,
            error,
        },
        RuntimeProgressEvent::TurnCompleted { assistant_text, .. } => {
            SessionEvent::TurnCompleted { assistant_text }
        }
    }
}

fn session_tool_call(call: &agent::types::ToolCall) -> SessionToolCall {
    SessionToolCall {
        call_id: call.call_id.to_string(),
        tool_name: call.tool_name.to_string(),
        origin: match &call.origin {
            agent::ToolOrigin::Local => SessionToolOrigin::Local,
            agent::ToolOrigin::Mcp { server_name } => SessionToolOrigin::Mcp {
                server_name: server_name.to_string(),
            },
            agent::ToolOrigin::Provider { provider } => SessionToolOrigin::Provider {
                provider: provider.clone(),
            },
        },
        arguments_preview: tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments),
    }
}

fn session_toast_variant(variant: &str) -> SessionToastVariant {
    match variant {
        "success" => SessionToastVariant::Success,
        "warning" => SessionToastVariant::Warning,
        "error" => SessionToastVariant::Error,
        _ => SessionToastVariant::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionEvent, SessionEventObserver, SessionEventStream, session_tool_call};
    use crate::ui::{SessionNotificationSource, SessionToolOrigin};
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
                source: SessionNotificationSource::LoopDetector,
                message: "loop detector warning".to_string(),
            }]
        );
    }

    #[test]
    fn observer_projects_runtime_tui_events_into_session_events() {
        let stream = SessionEventStream::default();
        let mut observer = SessionEventObserver::new(stream.clone());

        observer
            .on_event(RuntimeProgressEvent::TuiToastShow {
                variant: "warning".to_string(),
                message: "review the completed task".to_string(),
            })
            .expect("toast should record");
        observer
            .on_event(RuntimeProgressEvent::TuiPromptAppend {
                text: "follow up on task result".to_string(),
                only_when_empty: true,
            })
            .expect("prompt append should record");

        assert_eq!(
            stream.drain(),
            vec![
                SessionEvent::tui_warning_toast("review the completed task"),
                SessionEvent::tui_prompt_append("follow up on task result", true),
            ]
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
        assert_eq!(projected.origin, SessionToolOrigin::Local);
    }
}
