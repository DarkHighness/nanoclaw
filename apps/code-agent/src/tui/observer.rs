use super::state::{SharedUiState, preview_text};
use agent::runtime::{Result as RuntimeResult, RuntimeObserver, RuntimeProgressEvent};

pub(crate) struct SharedRenderObserver {
    ui_state: SharedUiState,
    active_assistant_line: Option<usize>,
}

impl SharedRenderObserver {
    pub(crate) fn new(ui_state: SharedUiState) -> Self {
        Self {
            ui_state,
            active_assistant_line: None,
        }
    }
}

impl RuntimeObserver for SharedRenderObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> RuntimeResult<()> {
        self.ui_state.mutate(|state| match event {
            RuntimeProgressEvent::SteerApplied { message, reason } => {
                state.push_transcript(format!("system> {message}"));
                state.status = "Applied steer".to_string();
                state.push_activity(format!(
                    "steer applied{}: {}",
                    reason
                        .as_deref()
                        .map(|value| format!(" ({value})"))
                        .unwrap_or_default(),
                    preview_text(&message, 48)
                ));
            }
            RuntimeProgressEvent::UserPromptAdded { prompt } => {
                self.active_assistant_line = None;
                state.push_transcript(format!("user> {prompt}"));
                state.status = "Planning next action".to_string();
                state.push_activity(format!("user prompt: {}", preview_text(&prompt, 40)));
            }
            RuntimeProgressEvent::AssistantTextDelta { delta } => {
                if let Some(index) = self.active_assistant_line {
                    state.transcript[index].push_str(&delta);
                } else {
                    state.push_transcript(format!("assistant> {delta}"));
                    self.active_assistant_line = Some(state.transcript.len() - 1);
                }
                state.transcript_scroll = u16::MAX;
                state.status = "Streaming answer".to_string();
            }
            RuntimeProgressEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                ..
            } => {
                state.status = format!(
                    "Compacted {source_message_count} messages, kept {retained_message_count}"
                );
                state.push_activity(format!(
                    "compaction complete: {}",
                    preview_text(&reason, 48)
                ));
            }
            RuntimeProgressEvent::ModelRequestStarted { iteration, .. } => {
                state.status = if iteration == 1 {
                    "Waiting for model response".to_string()
                } else {
                    format!("Continuing execution loop ({iteration})")
                };
            }
            RuntimeProgressEvent::ModelResponseCompleted { tool_calls, .. } => {
                self.active_assistant_line = None;
                state.status = if tool_calls.is_empty() {
                    "Model response complete".to_string()
                } else {
                    format!("Model requested {} tool(s)", tool_calls.len())
                };
            }
            RuntimeProgressEvent::ToolCallRequested { call } => {
                state.status = format!("Tool requested: {}", call.tool_name);
                state.push_activity(format!("requested {}", call.tool_name));
            }
            RuntimeProgressEvent::ToolApprovalRequested { call, .. } => {
                state.status = format!("Approval required: {}", call.tool_name);
                state.push_activity(format!("approval needed for {}", call.tool_name));
            }
            RuntimeProgressEvent::ToolApprovalResolved {
                call,
                approved,
                reason,
            } => {
                if approved {
                    state.status = format!("Approved {}", call.tool_name);
                    state.push_activity(format!("approved {}", call.tool_name));
                } else {
                    let reason = reason.unwrap_or_else(|| "permission denied".to_string());
                    state.status = format!("Denied {}: {}", call.tool_name, reason);
                    state.push_activity(format!(
                        "denied {}: {}",
                        call.tool_name,
                        preview_text(&reason, 44)
                    ));
                }
            }
            RuntimeProgressEvent::ToolCallStarted { call } => {
                state.status = format!("Running {}", call.tool_name);
                state.push_activity(format!("running {}", call.tool_name));
            }
            RuntimeProgressEvent::ToolCallCompleted { call, output } => {
                state.status = format!("Completed {}", call.tool_name);
                state.push_activity(format!(
                    "{} -> {}",
                    call.tool_name,
                    preview_text(&output.text_content(), 44)
                ));
            }
            RuntimeProgressEvent::ToolCallFailed { call, error } => {
                state.status = format!("{} failed", call.tool_name);
                state.push_activity(format!(
                    "{} failed: {}",
                    call.tool_name,
                    preview_text(&error, 44)
                ));
            }
            RuntimeProgressEvent::TurnCompleted { .. } => {
                self.active_assistant_line = None;
                state.status = "Turn complete".to_string();
                state.push_activity("turn complete");
            }
        });
        Ok(())
    }
}
