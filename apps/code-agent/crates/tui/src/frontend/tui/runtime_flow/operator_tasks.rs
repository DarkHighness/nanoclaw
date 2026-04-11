use super::*;

impl CodeAgentTui {
    pub(crate) async fn maybe_finish_turn(&mut self) -> Result<()> {
        let finished = self
            .turn_task
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if !finished {
            return Ok(());
        }
        let workspace_root = self.workspace_root_buf();
        let git = state::git_snapshot(&workspace_root, self.host_process_surfaces_allowed());
        if let Some(task) = self.turn_task.take() {
            match task.await {
                Ok(Ok(())) => {
                    let stored_session_count = self.refresh_stored_session_count().await.ok();
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.turn_started_at = None;
                        state.active_tool_label = None;
                        state.session.git = git.clone();
                        if let Some(stored_session_count) = stored_session_count {
                            state.session.stored_session_count = stored_session_count;
                        }
                    });
                }
                Ok(Err(error)) => {
                    let message = summarize_nonfatal_error("turn task", &error);
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.turn_started_at = None;
                        state.active_tool_label = None;
                        state.session.git = git.clone();
                        state.status = format!("Error: {message}");
                        state.push_transcript(state::TranscriptEntry::error_summary_details(
                            message.clone(),
                            Vec::new(),
                        ));
                        state.push_activity(format!(
                            "turn failed: {}",
                            state::preview_text(&message, 56)
                        ));
                    });
                }
                Err(error) => {
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.turn_started_at = None;
                        state.active_tool_label = None;
                        state.session.git = git.clone();
                        state.status = format!("Task join error: {error}");
                        state.push_activity(format!("task join error: {error}"));
                    });
                }
            }
        }
        self.sync_runtime_control_state();
        if self.turn_task.is_none() && self.queued_command_count() > 0 {
            self.start_runtime_queue_drain();
        }
        Ok(())
    }

    pub(crate) async fn maybe_finish_operator_task(&mut self) -> Result<()> {
        let finished = self
            .operator_task
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if !finished {
            return Ok(());
        }
        if let Some(task) = self.operator_task.take() {
            match task.await {
                Ok(Ok(OperatorTaskOutcome::WaitLiveTask(outcome))) => {
                    let inspector = format_live_task_wait_outcome(&outcome);
                    let turn_running = self.ui_state.snapshot().turn_running;
                    let toast_tone = live_task_wait_ui_toast_tone(&outcome);
                    let toast_message = live_task_wait_toast_message(&outcome, turn_running);
                    let attention = self.schedule_live_task_attention(&outcome, turn_running)?;
                    let notice_outcome = outcome.clone();
                    let live_task_id = outcome.task_id.clone();
                    let live_task_status = outcome.status.clone();
                    self.ui_state.mutate(move |state| {
                        // Background child completion is easy to miss if it only
                        // flashes through the status line, so persist it into
                        // the transcript and keep the operator-facing hint visible
                        // even when the model follow-up is handled automatically.
                        state.push_transcript(live_task_wait_notice_entry(&notice_outcome));
                        state.set_live_task_finished_hint(
                            live_task_id.clone(),
                            live_task_status.clone(),
                        );
                        state.show_main_view("Live Task Wait", inspector);
                        state.status = format!(
                            "Live task {} finished with status {}",
                            live_task_id, live_task_status
                        );
                        state.push_activity(format!(
                            "wait completed for {} ({})",
                            live_task_id, live_task_status
                        ));
                    });
                    match toast_tone {
                        ToastTone::Info => self
                            .event_renderer
                            .apply_event(SessionEvent::tui_info_toast(toast_message)),
                        ToastTone::Success => self
                            .event_renderer
                            .apply_event(SessionEvent::tui_success_toast(toast_message)),
                        ToastTone::Warning => self
                            .event_renderer
                            .apply_event(SessionEvent::tui_warning_toast(toast_message)),
                        ToastTone::Error => self
                            .event_renderer
                            .apply_event(SessionEvent::tui_error_toast(toast_message)),
                    }
                    self.ui_state.mutate(|state| {
                        state.push_activity(match attention.action {
                            LiveTaskAttentionAction::ScheduledSteer => {
                                format!("scheduled live-task steer for {}", outcome.task_id)
                            }
                            LiveTaskAttentionAction::QueuedPrompt => {
                                format!("queued live-task prompt for {}", outcome.task_id)
                            }
                        });
                    });
                    self.sync_runtime_control_state();
                    if !turn_running && self.turn_task.is_none() && self.queued_command_count() > 0
                    {
                        self.start_runtime_queue_drain();
                    }
                }
                Ok(Ok(OperatorTaskOutcome::SideQuestion(outcome))) => {
                    let inspector = format_side_question_inspector(&outcome);
                    let toast_message = format!(
                        "/btw answered: {}",
                        state::preview_text(&outcome.question, 48)
                    );
                    self.ui_state.mutate(move |state| {
                        state.show_main_view("BTW", inspector);
                        state.status = format!(
                            "Answered /btw {}",
                            state::preview_text(&outcome.question, 48)
                        );
                        state.push_activity(format!(
                            "answered /btw {}",
                            state::preview_text(&outcome.question, 48)
                        ));
                    });
                    self.event_renderer
                        .apply_event(SessionEvent::tui_info_toast(toast_message));
                }
                Ok(Err(error)) => {
                    let message = summarize_nonfatal_error("operator task", &error);
                    let toast_message = format!(
                        "operator task failed: {}",
                        state::preview_text(&message, 80)
                    );
                    self.ui_state.mutate(|state| {
                        state.status = format!("Operator task failed: {message}");
                        state.show_main_view(
                            "Operator Error",
                            vec![
                                InspectorEntry::section("Operator Error"),
                                InspectorEntry::Plain(message.clone()),
                            ],
                        );
                        state.push_activity(format!(
                            "operator task failed: {}",
                            state::preview_text(&message, 56)
                        ));
                    });
                    self.event_renderer
                        .apply_event(SessionEvent::tui_error_toast(toast_message));
                }
                Err(error) => {
                    let toast_message = format!("operator task join error: {error}");
                    self.ui_state.mutate(|state| {
                        state.status = format!("Operator task join error: {error}");
                        state.push_activity(format!("operator task join error: {error}"));
                    });
                    self.event_renderer
                        .apply_event(SessionEvent::tui_error_toast(toast_message));
                }
            }
        }
        Ok(())
    }
}
