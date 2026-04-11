use super::*;

impl CodeAgentTui {
    pub(super) async fn maybe_finish_turn(&mut self) -> Result<()> {
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

    pub(super) async fn maybe_finish_operator_task(&mut self) -> Result<()> {
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

    pub(super) async fn apply_plain_input_submit(
        &mut self,
        action: PlainInputSubmitAction,
        submission: ComposerSubmission,
    ) {
        self.record_submitted_prompt(&submission);
        let message = match self
            .materialize_prompt_message(&submission.local_history_draft)
            .await
        {
            Ok(message) => message,
            Err(error) => {
                let message = summarize_nonfatal_error("materialize prompt", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to prepare prompt: {message}");
                    state.push_activity(format!(
                        "failed to prepare prompt: {}",
                        state::preview_text(&message, 56)
                    ));
                });
                return;
            }
        };
        match action {
            PlainInputSubmitAction::StartPrompt => {
                self.start_turn_message(message, Some(submission.prompt_snapshot))
                    .await
            }
            PlainInputSubmitAction::QueuePrompt => {
                self.queue_prompt_behind_active_turn_message(
                    message,
                    Some(submission.prompt_snapshot),
                )
                .await;
            }
            PlainInputSubmitAction::SteerActiveTurn => {
                self.schedule_runtime_steer_while_active(
                    submission.prompt_snapshot.text,
                    Some("inline_enter".to_string()),
                )
                .await;
            }
        }
    }

    pub(super) async fn materialize_prompt_message(
        &self,
        draft: &ComposerDraftState,
    ) -> Result<Message> {
        let mut parts = Vec::new();
        for attachment in draft
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.placeholder.is_none())
        {
            parts.extend(self.materialize_attachment_parts(attachment).await?);
        }

        let mut remaining = draft.text.as_str();
        while !remaining.is_empty() {
            let next_attachment = draft
                .draft_attachments
                .iter()
                .filter_map(|attachment| {
                    let placeholder = attachment.placeholder.as_ref()?;
                    remaining.find(placeholder).map(|index| (index, attachment))
                })
                .min_by_key(|(index, _)| *index);
            let Some((index, attachment)) = next_attachment else {
                parts.push(MessagePart::inline_text(remaining));
                break;
            };

            if index > 0 {
                parts.push(MessagePart::inline_text(&remaining[..index]));
            }
            parts.extend(self.materialize_attachment_parts(attachment).await?);
            remaining = &remaining[index
                + attachment
                    .placeholder
                    .as_ref()
                    .expect("inline attachment placeholder")
                    .len()..];
        }

        Ok(if parts.is_empty() {
            Message::user("")
        } else {
            Message::new(agent::types::MessageRole::User, parts)
        })
    }

    pub(super) async fn materialize_attachment_parts(
        &self,
        attachment: &ComposerDraftAttachmentState,
    ) -> Result<Vec<MessagePart>> {
        Ok(match &attachment.kind {
            ComposerDraftAttachmentKind::LargePaste { payload } => {
                let label = attachment
                    .placeholder
                    .clone()
                    .unwrap_or_else(|| "[Paste]".to_string());
                vec![MessagePart::paste(label, payload.clone())]
            }
            ComposerDraftAttachmentKind::LocalImage {
                part: Some(part), ..
            }
            | ComposerDraftAttachmentKind::LocalFile {
                part: Some(part), ..
            }
            | ComposerDraftAttachmentKind::RemoteImage { part, .. }
            | ComposerDraftAttachmentKind::RemoteFile { part, .. } => vec![part.clone()],
            ComposerDraftAttachmentKind::LocalImage {
                requested_path,
                part: None,
                ..
            } => vec![
                load_tool_image(requested_path, &self.composer_attachment_context())
                    .await?
                    .message_part(),
            ],
            ComposerDraftAttachmentKind::LocalFile {
                requested_path,
                file_name,
                mime_type,
                part: None,
            } => {
                let file =
                    load_composer_file(requested_path, &self.composer_attachment_context()).await?;
                vec![MessagePart::File {
                    file_name: file_name.clone().or(file.file_name),
                    mime_type: mime_type.clone().or(file.mime_type),
                    data_base64: Some(file.data_base64),
                    uri: Some(file.requested_path),
                }]
            }
        })
    }

    pub(super) async fn try_apply_pending_control_edit(&mut self, input: &str) -> bool {
        let Some(editing) = self.ui_state.snapshot().editing_pending_control.clone() else {
            return false;
        };
        let content = input.trim();
        if content.is_empty() {
            self.ui_state.mutate(|state| {
                state.status = "Pending control edits cannot be empty".to_string();
                state.push_activity("rejected empty pending control edit");
            });
            return true;
        }
        match self.update_pending_control(&editing.id, content) {
            Ok(updated) => {
                self.sync_runtime_control_state();
                self.ui_state.mutate(|state| {
                    state.clear_pending_control_edit();
                    state.clear_input();
                    state.status = format!(
                        "Updated queued {} {}",
                        pending_control_kind_label(updated.kind),
                        preview_id(&updated.id)
                    );
                    state.push_activity(format!(
                        "updated queued {} {}",
                        pending_control_kind_label(updated.kind),
                        preview_id(&updated.id)
                    ));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("update pending control", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to update pending control: {message}");
                    state.push_activity(format!(
                        "failed to update pending control: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
        true
    }

    pub(super) async fn start_turn(&mut self, prompt: String) {
        self.start_turn_message(
            Message::user(prompt.clone()),
            Some(SubmittedPromptSnapshot::from_text(prompt)),
        )
        .await;
    }

    pub(super) async fn start_turn_message(
        &mut self,
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    ) {
        if self.turn_task.is_some() {
            self.queue_prompt_behind_active_turn_message(message, submitted_prompt)
                .await;
            return;
        }

        self.start_command(RuntimeCommand::Prompt {
            message,
            submitted_prompt,
        })
        .await;
    }

    pub(super) async fn queue_prompt_behind_active_turn_message(
        &mut self,
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    ) {
        let preview = state::preview_text(&message_operator_text(&message), 40);
        match self
            .run_ui::<String>(UIAsyncCommand::QueuePromptCommand {
                message,
                submitted_prompt,
            })
            .await
        {
            Ok(queued_id) => {
                let pending = self.pending_controls();
                let depth = pending.len();
                self.ui_state.mutate(|state| {
                    state.session.queued_commands = depth;
                    state.sync_pending_controls(pending);
                    state.status = "Queued prompt behind the active turn".to_string();
                    state.push_activity(format!("queued prompt {}: {}", queued_id, preview));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("queue prompt", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to queue prompt: {message}");
                    state.push_activity(format!(
                        "failed to queue prompt: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    pub(super) async fn schedule_runtime_steer_while_active(
        &mut self,
        message: String,
        reason: Option<String>,
    ) {
        let preview = state::preview_text(&message, 40);
        match self.schedule_runtime_steer(message, reason) {
            Ok(queued_id) => {
                let pending = self.pending_controls();
                self.ui_state.mutate(|state| {
                    state.session.queued_commands = pending.len();
                    state.sync_pending_controls(pending);
                    state.status = "Scheduled steer for the active turn".to_string();
                    state.push_activity(format!(
                        "scheduled active-turn steer {}: {preview}",
                        queued_id
                    ));
                })
            }
            Err(error) => {
                let message = summarize_nonfatal_error("schedule runtime steer", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to schedule steer: {message}");
                    state.push_activity(format!(
                        "failed to schedule active-turn steer: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    pub(super) async fn start_command(&mut self, command: RuntimeCommand) {
        let preview = queued_command_preview(&command);
        self.ui_state.mutate(|state| {
            state.show_transcript_pane();
            state.follow_transcript = true;
            state.transcript_scroll = u16::MAX;
            state.turn_running = true;
            state.turn_started_at = Some(Instant::now());
            state.active_tool_label = None;
            state.status = "Working".to_string();
            state.push_activity(preview.clone());
        });

        let session = self.session.clone();
        self.turn_task = Some(spawn_local(async move {
            session
                .run::<()>(UIAsyncCommand::ApplyControl { command })
                .await
        }));
    }

    pub(super) fn start_runtime_queue_drain(&mut self) {
        // The host only restarts draining once the active task goes idle. The
        // runtime still owns dequeue order and queue depth, so the TUI reads
        // the current depth instead of speculating about the next popped item.
        let queued = self.queued_command_count();
        self.ui_state.mutate(|state| {
            state.show_transcript_pane();
            state.follow_transcript = true;
            state.transcript_scroll = u16::MAX;
            state.turn_running = true;
            state.turn_started_at = Some(Instant::now());
            state.active_tool_label = None;
            state.session.queued_commands = queued;
            state.status = "Working".to_string();
        });

        let session = self.session.clone();
        self.turn_task = Some(spawn_local(async move {
            session
                .run::<bool>(UIAsyncCommand::DrainQueuedControls)
                .await
                .map(|_| ())
        }));
    }

    pub(super) async fn interrupt_active_turn(&mut self) -> Result<()> {
        if !self.abort_turn_task() {
            return Ok(());
        }

        // Once the live task is aborted, any safe-point steer would never be
        // merged in-band. Resubmit all pending steers as one fresh prompt in
        // FIFO order so their intent matches the sequence the operator entered.
        let pending_steers = self.take_pending_steers()?;
        self.sync_runtime_control_state();

        let steers = pending_steers
            .into_iter()
            .map(|steer| steer.preview)
            .collect::<Vec<_>>();
        let steer_count = steers.len();

        if let Some(prompt) = merge_interrupt_steers(steers) {
            let preview = state::preview_text(&prompt, 40);
            self.ui_state.mutate(|state| {
                state.turn_running = false;
                state.turn_started_at = None;
                state.active_tool_label = None;
                state.push_transcript(state::TranscriptEntry::error_summary_details(
                    "Interrupted current turn",
                    Vec::new(),
                ));
                state.push_activity(format!(
                    "interrupted current turn and resubmitted {steer_count} steer(s): {preview}"
                ));
            });
            self.start_command(RuntimeCommand::Prompt {
                message: Message::user(prompt),
                submitted_prompt: None,
            })
            .await;
        } else {
            self.ui_state.mutate(|state| {
                state.turn_running = false;
                state.turn_started_at = None;
                state.active_tool_label = None;
                state.status =
                    "Interrupted current turn. What should nanoclaw do next?".to_string();
                state.push_transcript(state::TranscriptEntry::error_summary_details(
                    "Interrupted current turn",
                    Vec::new(),
                ));
                state.push_activity("interrupted current turn");
            });
        }

        Ok(())
    }

    pub(super) fn cycle_model_reasoning_effort(&mut self) {
        match self.cycle_model_reasoning_effort_result() {
            Ok(outcome) => self.apply_model_reasoning_effort_outcome(outcome, "cycled"),
            Err(error) => self.record_model_reasoning_effort_error(summarize_nonfatal_error(
                "cycle model reasoning effort",
                &error,
            )),
        }
    }

    pub(super) fn set_model_reasoning_effort(&mut self, effort: &str) {
        match self.set_model_reasoning_effort_result(effort) {
            Ok(outcome) => self.apply_model_reasoning_effort_outcome(outcome, "set"),
            Err(error) => self.record_model_reasoning_effort_error(summarize_nonfatal_error(
                "set model reasoning effort",
                &error,
            )),
        }
    }

    pub(super) fn apply_model_reasoning_effort_outcome(
        &mut self,
        outcome: ModelReasoningEffortOutcome,
        verb: &str,
    ) {
        let current = outcome
            .current
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let previous = outcome
            .previous
            .clone()
            .unwrap_or_else(|| "default".to_string());
        self.ui_state.mutate(|state| {
            state.session.model_reasoning_effort = outcome.current;
            state.status =
                format!("Thinking effort {verb} to {current}; next model request will use it");
            state.push_activity(format!("thinking effort {previous} -> {current}"));
        });
    }

    pub(super) fn record_model_reasoning_effort_error(&mut self, message: String) {
        self.ui_state.mutate(|state| {
            state.status = format!("Thinking effort unavailable: {message}");
            state.push_activity(format!(
                "thinking effort rejected: {}",
                state::preview_text(&message, 56)
            ));
        });
    }

    pub(super) fn preview_selected_theme(&mut self) {
        let snapshot = self.ui_state.snapshot();
        if let Some(theme_id) = snapshot.selected_theme() {
            self.apply_tui_theme(&theme_id, false, None);
        }
    }

    pub(super) fn apply_tui_theme(
        &mut self,
        theme_id: &str,
        persist: bool,
        previous_override: Option<String>,
    ) {
        match set_active_theme(theme_id) {
            Ok(()) => {
                let current = theme_id.to_string();
                let mut previous = None;
                self.ui_state.mutate(|state| {
                    previous = Some(state.theme.clone());
                    state.theme = current.clone();
                    state.themes = crate::theme::theme_summaries();
                    if !persist {
                        state.status = format!("Previewing theme {current}");
                    }
                });
                if !persist {
                    return;
                }

                let workspace_root = self.workspace_root_buf();
                match persist_tui_theme_selection(&workspace_root, &current) {
                    Ok(()) => {
                        let previous = previous_override
                            .or(previous)
                            .unwrap_or_else(|| current.clone());
                        self.ui_state.mutate(|state| {
                            state.status = format!("Theme saved as {current}");
                            if previous == current {
                                state.push_activity(format!("theme persisted: {current}"));
                            } else {
                                state.push_activity(format!("theme {previous} -> {current}"));
                            }
                        });
                    }
                    Err(error) => {
                        let message = summarize_nonfatal_error("persist tui theme", &error);
                        self.ui_state.mutate(|state| {
                            state.status =
                                format!("Theme {current} active, but failed to save: {message}");
                            state.push_activity(format!(
                                "theme persistence failed: {}",
                                state::preview_text(&message, 56)
                            ));
                        });
                    }
                }
            }
            Err(error) => {
                let message = summarize_nonfatal_error("set tui theme", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Theme unavailable: {message}");
                    state.push_activity(format!(
                        "theme rejected: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }
}
