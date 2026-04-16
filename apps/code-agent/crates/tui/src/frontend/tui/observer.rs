use super::state::{
    ActiveToolCell, ProviderRetryState, SharedUiState, ToastTone, TranscriptEntry,
    TranscriptShellDetail, TranscriptShellEntry, TranscriptToolEntry, TranscriptToolStatus,
    TurnPhase, preview_text,
};
use super::task_state::{
    apply_subagent_started, apply_subagent_stopped, apply_task_completed, apply_task_created,
    apply_task_updated,
};
use crate::tool_render::{
    ToolDetail, ToolDetailLabel, compact_successful_exploration_details, tool_argument_details,
    tool_completion_state_from_preview, tool_output_details_from_preview, tool_review_from_preview,
};
use crate::ui::{SessionEvent, SessionNotificationSource, SessionToastVariant, SessionToolCall};
use agent::types::{
    AgentHandle, AgentResultEnvelope, AgentTaskSpec, BrowserSummaryRecord, MonitorEventKind,
    MonitorStatus, MonitorStream, MonitorSummaryRecord, TaskId, TaskStatus, WorktreeStatus,
    WorktreeSummaryRecord,
};
use std::time::Instant;

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
                flush_transcript_ready_tool_cells(state);
                state.active_tool_cells.clear();
                state.clear_tool_selection();
                state.provider_retry = None;
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
                state.provider_retry = None;
                state.status = working_status_label(iteration);
                state.turn_phase = TurnPhase::Working;
            }
            SessionEvent::ProviderRetryScheduled {
                iteration,
                status_code,
                retry_count,
                max_retries,
                remaining_retries,
                next_retry_at_ms,
            } => {
                state.status = working_status_label(iteration);
                state.provider_retry = Some(ProviderRetryState {
                    iteration,
                    status_code,
                    retry_count,
                    max_retries,
                    remaining_retries,
                    next_retry_at_ms,
                });
                state.turn_phase = TurnPhase::Working;
                state.push_activity(format!(
                    "provider retry status {status_code}: retry {retry_count}/{max_retries}, {remaining_retries} left"
                ));
            }
            SessionEvent::TokenUsageUpdated { ledger, .. } => {
                state.session.token_ledger = ledger.clone();
                if let Some(window) = ledger.context_window {
                    state.push_activity(format!(
                        "context {} / {} tokens, input {} output {} prefill {} decode {} cache {} reasoning {}",
                        window.used_tokens,
                        window.max_tokens,
                        ledger.cumulative_usage.input_tokens,
                        ledger.cumulative_usage.output_tokens,
                        ledger.cumulative_usage.prefill_tokens,
                        ledger.cumulative_usage.decode_tokens,
                        ledger.cumulative_usage.cache_read_tokens,
                        ledger.cumulative_usage.reasoning_tokens,
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
                upsert_active_tool_cell(state, &call.call_id, requested_tool_card(&call));
                state.push_activity(format!("requested {}", call.tool_name));
            }
            SessionEvent::ToolApprovalRequested { call, reasons } => {
                state.status = "Waiting for approval".to_string();
                state.turn_phase = TurnPhase::WaitingApproval;
                upsert_active_tool_cell(state, &call.call_id, waiting_tool_card(&call, &reasons));
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
                    upsert_active_tool_cell(
                        state,
                        &call.call_id,
                        approved_tool_card(&call, reason.as_deref()),
                    );
                    state.push_activity(format!("approved {}", call.tool_name));
                } else {
                    let reason = reason.unwrap_or_else(|| "permission denied".to_string());
                    state.status = format!("Denied {}: {}", call.tool_name, reason);
                    state.turn_phase = TurnPhase::Failed;
                    let denied_entry = denied_tool_entry(&call, &reason);
                    let selection_id = remove_active_tool_call(state, &call.call_id)
                        .map(|(cell_id, _)| cell_id)
                        .unwrap_or_else(|| call.call_id.clone());
                    let transcript_index = state.push_transcript(denied_entry);
                    state.promote_live_tool_selection(&selection_id, transcript_index);
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
                start_active_tool_cell(state, &call.call_id, running_tool_card(&call));
                state.push_activity(format!("running {}", call.tool_name));
            }
            SessionEvent::ToolLifecycleCompleted {
                call,
                output_preview,
                structured_output_preview,
            } => {
                state.status = format!("Completed {}", call.tool_name);
                state.turn_phase = TurnPhase::Working;
                let completed_entry = completed_tool_entry(
                    &call,
                    &output_preview,
                    structured_output_preview.as_deref(),
                );
                match completed_entry {
                    TranscriptEntry::Tool(entry) => {
                        if let Some((cell_id, entry)) =
                            complete_active_tool_call(state, &call.call_id, entry)
                        {
                            let transcript_index = state.push_transcript(entry);
                            state.promote_live_tool_selection(&cell_id, transcript_index);
                        }
                    }
                    other => {
                        let selection_id = remove_active_tool_call(state, &call.call_id)
                            .map(|(cell_id, _)| cell_id)
                            .unwrap_or_else(|| call.call_id.clone());
                        let transcript_index = state.push_transcript(other);
                        state.promote_live_tool_selection(&selection_id, transcript_index);
                    }
                }
                state.push_activity(format!(
                    "{} -> {}",
                    call.tool_name,
                    preview_text(&output_preview, 44)
                ));
                if matches!(
                    call.tool_name.as_str(),
                    "task_create"
                        | "task_get"
                        | "task_list"
                        | "task_update"
                        | "task_stop"
                        | "spawn_agent"
                        | "wait_agent"
                        | "resume_agent"
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
                let failed_entry = failed_tool_entry(&call, &error);
                let selection_id = remove_active_tool_call(state, &call.call_id)
                    .map(|(cell_id, _)| cell_id)
                    .unwrap_or_else(|| call.call_id.clone());
                let transcript_index = state.push_transcript(failed_entry);
                state.promote_live_tool_selection(&selection_id, transcript_index);
                state.push_activity(format!(
                    "{} failed: {}",
                    call.tool_name,
                    preview_text(&error, 44)
                ));
            }
            SessionEvent::ToolLifecycleCancelled { call, reason } => {
                state.status = format!("{} cancelled", call.tool_name);
                state.turn_phase = TurnPhase::Failed;
                let cancelled_entry = cancelled_tool_entry(&call, reason.as_deref());
                let selection_id = remove_active_tool_call(state, &call.call_id)
                    .map(|(cell_id, _)| cell_id)
                    .unwrap_or_else(|| call.call_id.clone());
                let transcript_index = state.push_transcript(cancelled_entry);
                state.promote_live_tool_selection(&selection_id, transcript_index);
                state.push_activity(format!(
                    "{} cancelled{}",
                    call.tool_name,
                    reason
                        .as_deref()
                        .map(|value| format!(": {}", preview_text(value, 44)))
                        .unwrap_or_default()
                ));
            }
            SessionEvent::BrowserOpened { summary } => {
                state.push_transcript(browser_entry(
                    format!("Opened browser {}", summary.browser_id),
                    &summary,
                ));
                state.status = format!("Browser {}", summary.browser_id);
                state.push_activity(format!(
                    "opened browser {} {}",
                    summary.browser_id,
                    preview_text(&summary.current_url, 48)
                ));
            }
            SessionEvent::BrowserUpdated { summary } => {
                state.push_transcript(browser_entry(
                    format!(
                        "{} browser {}",
                        browser_status_label(summary.status),
                        summary.browser_id
                    ),
                    &summary,
                ));
                state.status = format!("Browser {} {}", summary.browser_id, summary.status);
                state.push_activity(format!("browser {} {}", summary.browser_id, summary.status));
            }
            SessionEvent::MonitorStarted { summary } => {
                state.upsert_active_monitor(
                    summary.monitor_id.to_string(),
                    Instant::now(),
                    running_monitor_entry(&summary),
                );
                state.push_activity(format!(
                    "started monitor {}: {}",
                    summary.monitor_id,
                    preview_text(&summary.command, 48)
                ));
            }
            SessionEvent::MonitorEvent { event } => {
                if let Some(active) = state
                    .active_monitors
                    .iter_mut()
                    .find(|monitor| monitor.monitor_id == event.monitor_id.as_str())
                {
                    apply_monitor_event(active, &event.kind);
                }
                state.push_activity(match &event.kind {
                    MonitorEventKind::Line { stream, text } => format!(
                        "monitor {} {}: {}",
                        event.monitor_id,
                        stream,
                        preview_text(text, 44)
                    ),
                    MonitorEventKind::StateChanged { status } => {
                        format!("monitor {} state {}", event.monitor_id, status)
                    }
                    MonitorEventKind::Completed { exit_code } => {
                        format!("monitor {} completed ({exit_code})", event.monitor_id)
                    }
                    MonitorEventKind::Failed { exit_code, error } => format!(
                        "monitor {} failed{}{}",
                        event.monitor_id,
                        exit_code
                            .map(|code| format!(" ({code})"))
                            .unwrap_or_default(),
                        error
                            .as_deref()
                            .map(|value| format!(": {}", preview_text(value, 32)))
                            .unwrap_or_default()
                    ),
                    MonitorEventKind::Cancelled { reason } => format!(
                        "monitor {} cancelled{}",
                        event.monitor_id,
                        reason
                            .as_deref()
                            .map(|value| format!(": {}", preview_text(value, 32)))
                            .unwrap_or_default()
                    ),
                });
            }
            SessionEvent::MonitorUpdated { summary } => {
                let completed = state
                    .remove_active_monitor(summary.monitor_id.as_str())
                    .map(|mut entry| {
                        entry.status = Some(match summary.status {
                            MonitorStatus::Running => super::state::TranscriptShellStatus::Running,
                            MonitorStatus::Completed => {
                                super::state::TranscriptShellStatus::Completed
                            }
                            MonitorStatus::Failed => super::state::TranscriptShellStatus::Failed,
                            MonitorStatus::Cancelled => {
                                super::state::TranscriptShellStatus::Cancelled
                            }
                        });
                        entry.headline = format!(
                            "{} monitor {}",
                            match summary.status {
                                MonitorStatus::Running => "Running",
                                MonitorStatus::Completed => "Completed",
                                MonitorStatus::Failed => "Failed",
                                MonitorStatus::Cancelled => "Cancelled",
                            },
                            summary.monitor_id
                        );
                        TranscriptEntry::ShellSummary(entry)
                    })
                    .unwrap_or_else(|| monitor_terminal_entry(&summary));
                state.push_transcript(completed);
                state.status = format!("Monitor {} {}", summary.monitor_id, summary.status);
                state.push_activity(format!("monitor {} {}", summary.monitor_id, summary.status));
            }
            SessionEvent::WorktreeEntered { summary } => {
                state.push_transcript(worktree_entry(
                    format!("Entered Worktree {}", summary.worktree_id),
                    &summary,
                ));
                state.status = format!("Worktree {}", summary.worktree_id);
                state.push_activity(format!(
                    "entered worktree {}",
                    preview_text(summary.root.display().to_string().as_str(), 48)
                ));
            }
            SessionEvent::WorktreeUpdated { summary } => {
                state.push_transcript(worktree_entry(
                    format!(
                        "{} Worktree {}",
                        worktree_status_label(summary.status),
                        summary.worktree_id
                    ),
                    &summary,
                ));
                state.status = format!("Worktree {} {}", summary.worktree_id, summary.status);
                state.push_activity(format!(
                    "worktree {} {}",
                    summary.worktree_id, summary.status
                ));
            }
            SessionEvent::TaskCreated {
                task,
                parent_agent_id,
                status,
                summary,
                ..
            } => {
                apply_task_created(
                    &mut state.tracked_tasks,
                    &task,
                    parent_agent_id.as_ref(),
                    status,
                    summary.clone(),
                );
                state.push_transcript(task_created_entry(&task, status, summary.as_deref()));
                state.status = format!("Tracked task {}", task.task_id);
                state.push_activity(format!("task created {}", task.task_id));
            }
            SessionEvent::TaskUpdated {
                task_id,
                status,
                summary,
            } => {
                apply_task_updated(&mut state.tracked_tasks, &task_id, status, summary.clone());
                state.push_transcript(task_updated_entry(&task_id, status, summary.as_deref()));
                state.status = format!("Task {} {}", task_id, status);
                state.push_activity(format!("task {} {}", task_id, status));
            }
            SessionEvent::TaskCompleted {
                task_id,
                agent_id,
                status,
            } => {
                apply_task_completed(&mut state.tracked_tasks, &task_id, &agent_id, status);
                state.push_transcript(task_completed_entry(&task_id, &agent_id, status));
                state.status = format!("Task {} {}", task_id, status);
                state.push_activity(format!("task {} {}", task_id, status));
            }
            SessionEvent::SubagentStarted { handle, task } => {
                apply_subagent_started(
                    &mut state.tracked_tasks,
                    handle.agent_id.as_str(),
                    &task,
                    &handle.status,
                );
                state.push_transcript(subagent_started_entry(&handle, &task));
                state.push_activity(format!("started {} task {}", handle.role, handle.task_id));
            }
            SessionEvent::SubagentStopped {
                handle,
                result,
                error,
            } => {
                apply_subagent_stopped(
                    &mut state.tracked_tasks,
                    &handle.task_id,
                    handle.agent_id.as_str(),
                    result.as_ref(),
                    error.as_deref(),
                );
                state.push_transcript(subagent_stopped_entry(
                    &handle,
                    result.as_ref(),
                    error.as_deref(),
                ));
                state.push_activity(format!("stopped task {}", handle.task_id));
            }
            SessionEvent::TurnCompleted { .. } => {
                self.active_assistant_line = None;
                flush_transcript_ready_tool_cells(state);
                state.active_tool_cells.clear();
                state.clear_missing_live_tool_selection();
                state.provider_retry = None;
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

fn notification_entry(
    source: &SessionNotificationSource,
    message: &str,
    tone: ToastTone,
) -> TranscriptEntry {
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

fn working_status_label(iteration: usize) -> String {
    if iteration == 1 {
        "Working".to_string()
    } else {
        format!("Working ({iteration})")
    }
}

fn notification_toast_tone(source: &SessionNotificationSource, message: &str) -> ToastTone {
    match source {
        SessionNotificationSource::LoopDetector => {
            if message.contains("[critical]") || message.contains("blocked") {
                ToastTone::Error
            } else {
                ToastTone::Warning
            }
        }
        SessionNotificationSource::ProviderState => ToastTone::Warning,
        SessionNotificationSource::Other(_) => ToastTone::Info,
    }
}

fn map_ui_toast_tone(variant: SessionToastVariant) -> ToastTone {
    match variant {
        SessionToastVariant::Info => ToastTone::Info,
        SessionToastVariant::Success => ToastTone::Success,
        SessionToastVariant::Warning => ToastTone::Warning,
        SessionToastVariant::Error => ToastTone::Error,
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
    let mut detail_lines = tool_argument_detail_lines(call);
    detail_lines.extend(tool_output_details_from_preview(
        &call.tool_name,
        output_preview,
        structured_output_preview,
    ));
    let completion = tool_completion_state_from_preview(&call.tool_name, structured_output_preview);
    compact_successful_exploration_details(&mut detail_lines, completion);
    TranscriptEntry::tool_with_review_and_completion(
        TranscriptToolStatus::Finished,
        call.tool_name.clone(),
        detail_lines,
        tool_review_from_preview(&call.tool_name, structured_output_preview),
        completion,
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

fn upsert_active_tool_cell(
    state: &mut super::state::TuiState,
    call_id: &str,
    entry: TranscriptToolEntry,
) {
    if let Some(existing) = state
        .active_tool_cells
        .iter_mut()
        .find(|active| active.contains_call(call_id))
    {
        let _ = existing.update_call(call_id, entry);
        return;
    }

    state
        .active_tool_cells
        .push(ActiveToolCell::new(call_id.to_string(), entry));
}

fn start_active_tool_cell(
    state: &mut super::state::TuiState,
    call_id: &str,
    entry: TranscriptToolEntry,
) {
    if let Some(index) = state
        .active_tool_cells
        .iter()
        .position(|active| active.contains_call(call_id))
    {
        if state.active_tool_cells[index].calls.len() > 1 {
            let _ = state.active_tool_cells[index].update_call(call_id, entry);
            return;
        }

        let previous_cell_id = state.active_tool_cells[index].cell_id.clone();
        state.active_tool_cells.remove(index);
        if let Some(target) = state
            .active_tool_cells
            .iter_mut()
            .find(|active| active.can_absorb_running_call(&entry))
        {
            let target_id = target.cell_id.clone();
            let _ = target.absorb_exploration_call(call_id.to_string(), entry);
            state.redirect_live_tool_selection(&previous_cell_id, &target_id);
            return;
        }

        state
            .active_tool_cells
            .push(ActiveToolCell::new_with_cell_id(
                previous_cell_id,
                call_id.to_string(),
                entry,
            ));
        return;
    }

    if let Some(target) = state
        .active_tool_cells
        .iter_mut()
        .find(|active| active.can_absorb_running_call(&entry))
    {
        let _ = target.absorb_exploration_call(call_id.to_string(), entry);
    } else {
        state
            .active_tool_cells
            .push(ActiveToolCell::new(call_id.to_string(), entry));
    }
}

fn complete_active_tool_call(
    state: &mut super::state::TuiState,
    call_id: &str,
    entry: TranscriptToolEntry,
) -> Option<(String, TranscriptEntry)> {
    let Some(index) = state
        .active_tool_cells
        .iter()
        .position(|active| active.contains_call(call_id))
    else {
        if let Some(target) = state
            .active_tool_cells
            .iter_mut()
            .find(|active| active.can_absorb_exploration_call(&entry))
        {
            let _ = target.absorb_exploration_call(call_id.to_string(), entry);
            return None;
        }
        let cell = ActiveToolCell::new(call_id.to_string(), entry.clone());
        if cell.holds_completed_entry() {
            state.active_tool_cells.push(cell);
            return None;
        }
        return Some((call_id.to_string(), TranscriptEntry::Tool(entry)));
    };
    if state.active_tool_cells[index].kind == super::state::ActiveToolCellKind::ExplorationGroup {
        let _ = state.active_tool_cells[index].update_call(call_id, entry);
        return None;
    }

    let cell_id = state.active_tool_cells[index].cell_id.clone();
    state.active_tool_cells.remove(index);
    Some((cell_id, TranscriptEntry::Tool(entry)))
}

fn remove_active_tool_call(
    state: &mut super::state::TuiState,
    call_id: &str,
) -> Option<(String, TranscriptToolEntry)> {
    let index = state
        .active_tool_cells
        .iter()
        .position(|active| active.contains_call(call_id))?;
    let cell_id = state.active_tool_cells[index].cell_id.clone();
    let removed = state.active_tool_cells[index].remove_call(call_id)?;
    if state.active_tool_cells[index].calls.is_empty() {
        state.active_tool_cells.remove(index);
    }
    Some((cell_id, removed))
}

fn flush_transcript_ready_tool_cells(state: &mut super::state::TuiState) {
    for (cell_id, entry) in state.drain_transcript_ready_tool_cells() {
        let transcript_index = state.push_transcript(entry);
        state.promote_live_tool_selection(&cell_id, transcript_index);
    }
}

fn browser_entry(headline: String, summary: &BrowserSummaryRecord) -> TranscriptEntry {
    TranscriptEntry::shell_summary_details(headline, browser_summary_details(summary))
}

fn browser_summary_details(summary: &BrowserSummaryRecord) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("status {}", summary.status),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("url {}", summary.current_url),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: if summary.headless {
                "mode headless".to_string()
            } else {
                "mode headful".to_string()
            },
            continuation: false,
        },
    ];
    if let Some(viewport) = summary.viewport.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("viewport {}x{}", viewport.width, viewport.height),
            continuation: false,
        });
    }
    if let Some(title) = summary
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(TranscriptShellDetail::Raw {
            text: format!("title {}", preview_text(title, 72)),
            continuation: false,
        });
    }
    if let Some(task_id) = summary.task_id.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("task {}", task_id),
            continuation: false,
        });
    }
    details
}

fn running_monitor_entry(summary: &MonitorSummaryRecord) -> TranscriptShellEntry {
    TranscriptShellEntry::new_with_status(
        format!("Running monitor {}", summary.monitor_id),
        Some(super::state::TranscriptShellStatus::Running),
        monitor_summary_details(summary),
    )
}

fn monitor_terminal_entry(summary: &MonitorSummaryRecord) -> TranscriptEntry {
    TranscriptEntry::shell_summary_status_details(
        match summary.status {
            MonitorStatus::Running => super::state::TranscriptShellStatus::Running,
            MonitorStatus::Completed => super::state::TranscriptShellStatus::Completed,
            MonitorStatus::Failed => super::state::TranscriptShellStatus::Failed,
            MonitorStatus::Cancelled => super::state::TranscriptShellStatus::Cancelled,
        },
        format!(
            "{} monitor {}",
            match summary.status {
                MonitorStatus::Running => "Running",
                MonitorStatus::Completed => "Completed",
                MonitorStatus::Failed => "Failed",
                MonitorStatus::Cancelled => "Cancelled",
            },
            summary.monitor_id
        ),
        monitor_summary_details(summary),
    )
}

fn monitor_summary_details(summary: &MonitorSummaryRecord) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("cwd {}", summary.cwd),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("command {}", summary.command),
            continuation: false,
        },
    ];
    if let Some(task_id) = summary.task_id.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("task {}", task_id),
            continuation: false,
        });
    }
    details
}

fn worktree_entry(headline: String, summary: &WorktreeSummaryRecord) -> TranscriptEntry {
    TranscriptEntry::shell_summary_details(headline, worktree_summary_details(summary))
}

fn worktree_summary_details(summary: &WorktreeSummaryRecord) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("scope {}", summary.scope),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("status {}", summary.status),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("root {}", summary.root.display()),
            continuation: false,
        },
    ];
    if let Some(label) = summary
        .label
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(TranscriptShellDetail::Raw {
            text: format!("label {}", label),
            continuation: false,
        });
    }
    if let Some(task_id) = summary.task_id.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("task {}", task_id),
            continuation: false,
        });
    }
    details
}

fn browser_status_label(status: agent::types::BrowserStatus) -> &'static str {
    match status {
        agent::types::BrowserStatus::Open => "Opened",
        agent::types::BrowserStatus::Closed => "Closed",
        agent::types::BrowserStatus::Failed => "Failed",
    }
}

fn apply_monitor_event(active: &mut super::state::ActiveMonitorCell, event: &MonitorEventKind) {
    match event {
        MonitorEventKind::Line { stream, text } => {
            push_monitor_output_line(&mut active.entry, *stream, text);
        }
        MonitorEventKind::StateChanged { status } => {
            active.entry.status = Some(match status {
                MonitorStatus::Running => super::state::TranscriptShellStatus::Running,
                MonitorStatus::Completed => super::state::TranscriptShellStatus::Completed,
                MonitorStatus::Failed => super::state::TranscriptShellStatus::Failed,
                MonitorStatus::Cancelled => super::state::TranscriptShellStatus::Cancelled,
            });
        }
        MonitorEventKind::Completed { exit_code } => {
            active.entry.status = Some(super::state::TranscriptShellStatus::Completed);
            active.entry.detail_lines.push(TranscriptShellDetail::Raw {
                text: format!("exit {}", exit_code),
                continuation: false,
            });
        }
        MonitorEventKind::Failed { exit_code, error } => {
            active.entry.status = Some(super::state::TranscriptShellStatus::Failed);
            if let Some(exit_code) = exit_code {
                active.entry.detail_lines.push(TranscriptShellDetail::Raw {
                    text: format!("exit {}", exit_code),
                    continuation: false,
                });
            }
            if let Some(error) = error.as_deref().filter(|value| !value.trim().is_empty()) {
                active.entry.detail_lines.push(TranscriptShellDetail::Raw {
                    text: format!("error {}", error),
                    continuation: false,
                });
            }
        }
        MonitorEventKind::Cancelled { reason } => {
            active.entry.status = Some(super::state::TranscriptShellStatus::Cancelled);
            if let Some(reason) = reason.as_deref().filter(|value| !value.trim().is_empty()) {
                active.entry.detail_lines.push(TranscriptShellDetail::Raw {
                    text: format!("reason {}", reason),
                    continuation: false,
                });
            }
        }
    }
}

fn push_monitor_output_line(entry: &mut TranscriptShellEntry, stream: MonitorStream, text: &str) {
    let label = match stream {
        MonitorStream::Stdout => "Stdout",
        MonitorStream::Stderr => "Stderr",
    };
    let kind = match stream {
        MonitorStream::Stdout => super::state::TranscriptShellBlockKind::Stdout,
        MonitorStream::Stderr => super::state::TranscriptShellBlockKind::Stderr,
    };
    if let Some(TranscriptShellDetail::NamedBlock { lines, .. }) = entry
        .detail_lines
        .iter_mut()
        .find(|detail| matches!(detail, TranscriptShellDetail::NamedBlock { label: existing, .. } if existing == label))
    {
        lines.push(text.to_string());
        let overflow = lines.len().saturating_sub(6);
        if overflow > 0 {
            lines.drain(0..overflow);
        }
        return;
    }
    entry.detail_lines.push(TranscriptShellDetail::NamedBlock {
        label: label.to_string(),
        kind,
        lines: vec![text.to_string()],
    });
}

fn task_created_entry(
    task: &AgentTaskSpec,
    status: TaskStatus,
    summary: Option<&str>,
) -> TranscriptEntry {
    TranscriptEntry::shell_summary_details(
        format!("Tracked Task {} ({status})", task.task_id),
        vec![
            TranscriptShellDetail::Raw {
                text: format!("role {}", task.role),
                continuation: false,
            },
            TranscriptShellDetail::Raw {
                text: format!("origin {}", task.origin),
                continuation: false,
            },
            TranscriptShellDetail::Raw {
                text: format!(
                    "summary {}",
                    preview_text(summary.unwrap_or(&task.prompt), 72)
                ),
                continuation: false,
            },
        ],
    )
}

fn worktree_status_label(status: WorktreeStatus) -> &'static str {
    match status {
        WorktreeStatus::Active => "Updated Active",
        WorktreeStatus::Inactive => "Updated Inactive",
        WorktreeStatus::Removed => "Removed",
    }
}

fn task_updated_entry(
    task_id: &TaskId,
    status: TaskStatus,
    summary: Option<&str>,
) -> TranscriptEntry {
    TranscriptEntry::shell_summary_details(
        format!("Updated Task {task_id} ({status})"),
        vec![TranscriptShellDetail::Raw {
            text: summary
                .map(|value| format!("summary {}", preview_text(value, 72)))
                .unwrap_or_else(|| "summary unchanged".to_string()),
            continuation: false,
        }],
    )
}

fn task_completed_entry(
    task_id: &TaskId,
    agent_id: &agent::types::AgentId,
    status: TaskStatus,
) -> TranscriptEntry {
    TranscriptEntry::shell_summary_details(
        format!("Completed Task {task_id} ({status})"),
        vec![TranscriptShellDetail::Raw {
            text: format!("agent {}", preview_text(agent_id.as_str(), 40)),
            continuation: false,
        }],
    )
}

fn subagent_started_entry(handle: &AgentHandle, task: &AgentTaskSpec) -> TranscriptEntry {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("task {}", task.task_id),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("prompt {}", preview_text(&task.prompt, 72)),
            continuation: false,
        },
    ];
    if let Some(worktree_id) = handle.worktree_id.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("worktree {}", worktree_id),
            continuation: false,
        });
    }
    if let Some(worktree_root) = handle.worktree_root.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("root {}", worktree_root.display()),
            continuation: false,
        });
    }
    TranscriptEntry::shell_summary_details(
        format!(
            "Started {} Agent {}",
            handle.role,
            preview_text(handle.agent_id.as_str(), 24)
        ),
        details,
    )
}

fn subagent_stopped_entry(
    handle: &AgentHandle,
    result: Option<&AgentResultEnvelope>,
    error: Option<&str>,
) -> TranscriptEntry {
    let headline = if error.is_some() {
        format!(
            "Stopped Agent {} (failed)",
            preview_text(handle.agent_id.as_str(), 24)
        )
    } else {
        format!(
            "Stopped Agent {}",
            preview_text(handle.agent_id.as_str(), 24)
        )
    };
    let mut details = Vec::new();
    if let Some(result) = result {
        details.push(TranscriptShellDetail::Raw {
            text: format!("summary {}", preview_text(&result.summary, 72)),
            continuation: false,
        });
    }
    if let Some(error) = error {
        details.push(TranscriptShellDetail::Raw {
            text: format!("error {}", preview_text(error, 72)),
            continuation: false,
        });
    }
    TranscriptEntry::shell_summary_details(headline, details)
}

#[cfg(test)]
mod tests {
    use super::SharedRenderObserver;
    use crate::frontend::tui::state::{
        ProviderRetryState, SharedUiState, ToolSelectionTarget, TranscriptEntry,
    };
    use crate::ui::{SessionEvent, SessionNotificationSource, SessionToolCall, SessionToolOrigin};
    use agent::types::{
        AgentSessionId, AgentTaskSpec, ContextWindowUsage, MonitorEventKind, MonitorEventRecord,
        MonitorId, MonitorStatus, MonitorStream, MonitorSummaryRecord, SessionId, TaskId,
        TaskOrigin, TaskStatus, TokenLedgerSnapshot, TokenUsage, TokenUsagePhase,
    };
    use serde_json::json;

    #[test]
    fn token_usage_updates_are_persisted_into_session_state() {
        let ui_state = SharedUiState::new();
        let ledger = TokenLedgerSnapshot {
            context_window: Some(ContextWindowUsage {
                used_tokens: 64_000,
                max_tokens: 400_000,
            }),
            last_usage: Some(TokenUsage {
                reasoning_tokens: 120,
                ..TokenUsage::from_input_output(4_000, 300, 500)
            }),
            cumulative_usage: TokenUsage {
                reasoning_tokens: 480,
                ..TokenUsage::from_input_output(20_000, 1_200, 3_000)
            },
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
                    "context 64000 / 400000 tokens, input 20000 output 1200 prefill 17000 decode 1200 cache 3000 reasoning 480"
                )
        );
    }

    #[test]
    fn notifications_surface_transcript_activity_and_toast() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::Notification {
            source: SessionNotificationSource::LoopDetector,
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
    fn provider_retry_status_is_live_only_until_the_next_request_attempt() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ProviderRetryScheduled {
            iteration: 1,
            status_code: 429,
            retry_count: 1,
            max_retries: 5,
            remaining_retries: 4,
            next_retry_at_ms: 5_000,
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.status, "Working");
        assert_eq!(
            snapshot.provider_retry,
            Some(ProviderRetryState {
                iteration: 1,
                status_code: 429,
                retry_count: 1,
                max_retries: 5,
                remaining_retries: 4,
                next_retry_at_ms: 5_000,
            })
        );

        observer.apply_event(SessionEvent::ModelRequestStarted { iteration: 1 });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.status, "Working");
        assert!(snapshot.provider_retry.is_none());
    }

    #[test]
    fn tool_lifecycle_events_are_projected_into_transcript_timeline() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: SessionToolOrigin::Provider {
                provider: "shell".to_string(),
            },
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
        assert_eq!(snapshot.active_tool_cells.len(), 1);
        assert_eq!(
            transcript_text(&TranscriptEntry::Tool(
                snapshot.active_tool_cells[0].entry.clone()
            )),
            "• Explored\n  └ List ."
        );

        observer.apply_event(SessionEvent::TurnCompleted {
            assistant_text: String::new(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Explored\n  └ List ."
        );
    }

    #[test]
    fn tool_request_and_completion_hold_exploration_until_turn_boundary() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: SessionToolOrigin::Provider {
                provider: "shell".to_string(),
            },
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
        assert!(snapshot.transcript.is_empty());
        assert_eq!(snapshot.active_tool_cells.len(), 1);
        assert_eq!(
            transcript_text(&TranscriptEntry::Tool(
                snapshot.active_tool_cells[0].entry.clone()
            )),
            "• Explored\n  └ List ."
        );

        observer.apply_event(SessionEvent::TurnCompleted {
            assistant_text: String::new(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Explored\n  └ List ."
        );
    }

    #[test]
    fn finished_exploration_calls_merge_into_one_transcript_cell() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        for (call_id, command) in [
            ("call_search", "$ rg shimmer_spans"),
            ("call_read_1", "$ cat shimmer.rs"),
            ("call_read_2", "$ cat status_indicator_widget.rs"),
        ] {
            observer.apply_event(SessionEvent::ToolLifecycleCompleted {
                call: SessionToolCall {
                    tool_name: "exec_command".to_string(),
                    call_id: call_id.to_string(),
                    origin: SessionToolOrigin::Provider {
                        provider: "shell".to_string(),
                    },
                    arguments_preview: vec![command.to_string()],
                },
                output_preview: String::new(),
                structured_output_preview: Some(
                    json!({
                        "kind": "run",
                        "exit_code": 0,
                        "timed_out": false,
                        "stdout": {"text": "", "chars": 0, "truncated": false},
                        "stderr": {"text": "", "chars": 0, "truncated": false}
                    })
                    .to_string(),
                ),
            });
        }

        let snapshot = ui_state.snapshot();
        assert!(snapshot.transcript.is_empty());
        assert_eq!(snapshot.active_tool_cells.len(), 1);
        assert_eq!(
            transcript_text(&TranscriptEntry::Tool(
                snapshot.active_tool_cells[0].entry.clone()
            )),
            "• Explored\n  └ Search shimmer_spans\n    Read shimmer.rs, status_indicator_widget.rs"
        );

        observer.apply_event(SessionEvent::TurnCompleted {
            assistant_text: String::new(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Explored\n  └ Search shimmer_spans\n    Read shimmer.rs, status_indicator_widget.rs"
        );
    }

    #[test]
    fn completed_exploration_keeps_selection_on_live_cell_until_turn_end() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: SessionToolOrigin::Provider {
                provider: "shell".to_string(),
            },
            arguments_preview: vec!["$ ls".to_string()],
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleStarted { call: call.clone() });
        ui_state.mutate(|state| {
            state.tool_selection = Some(ToolSelectionTarget::LiveCell("call_123".to_string()));
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
            Some(ToolSelectionTarget::LiveCell("call_123".to_string()))
        );

        observer.apply_event(SessionEvent::TurnCompleted {
            assistant_text: String::new(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(
            snapshot.tool_selection,
            Some(ToolSelectionTarget::Transcript(0))
        );
    }

    #[test]
    fn merged_exploration_completion_keeps_selection_on_aggregate_live_cell() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call: SessionToolCall {
                tool_name: "exec_command".to_string(),
                call_id: "call_search".to_string(),
                origin: SessionToolOrigin::Provider {
                    provider: "shell".to_string(),
                },
                arguments_preview: vec!["$ rg shimmer_spans".to_string()],
            },
            output_preview: String::new(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 0,
                    "timed_out": false,
                    "stdout": {"text": "", "chars": 0, "truncated": false},
                    "stderr": {"text": "", "chars": 0, "truncated": false}
                })
                .to_string(),
            ),
        });

        let read_call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_read".to_string(),
            origin: SessionToolOrigin::Provider {
                provider: "shell".to_string(),
            },
            arguments_preview: vec!["$ cat shimmer.rs".to_string()],
        };
        observer.apply_event(SessionEvent::ToolCallRequested {
            call: read_call.clone(),
        });
        ui_state.mutate(|state| {
            state.tool_selection = Some(ToolSelectionTarget::LiveCell("call_read".to_string()));
        });
        observer.apply_event(SessionEvent::ToolLifecycleStarted {
            call: read_call.clone(),
        });
        observer.apply_event(SessionEvent::ToolLifecycleCompleted {
            call: read_call,
            output_preview: String::new(),
            structured_output_preview: Some(
                json!({
                    "kind": "run",
                    "exit_code": 0,
                    "timed_out": false,
                    "stdout": {"text": "", "chars": 0, "truncated": false},
                    "stderr": {"text": "", "chars": 0, "truncated": false}
                })
                .to_string(),
            ),
        });

        let snapshot = ui_state.snapshot();
        assert!(snapshot.transcript.is_empty());
        assert_eq!(snapshot.active_tool_cells.len(), 1);
        assert_eq!(
            snapshot.tool_selection,
            Some(ToolSelectionTarget::LiveCell("call_search".to_string()))
        );

        observer.apply_event(SessionEvent::TurnCompleted {
            assistant_text: String::new(),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            snapshot.tool_selection,
            Some(ToolSelectionTarget::Transcript(0))
        );
    }

    #[test]
    fn task_events_update_tracked_task_snapshot() {
        let ui_state = SharedUiState::new();
        let task = AgentTaskSpec {
            task_id: TaskId::from("task_123"),
            role: "reviewer".to_string(),
            prompt: "Inspect repo".to_string(),
            origin: TaskOrigin::AgentCreated,
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let mut observer = SharedRenderObserver::new(ui_state.clone());

        observer.apply_event(SessionEvent::TaskCreated {
            task: task.clone(),
            parent_agent_id: None,
            status: TaskStatus::Open,
            summary: Some("Inspect repo".to_string()),
            worktree_id: None,
            worktree_root: None,
        });
        observer.apply_event(SessionEvent::TaskUpdated {
            task_id: task.task_id.clone(),
            status: TaskStatus::Running,
            summary: Some("Refine TUI".to_string()),
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 2);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Tracked Task task_123 (open)\n  └ role reviewer\n  └ origin agent_created\n  └ summary Inspect repo"
        );
        assert_eq!(
            transcript_text(&snapshot.transcript[1]),
            "• Updated Task task_123 (running)\n  └ summary Refine TUI"
        );
        assert_eq!(snapshot.tracked_tasks.len(), 1);
        assert_eq!(snapshot.tracked_tasks[0].task_id, TaskId::from("task_123"));
        assert_eq!(snapshot.tracked_tasks[0].status, TaskStatus::Running);
        assert_eq!(
            snapshot.tracked_tasks[0].summary.as_deref(),
            Some("Refine TUI")
        );
    }

    #[test]
    fn task_completion_events_update_snapshot() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());
        let task_id = TaskId::from("task_exec");

        observer.apply_event(SessionEvent::TaskCompleted {
            task_id: task_id.clone(),
            agent_id: "agent_123".into(),
            status: TaskStatus::Completed,
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Completed Task task_exec (completed)\n  └ agent agent_123"
        );
        assert_eq!(snapshot.tracked_tasks.len(), 1);
        assert_eq!(snapshot.tracked_tasks[0].task_id, task_id);
        assert_eq!(snapshot.tracked_tasks[0].status, TaskStatus::Completed);
        assert_eq!(
            snapshot.tracked_tasks[0].child_agent_id.as_deref(),
            Some("agent_123")
        );
    }

    #[test]
    fn approval_and_tool_run_share_one_timeline_cell() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_123".to_string(),
            origin: SessionToolOrigin::Local,
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
            "• Ran cargo test\n  └ $ cargo test\n  └ Result exit 0\n  └ Stdout\n    ok"
        );
    }

    #[test]
    fn exec_command_failures_prefer_tail_output_preview() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "exec_command".to_string(),
            call_id: "call_999".to_string(),
            origin: SessionToolOrigin::Local,
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
        let transcript_lines = transcript.lines().collect::<Vec<_>>();
        assert!(transcript.contains("  └ Result exit 1"));
        assert!(transcript.contains("  └ Stderr"));
        assert!(
            transcript_lines
                .windows(2)
                .any(|window| window == ["  └ Stderr", "    1"])
        );
        assert!(transcript.contains("    20"));
        assert!(!transcript.contains("… +"));
    }

    #[test]
    fn file_tools_surface_diff_blocks_in_live_transcript() {
        let ui_state = SharedUiState::new();
        let call = SessionToolCall {
            tool_name: "write".to_string(),
            call_id: "call_456".to_string(),
            origin: SessionToolOrigin::Local,
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
        assert!(transcript.contains("  └ Files src/lib.rs"));
        assert!(transcript.contains("action [r] review diff"));
        assert!(
            snapshot.transcript[0]
                .tool_entry()
                .and_then(|entry| entry.review.as_ref())
                .is_some()
        );
    }

    #[test]
    fn monitor_events_stream_into_live_tail_and_finish_as_terminal_summary() {
        let ui_state = SharedUiState::new();
        let mut observer = SharedRenderObserver::new(ui_state.clone());
        let summary = MonitorSummaryRecord {
            monitor_id: MonitorId::from("mon_123"),
            session_id: SessionId::from("session_1"),
            agent_session_id: AgentSessionId::from("agent_session_1"),
            parent_agent_id: None,
            task_id: None,
            command: "npm run dev".to_string(),
            cwd: "/workspace/web".to_string(),
            shell: "/bin/zsh".to_string(),
            login: true,
            status: MonitorStatus::Running,
            started_at_unix_s: 1_700_000_000,
            finished_at_unix_s: None,
        };

        observer.apply_event(SessionEvent::MonitorStarted {
            summary: summary.clone(),
        });
        observer.apply_event(SessionEvent::MonitorEvent {
            event: MonitorEventRecord {
                monitor_id: summary.monitor_id.clone(),
                timestamp_unix_s: 1_700_000_001,
                kind: MonitorEventKind::Line {
                    stream: MonitorStream::Stdout,
                    text: "ready on http://localhost:3000".to_string(),
                },
            },
        });

        let snapshot = ui_state.snapshot();
        assert_eq!(snapshot.active_monitors.len(), 1);
        assert_eq!(
            transcript_text(&TranscriptEntry::ShellSummary(
                snapshot.active_monitors[0].entry.clone()
            )),
            "• Running monitor mon_123\n  └ cwd /workspace/web\n  └ command npm run dev\n  └ Stdout\n    ready on http://localhost:3000"
        );

        let mut completed_summary = summary.clone();
        completed_summary.status = MonitorStatus::Completed;
        completed_summary.finished_at_unix_s = Some(1_700_000_010);
        observer.apply_event(SessionEvent::MonitorUpdated {
            summary: completed_summary,
        });

        let snapshot = ui_state.snapshot();
        assert!(snapshot.active_monitors.is_empty());
        assert_eq!(
            transcript_text(&snapshot.transcript[0]),
            "• Completed monitor mon_123\n  └ cwd /workspace/web\n  └ command npm run dev\n  └ Stdout\n    ready on http://localhost:3000"
        );
    }

    fn transcript_text(entry: &crate::frontend::tui::state::TranscriptEntry) -> String {
        entry.serialized()
    }
}
