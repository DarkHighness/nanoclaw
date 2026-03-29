use super::state::{SharedUiState, preview_text};
use crate::backend::SessionEvent;
use std::collections::HashMap;

pub(crate) struct SharedRenderObserver {
    ui_state: SharedUiState,
    active_assistant_line: Option<usize>,
    active_tool_lines: HashMap<String, usize>,
}

impl SharedRenderObserver {
    pub(crate) fn new(ui_state: SharedUiState) -> Self {
        Self {
            ui_state,
            active_assistant_line: None,
            active_tool_lines: HashMap::new(),
        }
    }

    pub(crate) fn apply_event(&mut self, event: SessionEvent) {
        self.ui_state.mutate(|state| match event {
            SessionEvent::SteerApplied { message, reason } => {
                state.push_transcript(format!(
                    "• Applied steer\n  └ {}{}",
                    message,
                    reason
                        .as_deref()
                        .map(|value| format!(" ({value})"))
                        .unwrap_or_default()
                ));
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
            SessionEvent::UserPromptAdded { prompt } => {
                self.active_assistant_line = None;
                self.active_tool_lines.clear();
                state.active_tool_label = None;
                state.push_transcript(format!("› {prompt}"));
                state.status = "Working".to_string();
                state.push_activity(format!("user prompt: {}", preview_text(&prompt, 40)));
            }
            SessionEvent::AssistantTextDelta { delta } => {
                if let Some(index) = self.active_assistant_line {
                    state.transcript[index].push_str(&delta);
                } else {
                    state.push_transcript(format!("• {delta}"));
                    self.active_assistant_line = Some(state.transcript.len() - 1);
                }
                state.transcript_scroll = u16::MAX;
                state.status = "Working".to_string();
            }
            SessionEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                ..
            } => {
                state.push_transcript(format!(
                    "• Compacted history\n  └ kept {retained_message_count} of {source_message_count} messages"
                ));
                state.status = format!(
                    "Compacted {source_message_count} messages, kept {retained_message_count}"
                );
                state.push_activity(format!(
                    "compaction complete: {}",
                    preview_text(&reason, 48)
                ));
            }
            SessionEvent::ModelRequestStarted { iteration } => {
                state.status = if iteration == 1 {
                    "Working".to_string()
                } else {
                    format!("Working ({iteration})")
                };
            }
            SessionEvent::TokenUsageUpdated { ledger, .. } => {
                state.session.token_ledger = ledger.clone();
                if let Some(window) = ledger.context_window {
                    state.push_activity(format!(
                        "context {} / {} tokens, input {} output {} prefill {} decode {} cache {}",
                        window.used_tokens,
                        window.max_tokens,
                        ledger.cumulative_usage.input_tokens,
                        ledger.cumulative_usage.output_tokens,
                        ledger.cumulative_usage.prefill_tokens,
                        ledger.cumulative_usage.decode_tokens,
                        ledger.cumulative_usage.cache_read_tokens,
                    ));
                }
            }
            SessionEvent::ModelResponseCompleted {
                tool_call_count, ..
            } => {
                self.active_assistant_line = None;
                state.status = if tool_call_count == 0 {
                    "Working".to_string()
                } else {
                    "Working".to_string()
                };
            }
            SessionEvent::ToolCallRequested { call } => {
                state.status = "Working".to_string();
                state.active_tool_label = Some(call.tool_name.clone());
                state.push_activity(format!("requested {}", call.tool_name));
            }
            SessionEvent::ToolApprovalRequested { call, reasons } => {
                state.status = "Waiting for approval".to_string();
                state.active_tool_label = Some(call.tool_name.clone());
                state.push_activity(format!(
                    "approval needed for {} ({})",
                    call.tool_name,
                    preview_text(&reasons.join("; "), 40)
                ));
            }
            SessionEvent::ToolApprovalResolved {
                call,
                approved,
                reason,
            } => {
                state.active_tool_label = None;
                if approved {
                    state.status = format!("Approved {}", call.tool_name);
                    state.push_transcript(format!(
                        "✔ You approved Code Agent to run {}",
                        call.tool_name
                    ));
                    state.push_activity(format!("approved {}", call.tool_name));
                } else {
                    let reason = reason.unwrap_or_else(|| "permission denied".to_string());
                    state.status = format!("Denied {}: {}", call.tool_name, reason);
                    state.push_transcript(format!(
                        "✗ You did not approve Code Agent to run {}\n  └ {}",
                        call.tool_name,
                        preview_text(&reason, 72)
                    ));
                    state.push_activity(format!(
                        "denied {}: {}",
                        call.tool_name,
                        preview_text(&reason, 44)
                    ));
                }
            }
            SessionEvent::ToolLifecycleStarted { call } => {
                state.status = "Working".to_string();
                state.active_tool_label = Some(call.tool_name.clone());
                state.push_transcript(format!("• Running {}", call.tool_name));
                self.active_tool_lines
                    .insert(call.call_id.clone(), state.transcript.len() - 1);
                state.push_activity(format!("running {}", call.tool_name));
            }
            SessionEvent::ToolLifecycleCompleted {
                call,
                output_preview,
                structured_output_preview,
            } => {
                state.status = format!("Completed {}", call.tool_name);
                state.active_tool_label = None;
                replace_tool_line(
                    state,
                    self.active_tool_lines.remove(&call.call_id),
                    completed_tool_entry(&call.tool_name, &output_preview),
                );
                state.push_activity(format!(
                    "{} -> {}",
                    call.tool_name,
                    preview_text(&output_preview, 44)
                ));
                if matches!(
                    call.tool_name.as_str(),
                    "task" | "task_batch" | "agent_wait" | "agent_spawn"
                ) {
                    if let Some(structured) = structured_output_preview {
                        state.push_activity(format!(
                            "{} structured {}",
                            call.tool_name,
                            preview_text(&structured, 44)
                        ));
                    }
                }
            }
            SessionEvent::ToolLifecycleFailed { call, error } => {
                state.status = format!("{} failed", call.tool_name);
                state.active_tool_label = None;
                replace_tool_line(
                    state,
                    self.active_tool_lines.remove(&call.call_id),
                    format!(
                        "✗ {} failed\n  └ {}",
                        call.tool_name,
                        preview_text(&error, 72)
                    ),
                );
                state.push_activity(format!(
                    "{} failed: {}",
                    call.tool_name,
                    preview_text(&error, 44)
                ));
            }
            SessionEvent::ToolLifecycleCancelled { call, reason } => {
                state.status = format!("{} cancelled", call.tool_name);
                state.active_tool_label = None;
                replace_tool_line(
                    state,
                    self.active_tool_lines.remove(&call.call_id),
                    format!(
                        "✗ Cancelled {}\n  └ {}",
                        call.tool_name,
                        reason
                            .as_deref()
                            .map(|value| preview_text(value, 72))
                            .unwrap_or_else(|| "cancelled".to_string())
                    ),
                );
                state.push_activity(format!(
                    "{} cancelled{}",
                    call.tool_name,
                    reason
                        .as_deref()
                        .map(|value| format!(": {}", preview_text(value, 44)))
                        .unwrap_or_default()
                ));
            }
            SessionEvent::TurnCompleted { .. } => {
                self.active_assistant_line = None;
                self.active_tool_lines.clear();
                state.active_tool_label = None;
                state.status = "Ready".to_string();
                state.push_activity("turn complete");
            }
        });
    }
}

fn completed_tool_entry(tool_name: &str, output_preview: &str) -> String {
    let output_preview = preview_text(output_preview, 96);
    if output_preview == "<empty>" {
        format!("• Called {tool_name}")
    } else {
        format!("• Called {tool_name}\n  └ {output_preview}")
    }
}

fn replace_tool_line(
    state: &mut super::state::TuiState,
    index: Option<usize>,
    replacement: String,
) {
    if let Some(index) = index {
        if let Some(line) = state.transcript.get_mut(index) {
            *line = replacement;
            state.transcript_scroll = u16::MAX;
            return;
        }
    }
    state.push_transcript(replacement);
}

#[cfg(test)]
mod tests {
    use super::SharedRenderObserver;
    use crate::backend::{SessionEvent, SessionToolCall};
    use crate::frontend::tui::state::SharedUiState;
    use agent::types::{ContextWindowUsage, TokenLedgerSnapshot, TokenUsage, TokenUsagePhase};

    #[test]
    fn token_usage_updates_are_persisted_into_session_state() {
        let ui_state = SharedUiState::new();
        let ledger = TokenLedgerSnapshot {
            context_window: Some(ContextWindowUsage {
                used_tokens: 64_000,
                max_tokens: 400_000,
            }),
            last_usage: Some(TokenUsage::from_input_output(4_000, 300, 500)),
            cumulative_usage: TokenUsage::from_input_output(20_000, 1_200, 3_000),
        };

        SharedRenderObserver::new(ui_state.clone()).apply_event(SessionEvent::TokenUsageUpdated {
            phase: TokenUsagePhase::ResponseCompleted,
            ledger: ledger.clone(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.session.token_ledger, ledger);
        assert!(
            snapshot
                .activity
                .last()
                .expect("token usage activity should be recorded")
                .contains(
                    "context 64000 / 400000 tokens, input 20000 output 1200 prefill 17000 decode 1200 cache 3000"
                )
        );
    }

    #[test]
    fn tool_lifecycle_events_are_projected_into_transcript_timeline() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "bash".to_string(),
            call_id: "call_123".to_string(),
            origin: "shell".to_string(),
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ModelRequestStarted { iteration: 1 });
        observer.apply_event(SessionEvent::ToolLifecycleStarted { call: call.clone() });
        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "listed files".to_string(),
            structured_output_preview: None,
        });

        let snapshot = ui_state.snapshot();
        assert!(snapshot.transcript.iter().all(|line| !line.contains('>')));
        assert!(
            snapshot
                .transcript
                .iter()
                .any(|line| line == "• Called bash\n  └ listed files")
        );
    }
}
