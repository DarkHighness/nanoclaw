use super::state::{PlanEntry, SharedUiState, TranscriptEntry, preview_text};
use crate::backend::SessionEvent;
use crate::tool_render::{prefixed_detail_lines, tool_output_detail_lines_from_preview};
use serde_json::Value;
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
                state.push_transcript(TranscriptEntry::shell_summary_entry(
                    "Applied steer",
                    &[format!(
                        "  └ {}{}",
                        message,
                        reason
                            .as_deref()
                            .map(|value| format!(" ({value})"))
                            .unwrap_or_default()
                    )],
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
                state.push_transcript(TranscriptEntry::UserPrompt(prompt.clone()));
                state.status = "Working".to_string();
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
            }
            SessionEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                ..
            } => {
                state.push_transcript(TranscriptEntry::shell_summary_entry(
                    "Compacted history",
                    &[format!(
                        "  └ kept {retained_message_count} of {source_message_count} messages"
                    )],
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
                let line_index = replace_or_push_tool_line(
                    state,
                    self.active_tool_lines.get(&call.call_id).copied(),
                    requested_tool_entry(&call),
                );
                self.active_tool_lines
                    .insert(call.call_id.clone(), line_index);
                state.push_activity(format!("requested {}", call.tool_name));
            }
            SessionEvent::ToolApprovalRequested { call, reasons } => {
                state.status = "Waiting for approval".to_string();
                state.active_tool_label = Some(call.tool_name.clone());
                let line_index = replace_or_push_tool_line(
                    state,
                    self.active_tool_lines.get(&call.call_id).copied(),
                    waiting_tool_entry(&call, &reasons),
                );
                self.active_tool_lines
                    .insert(call.call_id.clone(), line_index);
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
                    state.active_tool_label = Some(call.tool_name.clone());
                    state.push_activity(format!("approved {}", call.tool_name));
                } else {
                    let reason = reason.unwrap_or_else(|| "permission denied".to_string());
                    state.status = format!("Denied {}: {}", call.tool_name, reason);
                    state.active_tool_label = None;
                    let existing = self.active_tool_lines.remove(&call.call_id);
                    replace_tool_line(state, existing, denied_tool_entry(&call, &reason));
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
                let existing = self.active_tool_lines.get(&call.call_id).copied();
                let line_index =
                    replace_or_push_tool_line(state, existing, running_tool_entry(&call));
                self.active_tool_lines
                    .insert(call.call_id.clone(), line_index);
                state.push_activity(format!("running {}", call.tool_name));
            }
            SessionEvent::ToolLifecycleCompleted {
                call,
                output_preview,
                structured_output_preview,
            } => {
                state.status = format!("Completed {}", call.tool_name);
                state.active_tool_label = None;
                if let Some(plan_items) =
                    plan_items_from_output(&call.tool_name, structured_output_preview.as_deref())
                {
                    state.plan_items = plan_items;
                }
                replace_tool_line(
                    state,
                    self.active_tool_lines.remove(&call.call_id),
                    completed_tool_entry(
                        &call,
                        &output_preview,
                        structured_output_preview.as_deref(),
                    ),
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
                    failed_tool_entry(&call, &error),
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
                    cancelled_tool_entry(&call, reason.as_deref()),
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

fn requested_tool_entry(call: &crate::backend::SessionToolCall) -> TranscriptEntry {
    TranscriptEntry::shell_summary_entry(
        format!("Requested {}", call.tool_name),
        &tool_argument_detail_lines(call),
    )
}

fn denied_tool_entry(call: &crate::backend::SessionToolCall, reason: &str) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.push(format!("  └ {}", preview_text(reason, 72)));
    TranscriptEntry::error_summary_entry(format!("Denied {}", call.tool_name), &detail_lines)
}

fn waiting_tool_entry(
    call: &crate::backend::SessionToolCall,
    reasons: &[String],
) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    if let Some(reason) = reasons.first() {
        detail_lines.push(format!("  └ {}", preview_text(reason, 72)));
    }
    TranscriptEntry::shell_summary_entry(
        format!("Awaiting approval for {}", call.tool_name),
        &detail_lines,
    )
}

fn running_tool_entry(call: &crate::backend::SessionToolCall) -> TranscriptEntry {
    TranscriptEntry::shell_summary_entry(
        format!("Running {}", call.tool_name),
        &tool_argument_detail_lines(call),
    )
}

fn completed_tool_entry(
    call: &crate::backend::SessionToolCall,
    output_preview: &str,
    structured_output_preview: Option<&str>,
) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.extend(tool_output_detail_lines_from_preview(
        &call.tool_name,
        output_preview,
        structured_output_preview,
    ));
    TranscriptEntry::shell_summary_entry(format!("Finished {}", call.tool_name), &detail_lines)
}

fn failed_tool_entry(call: &crate::backend::SessionToolCall, error: &str) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.push(format!("  └ {}", preview_text(error, 72)));
    TranscriptEntry::error_summary_entry(format!("{} failed", call.tool_name), &detail_lines)
}

fn cancelled_tool_entry(
    call: &crate::backend::SessionToolCall,
    reason: Option<&str>,
) -> TranscriptEntry {
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.push(format!(
        "  └ {}",
        reason
            .map(|value| preview_text(value, 72))
            .unwrap_or_else(|| "cancelled".to_string())
    ));
    TranscriptEntry::error_summary_entry(format!("Cancelled {}", call.tool_name), &detail_lines)
}

fn tool_argument_detail_lines(call: &crate::backend::SessionToolCall) -> Vec<String> {
    prefixed_detail_lines(&call.arguments_preview)
}

fn plan_items_from_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<Vec<PlanEntry>> {
    if tool_name != "update_plan" {
        return None;
    }
    let value = serde_json::from_str::<Value>(structured_output_preview?).ok()?;
    let items = value.get("items")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|item| {
                let step = item.get("step")?.as_str()?.to_string();
                Some(PlanEntry {
                    id: step.clone(),
                    content: step,
                    status: item.get("status")?.as_str()?.to_string(),
                })
            })
            .collect(),
    )
}

fn replace_tool_line(
    state: &mut super::state::TuiState,
    index: Option<usize>,
    replacement: TranscriptEntry,
) {
    if let Some(index) = index {
        if state.replace_transcript(index, replacement.clone()) {
            return;
        }
    }
    state.push_transcript(replacement);
}

// Approval, execution, and terminal tool states intentionally share one
// transcript cell so the operator reads one progressing operation instead of
// a stream of disconnected status fragments.
fn replace_or_push_tool_line(
    state: &mut super::state::TuiState,
    index: Option<usize>,
    replacement: TranscriptEntry,
) -> usize {
    if let Some(index) = index {
        if state.replace_transcript(index, replacement.clone()) {
            return index;
        }
    }

    state.push_transcript(replacement);
    state.transcript.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::SharedRenderObserver;
    use crate::backend::{SessionEvent, SessionToolCall};
    use crate::frontend::tui::state::SharedUiState;
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
    fn tool_lifecycle_events_are_projected_into_transcript_timeline() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "bash".to_string(),
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
            == "• Finished bash\n  └ $ ls\n  └ exit 0\n  └ listed files"));
    }

    #[test]
    fn tool_request_and_completion_share_one_timeline_cell() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "bash".to_string(),
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
            "• Finished bash\n  └ $ ls\n  └ exit 0\n  └ listed files"
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
                    "items": [
                        {"step": "Inspect repo", "status": "completed"},
                        {"step": "Refine TUI", "status": "in_progress"}
                    ]
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.plan_items.len(), 2);
        assert_eq!(snapshot.plan_items[1].content, "Refine TUI");
        assert_eq!(snapshot.plan_items[1].status, "in_progress");
    }

    #[test]
    fn approval_and_tool_run_share_one_timeline_cell() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "bash".to_string(),
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
            "• Finished bash\n  └ $ cargo test\n  └ exit 0\n  └ ok"
        );
    }

    #[test]
    fn bash_failures_prefer_tail_output_preview() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "bash".to_string(),
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
        assert!(transcript.contains("  └ exit 1"));
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
        assert!(transcript_text(&snapshot.transcript[0]).contains("  └ diff src/lib.rs"));
        assert!(transcript_text(&snapshot.transcript[0]).contains("@@ -1,1 +1,1 @@"));
        assert!(transcript_text(&snapshot.transcript[0]).contains("+new()"));
    }

    fn transcript_text(entry: &crate::frontend::tui::state::TranscriptEntry) -> String {
        entry.serialized()
    }
}
