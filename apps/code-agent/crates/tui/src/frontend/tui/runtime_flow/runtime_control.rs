use super::*;

impl CodeAgentTui {
    pub(crate) async fn start_turn(&mut self, prompt: String) {
        self.start_turn_message(
            Message::user(prompt.clone()),
            Some(SubmittedPromptSnapshot::from_text(prompt)),
        )
        .await;
    }

    pub(crate) async fn start_turn_message(
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

    pub(crate) async fn queue_prompt_behind_active_turn_message(
        &mut self,
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    ) {
        let preview = submitted_prompt
            .as_ref()
            .map(|prompt| state::preview_text(&prompt.text, 40))
            .unwrap_or_else(|| state::preview_text(&message_operator_text(&message), 40));
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

    pub(crate) async fn schedule_runtime_steer_while_active(
        &mut self,
        message: String,
        reason: Option<crate::interaction::PendingControlReason>,
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

    pub(crate) async fn start_command(&mut self, command: RuntimeCommand) {
        let preview = queued_command_preview(&command);
        self.ui_state.mutate(|state| {
            state.show_transcript_pane();
            state.follow_transcript = true;
            state.transcript_scroll = u16::MAX;
            state.turn_running = true;
            state.turn_started_at = Some(Instant::now());
            state.clear_missing_live_tool_selection();
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

    pub(crate) fn start_runtime_queue_drain(&mut self) {
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
            state.clear_missing_live_tool_selection();
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

    pub(crate) async fn interrupt_active_turn(&mut self) -> Result<()> {
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
                state.clear_missing_live_tool_selection();
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
                state.clear_missing_live_tool_selection();
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
}
