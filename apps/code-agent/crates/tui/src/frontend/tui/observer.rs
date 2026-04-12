use super::state::{
    ActiveToolEntry, SharedUiState, ToastTone, TranscriptEntry, TranscriptShellDetail,
    TranscriptToolEntry, TranscriptToolStatus, TurnPhase, preview_text,
};
use super::tool_state::{
    execution_state_from_tool_output, execution_update_entry_from_tool_output,
    plan_items_from_tool_output, plan_update_entry_from_tool_output,
};
use crate::tool_render::{
    ToolDetail, ToolDetailLabel, tool_argument_details, tool_output_details_from_preview,
    tool_review_from_preview,
};
use crate::ui::{SessionEvent, SessionToolCall};

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

    pub(crate) fn apply_event(&mut self, event: SessionEvent) {
        self.ui_state.mutate(|state| match event {
            SessionEvent::SteerApplied { message, reason } => {
                state.push_transcript(TranscriptEntry::shell_summary_details(
                    "Applied steer",
                    vec![TranscriptShellDetail::Raw {
                        text: format!(
                            "{}{}",
                            message,
                            reason
                                .as_deref()
                                .map(|value| format!(" ({value})"))
                                .unwrap_or_default()
                        ),
                        continuation: false,
                    }],
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
                state.active_tools.clear();
                state.clear_tool_selection();
                state.push_transcript(TranscriptEntry::UserPrompt(prompt.clone()));
                state.status = "Working".to_string();
                state.turn_phase = TurnPhase::Working;
                state.push_activity(format!("user prompt: {}", preview_text(&prompt, 40)));
            }
            SessionEvent::AssistantTextDelta { delta } => {
                if let Some(index) = self.active_assistant_line {
                    if !state.append_transcript_text(index, &delta) {
                        state.push_transcript(TranscriptEntry::AssistantMessage(delta.clone()));
                        self.active_assistant_line = Some(state.transcript.len() - 1);
                    }
                } else {
                    state.push_transcript(TranscriptEntry::AssistantMessage(delta.clone()));
                    self.active_assistant_line = Some(state.transcript.len() - 1);
                }
                state.status = "Working".to_string();
                state.turn_phase = TurnPhase::Working;
            }
            SessionEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                ..
            } => {
                state.push_transcript(TranscriptEntry::shell_summary_details(
                    "Compacted history",
                    vec![TranscriptShellDetail::Raw {
                        text: format!(
                            "kept {retained_message_count} of {source_message_count} messages"
                        ),
                        continuation: false,
                    }],
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
                state.turn_phase = TurnPhase::Working;
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
            SessionEvent::Notification { source, message } => {
                let tone = notification_toast_tone(&source, &message);
                state.push_transcript(notification_entry(&source, &message, tone));
                state.show_toast(tone, format!("{source}: {}", preview_text(&message, 88)));
                state.push_activity(format!(
                    "notification {}: {}",
                    source,
                    preview_text(&message, 48)
                ));
            }
            SessionEvent::TuiToastShow { variant, message } => {
                state.show_toast(map_ui_toast_tone(variant), message.clone());
                state.push_activity(format!("tui toast: {}", preview_text(&message, 48)));
            }
            SessionEvent::TuiPromptAppend {
                text,
                only_when_empty,
            } => {
                if !only_when_empty
                    || (state.input.is_empty() && state.draft_attachments.is_empty())
                {
                    state.append_input_text(&text);
                    state.push_activity(format!("tui prompt append: {}", preview_text(&text, 48)));
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
                state.turn_phase = TurnPhase::Working;
            }
            SessionEvent::ToolCallRequested { call } => {
                state.status = "Working".to_string();
                state.turn_phase = TurnPhase::Working;
                upsert_active_tool(state, &call.call_id, requested_tool_card(&call));
                state.push_activity(format!("requested {}", call.tool_name));
            }
            SessionEvent::ToolApprovalRequested { call, reasons } => {
                state.status = "Waiting for approval".to_string();
                state.turn_phase = TurnPhase::WaitingApproval;
                upsert_active_tool(state, &call.call_id, waiting_tool_card(&call, &reasons));
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
                if approved {
                    state.status = "Working".to_string();
                    state.turn_phase = TurnPhase::Working;
                    upsert_active_tool(
                        state,
                        &call.call_id,
                        approved_tool_card(&call, reason.as_deref()),
                    );
                    state.push_activity(format!("approved {}", call.tool_name));
                } else {
                    let reason = reason.unwrap_or_else(|| "permission denied".to_string());
                    state.status = format!("Denied {}: {}", call.tool_name, reason);
                    state.turn_phase = TurnPhase::Failed;
                    remove_active_tool(state, &call.call_id);
                    let transcript_index = state.transcript.len();
                    state.push_transcript(denied_tool_entry(&call, &reason));
                    state.promote_live_tool_selection(&call.call_id, transcript_index);
                    state.push_activity(format!(
                        "denied {}: {}",
                        call.tool_name,
                        preview_text(&reason, 44)
                    ));
                }
            }
            SessionEvent::ToolLifecycleStarted { call } => {
                state.status = "Working".to_string();
                state.turn_phase = TurnPhase::Working;
                upsert_active_tool(state, &call.call_id, running_tool_card(&call));
                state.push_activity(format!("running {}", call.tool_name));
            }
            SessionEvent::ToolLifecycleCompleted {
                call,
                output_preview,
                structured_output_preview,
            } => {
                state.status = format!("Completed {}", call.tool_name);
                state.turn_phase = TurnPhase::Working;
                if let Some(plan_items) = plan_items_from_tool_output(
                    &call.tool_name,
                    structured_output_preview.as_deref(),
                ) {
                    state.plan_items = plan_items;
                }
                if let Some(execution) = execution_state_from_tool_output(
                    &call.tool_name,
                    structured_output_preview.as_deref(),
                ) {
                    state.execution = execution;
                }
                remove_active_tool(state, &call.call_id);
                let transcript_index = state.transcript.len();
                state.push_transcript(completed_tool_entry(
                    &call,
                    &output_preview,
                    structured_output_preview.as_deref(),
                ));
                state.promote_live_tool_selection(&call.call_id, transcript_index);
                state.push_activity(format!(
                    "{} -> {}",
                    call.tool_name,
                    preview_text(&output_preview, 44)
                ));
                if matches!(
                    call.tool_name.as_str(),
                    "task"
                        | "task_batch"
                        | "spawn_agent"
                        | "wait_agent"
                        | "resume_agent"
                        | "agent_spawn"
                        | "agent_wait"
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
                state.turn_phase = TurnPhase::Failed;
                remove_active_tool(state, &call.call_id);
                let transcript_index = state.transcript.len();
                state.push_transcript(failed_tool_entry(&call, &error));
                state.promote_live_tool_selection(&call.call_id, transcript_index);
                state.push_activity(format!(
                    "{} failed: {}",
                    call.tool_name,
                    preview_text(&error, 44)
                ));
            }
            SessionEvent::ToolLifecycleCancelled { call, reason } => {
                state.status = format!("{} cancelled", call.tool_name);
                state.turn_phase = TurnPhase::Failed;
                remove_active_tool(state, &call.call_id);
                let transcript_index = state.transcript.len();
                state.push_transcript(cancelled_tool_entry(&call, reason.as_deref()));
                state.promote_live_tool_selection(&call.call_id, transcript_index);
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
                state.active_tools.clear();
                state.clear_missing_live_tool_selection();
                state.status = "Ready".to_string();
                state.turn_phase = TurnPhase::Idle;
                state.push_activity("turn complete");
            }
        });
    }
}

fn requested_tool_card(call: &SessionToolCall) -> TranscriptToolEntry {
    TranscriptToolEntry::new(
        TranscriptToolStatus::Requested,
        call.tool_name.clone(),
        tool_argument_detail_lines(call),
    )
}

fn approved_tool_card(call: &SessionToolCall, reason: Option<&str>) -> TranscriptToolEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    if let Some(reason) = reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Reason,
            value: preview_text(reason, 72),
        });
    }
    TranscriptToolEntry::new(
        TranscriptToolStatus::Approved,
        call.tool_name.clone(),
        detail_lines,
    )
}

fn notification_entry(source: &str, message: &str, tone: ToastTone) -> TranscriptEntry {
    let detail_lines = vec![TranscriptShellDetail::Raw {
        text: message.to_string(),
        continuation: false,
    }];
    match tone {
        ToastTone::Success => TranscriptEntry::success_summary_details(
            format!("Notification from {source}"),
            detail_lines,
        ),
        ToastTone::Warning => TranscriptEntry::warning_summary_details(
            format!("Notification from {source}"),
            detail_lines,
        ),
        ToastTone::Error => TranscriptEntry::error_summary_details(
            format!("Notification from {source}"),
            detail_lines,
        ),
        ToastTone::Info => TranscriptEntry::shell_summary_details(
            format!("Notification from {source}"),
            detail_lines,
        ),
    }
}

fn notification_toast_tone(source: &str, message: &str) -> ToastTone {
    if source == "loop_detector" {
        if message.contains("[critical]") || message.contains("blocked") {
            ToastTone::Error
        } else {
            ToastTone::Warning
        }
    } else if source == "provider_state" {
        ToastTone::Warning
    } else {
        ToastTone::Info
    }
}

fn map_ui_toast_tone(variant: &str) -> ToastTone {
    match variant {
        "success" => ToastTone::Success,
        "warning" => ToastTone::Warning,
        "error" => ToastTone::Error,
        _ => ToastTone::Info,
    }
}

fn denied_tool_entry(call: &SessionToolCall, reason: &str) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.push(ToolDetail::LabeledValue {
        label: ToolDetailLabel::Reason,
        value: preview_text(reason, 72),
    });
    TranscriptEntry::tool(
        TranscriptToolStatus::Denied,
        call.tool_name.clone(),
        detail_lines,
    )
}

fn waiting_tool_card(call: &SessionToolCall, reasons: &[String]) -> TranscriptToolEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    if let Some(reason) = reasons.first() {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Reason,
            value: preview_text(reason, 72),
        });
    }
    TranscriptToolEntry::new(
        TranscriptToolStatus::WaitingApproval,
        call.tool_name.clone(),
        detail_lines,
    )
}

fn running_tool_card(call: &SessionToolCall) -> TranscriptToolEntry {
    TranscriptToolEntry::new(
        TranscriptToolStatus::Running,
        call.tool_name.clone(),
        tool_argument_detail_lines(call),
    )
}

fn completed_tool_entry(
    call: &SessionToolCall,
    output_preview: &str,
    structured_output_preview: Option<&str>,
) -> TranscriptEntry {
    if let Some(plan_entry) =
        plan_update_entry_from_tool_output(&call.tool_name, structured_output_preview)
    {
        return plan_entry;
    }
    if let Some(execution_entry) =
        execution_update_entry_from_tool_output(&call.tool_name, structured_output_preview)
    {
        return execution_entry;
    }

    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.extend(tool_output_details_from_preview(
        &call.tool_name,
        output_preview,
        structured_output_preview,
    ));
    TranscriptEntry::tool_with_review(
        TranscriptToolStatus::Finished,
        call.tool_name.clone(),
        detail_lines,
        tool_review_from_preview(&call.tool_name, structured_output_preview),
    )
}

fn failed_tool_entry(call: &SessionToolCall, error: &str) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.push(ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: preview_text(error, 72),
    });
    TranscriptEntry::tool(
        TranscriptToolStatus::Failed,
        call.tool_name.clone(),
        detail_lines,
    )
}

fn cancelled_tool_entry(call: &SessionToolCall, reason: Option<&str>) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.push(ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: reason
            .map(|value| preview_text(value, 72))
            .unwrap_or_else(|| "cancelled".to_string()),
    });
    TranscriptEntry::tool(
        TranscriptToolStatus::Cancelled,
        call.tool_name.clone(),
        detail_lines,
    )
}

fn tool_argument_detail_lines(call: &SessionToolCall) -> Vec<ToolDetail> {
    tool_argument_details(&call.arguments_preview)
}

fn upsert_active_tool(
    state: &mut super::state::TuiState,
    call_id: &str,
    entry: TranscriptToolEntry,
) {
    if let Some(existing) = state
        .active_tools
        .iter_mut()
        .find(|active| active.call_id == call_id)
    {
        existing.entry = entry;
        return;
    }

    state.active_tools.push(ActiveToolEntry {
        call_id: call_id.to_string(),
        entry,
    });
}

fn remove_active_tool(state: &mut super::state::TuiState, call_id: &str) {
    if let Some(index) = state
        .active_tools
        .iter()
        .position(|active| active.call_id == call_id)
    {
        state.active_tools.remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::SharedRenderObserver;
    use crate::frontend::tui::state::{SharedUiState, ToolSelectionTarget};
    use crate::ui::{SessionEvent, SessionToolCall};
    use agent::types::{ContextWindowUsage, TokenLedgerSnapshot, TokenUsage, TokenUsagePhase};
    use serde_json::json;

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
    fn notifications_surface_transcript_activity_and_toast() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::Notification {
            source: "loop_detector".to_string(),
            message: "loop_detector [warning] repeated tool calls".to_string(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "⚠ Notification from loop_detector\n  └ loop_detector [warning] repeated tool calls"
        );
        assert_eq!(
            snapshot.toast.as_ref().map(|toast| toast.message.as_str()),
            Some("loop_detector: loop_detector [warning] repeated tool calls")
        );
        assert!(
            snapshot
                .activity
                .last()
                .expect("notification activity should be recorded")
                .contains("notification loop_detector")
        );
    }

    #[test]
    fn tui_toast_events_surface_transient_toasts() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::tui_success_toast("background result ready"));

        let snapshot = ui_state.snapshot();
        assert_eq!(
            snapshot.toast.as_ref().map(|toast| toast.message.as_str()),
            Some("background result ready")
        );
        assert!(
            snapshot
                .activity
                .last()
                .expect("toast activity should be recorded")
                .contains("tui toast")
        );
    }

    #[test]
    fn tui_prompt_append_only_when_empty_respects_existing_drafts() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::TuiPromptAppend {
            text: "queued follow-up".to_string(),
            only_when_empty: true,
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.input, "queued follow-up");

        ui_state.mutate(|state| state.replace_input("existing"));
        observer.apply_event(SessionEvent::TuiPromptAppend {
            text: " + appended".to_string(),
            only_when_empty: true,
        });
        observer.apply_event(SessionEvent::TuiPromptAppend {
            text: " + appended".to_string(),
            only_when_empty: false,
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.input, "existing + appended");
    }

    #[test]
    fn tool_lifecycle_events_are_projected_into_transcript_timeline() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: "shell".to_string(),
            arguments_preview: vec!["$ ls".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ModelRequestStarted { iteration: 1 });
        observer.apply_event(SessionEvent::ToolLifecycleStarted { call: call.clone() });
        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "listed files".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 0,
                    "timed_out": false,
                    "stdout": {"text": "listed files", "chars": 12, "truncated": false},
                    "stderr": {"text": "", "chars": 0, "truncated": false}
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert!(
            snapshot
                .transcript
                .iter()
                .all(|line| !transcript_text(line).contains('>'))
        );
        assert!(snapshot.transcript.iter().any(|line| transcript_text(line)
            == "• Finished exec_command\n  └ $ ls\n  └ result exit 0\n  └ stdout\n    listed files"));
    }

    #[test]
    fn tool_request_and_completion_share_one_timeline_cell() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: "shell".to_string(),
            arguments_preview: vec!["$ ls".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolCallRequested { call: call.clone() });
        observer.apply_event(SessionEvent::ToolLifecycleStarted { call: call.clone() });
        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "listed files".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 0,
                    "timed_out": false,
                    "stdout": {"text": "listed files", "chars": 12, "truncated": false},
                    "stderr": {"text": "", "chars": 0, "truncated": false}
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Finished exec_command\n  └ $ ls\n  └ result exit 0\n  └ stdout\n    listed files"
        );
    }

    #[test]
    fn completed_live_tool_keeps_selection_on_the_committed_result() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: "shell".to_string(),
            arguments_preview: vec!["$ ls".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleStarted { call: call.clone() });
        ui_state.mutate(|state| {
            state.tool_selection = Some(ToolSelectionTarget::Live("call_123".to_string()));
        });
        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "listed files".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 0,
                    "timed_out": false,
                    "stdout": {"text": "listed files", "chars": 12, "truncated": false},
                    "stderr": {"text": "", "chars": 0, "truncated": false}
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(
            snapshot.tool_selection,
            Some(ToolSelectionTarget::Transcript(0))
        );
    }

    #[test]
    fn update_plan_results_update_side_rail_snapshot() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "update_plan".to_string(),
            call_id: "call_123".to_string(),
            origin: "local".to_string(),
            arguments_preview: vec!["set 2 plan step(s)".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "updated todos".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "success",
                    "warnings": ["demoted 1 extra in_progress step(s) to pending"],
                    "items": [
                        {"step": "Inspect repo", "status": "completed"},
                        {"step": "Refine TUI", "status": "in_progress"}
                    ]
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Updated Plan\n  └ warning demoted 1 extra in_progress step(s) to pending\n  └ [x] Inspect repo\n  └ [~] Refine TUI"
        );
        assert_eq!(snapshot.plan_items.len(), 2);
        assert_eq!(snapshot.plan_items[1].content, "Refine TUI");
        assert_eq!(
            snapshot.plan_items[1].status,
            crate::frontend::tui::state::PlanEntryStatus::InProgress
        );
    }

    #[test]
    fn update_execution_results_update_side_rail_snapshot() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "update_execution".to_string(),
            call_id: "call_exec".to_string(),
            origin: "local".to_string(),
            arguments_preview: vec!["set execution snapshot".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "updated execution".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "success",
                    "action": "set",
                    "scope": {"label": "root session"},
                    "state": {
                        "status": "verifying",
                        "summary": "Run focused regression checks",
                        "next_action": "Inspect failures",
                        "verification": "cargo test -p code-agent"
                    }
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Updated Execution\n  └ [~] Run focused regression checks\n  └ scope root session\n  └ next Inspect failures\n  └ verify cargo test -p code-agent"
        );
        assert_eq!(
            snapshot.execution.as_ref().map(|entry| &entry.status),
            Some(&crate::frontend::tui::state::ExecutionStatus::Verifying)
        );
        assert_eq!(
            snapshot
                .execution
                .as_ref()
                .map(|entry| entry.scope_label.as_str()),
            Some("root session")
        );
    }

    #[test]
    fn approval_and_tool_run_share_one_timeline_cell() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: "local".to_string(),
            arguments_preview: vec!["$ cargo test".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolApprovalRequested {
            call: call.clone(),
            reasons: vec!["sandbox approval required".to_string()],
        });
        observer.apply_event(SessionEvent::ToolApprovalResolved {
            call: call.clone(),
            approved: true,
            reason: None,
        });
        observer.apply_event(SessionEvent::ToolLifecycleStarted { call: call.clone() });
        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "ok".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 0,
                    "timed_out": false,
                    "stdout": {"text": "ok", "chars": 2, "truncated": false},
                    "stderr": {"text": "", "chars": 0, "truncated": false}
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Finished exec_command\n  └ $ cargo test\n  └ result exit 0\n  └ stdout\n    ok"
        );
    }

    #[test]
    fn exec_command_failures_prefer_tail_output_preview() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_999".to_string(),
            origin: "local".to_string(),
            arguments_preview: vec!["$ cargo test".to_string()],
        };
        let stderr = (1..=20)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: stderr.clone(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 1,
                    "timed_out": false,
                    "stdout": {"text": "", "chars": 0, "truncated": false},
                    "stderr": {"text": stderr, "chars": 50, "truncated": false}
                })
                .to_string(),
            ),
        });

        let transcript = transcript_text(&ui_state.snapshot().transcript[0]);
        assert!(transcript.contains("  └ result exit 1"));
        assert!(transcript.contains("  └ stderr"));
        assert!(transcript.contains("… +"));
        assert!(transcript.contains("    20"));
        assert!(!transcript.contains("    1\n"));
    }

    #[test]
    fn file_tools_surface_diff_blocks_in_live_transcript() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "write".to_string(),
            call_id: "call_456".to_string(),
            origin: "local".to_string(),
            arguments_preview: vec!["src/lib.rs".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call,
            output_preview: "Wrote 18 bytes to src/lib.rs".to_string(),
            structured_output_preview: Some(
                json!({
                    "kind": "success",
                    "requested_path": "src/lib.rs",
                    "resolved_path": "/workspace/src/lib.rs",
                    "summary": "Wrote 18 bytes to src/lib.rs",
                    "snapshot_before": "snap_old",
                    "snapshot_after": "snap_new",
                    "file_diffs": [{
                        "path": "src/lib.rs",
                        "preview": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()"
                    }],
                    "write": {
                        "command": "write",
                        "path": "src/lib.rs"
                    }
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        let transcript = transcript_text(&snapshot.transcript[0]);
        assert!(transcript.contains("  └ files src/lib.rs"));
        assert!(transcript.contains("action [r] review diff"));
        assert!(
            snapshot.transcript[0]
                .tool_entry()
                .and_then(|entry| entry.review.as_ref())
                .is_some()
        );
    }

    fn transcript_text(entry: &crate::frontend::tui::state::TranscriptEntry) -> String {
        entry.serialized()
    }
}
