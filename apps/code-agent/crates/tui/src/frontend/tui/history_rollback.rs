use super::*;
use crate::ui::{CheckpointRestoreOutcome, HistoryRollbackOutcome};

impl CodeAgentTui {
    pub(super) async fn handle_history_rollback_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(false);
        }
        let snapshot = self.ui_state.snapshot();
        if snapshot.history_rollback.is_none() {
            return Ok(false);
        }
        if !snapshot.input.is_empty() {
            self.ui_state.mutate(|state| state.clear_history_rollback());
            return Ok(false);
        }

        if snapshot.history_rollback_is_primed() {
            if key.code == KeyCode::Esc {
                self.open_history_rollback_overlay().await?;
                return Ok(true);
            }
            self.ui_state.mutate(|state| state.clear_history_rollback());
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_history_rollback_selection(true);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Right | KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_history_rollback_selection(false);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    let _ = state.jump_history_rollback_selection(true);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    let _ = state.jump_history_rollback_selection(false);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Enter => {
                self.confirm_history_rollback().await?;
            }
            KeyCode::Tab => {
                self.ui_state.mutate(|state| {
                    let _ = state.cycle_history_rollback_restore_mode();
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Char('q') | KeyCode::Backspace | KeyCode::Delete => {
                self.ui_state.mutate(|state| {
                    state.clear_history_rollback();
                    state.status = "Closed history rollback".to_string();
                    state.push_activity("closed history rollback overlay");
                });
            }
            _ => {}
        }

        Ok(true)
    }

    pub(super) async fn prime_history_rollback(&mut self) -> Result<()> {
        if self
            .history_rollback_candidates()
            .await
            .into_iter()
            .next()
            .is_none()
        {
            self.ui_state.mutate(|state| {
                state.clear_history_rollback();
                state.status = "No visible user turns are available to roll back".to_string();
                state.push_activity("history rollback unavailable");
            });
            return Ok(());
        }

        self.ui_state.mutate(|state| {
            state.prime_history_rollback();
            state.status =
                "Transcript rollback armed. Press Esc again to choose a turn".to_string();
            state.push_activity("armed history rollback");
        });
        Ok(())
    }

    pub(super) async fn open_history_rollback_overlay(&mut self) -> Result<()> {
        let candidates = self.history_rollback_candidates().await;
        if candidates.is_empty() {
            self.ui_state.mutate(|state| {
                state.clear_history_rollback();
                state.status = "No visible user turns are available to roll back".to_string();
                state.push_activity("history rollback unavailable");
            });
            return Ok(());
        }

        self.ui_state.mutate(|state| {
            let opened = state.open_history_rollback_overlay(candidates);
            if opened {
                state.status =
                    "Transcript rollback overlay opened. Select a turn to rewind to".to_string();
                state.push_activity("opened history rollback overlay");
            }
        });
        self.refresh_history_rollback_selection_status();
        Ok(())
    }

    pub(super) async fn history_rollback_candidates(&self) -> Vec<state::HistoryRollbackCandidate> {
        let rounds: Vec<HistoryRollbackRound> = self
            .run_ui(UIAsyncCommand::HistoryRollbackRounds)
            .await
            .unwrap_or_default();
        build_history_rollback_candidates(&rounds)
    }

    pub(super) fn refresh_history_rollback_selection_status(&self) {
        let snapshot = self.ui_state.snapshot();
        let Some(overlay) = snapshot.history_rollback_overlay() else {
            return;
        };
        let Some(candidate) = overlay.candidates.get(overlay.selected) else {
            return;
        };
        let status = history_rollback_status(
            candidate,
            overlay.selected,
            overlay.candidates.len(),
            overlay.restore_mode,
        );
        self.ui_state.mutate(|state| {
            state.status = status;
        });
    }

    pub(super) async fn confirm_history_rollback(&mut self) -> Result<()> {
        let snapshot = self.ui_state.snapshot();
        let Some(overlay) = snapshot.history_rollback_overlay() else {
            return Ok(());
        };
        let Some(candidate) = overlay.candidates.get(overlay.selected).cloned() else {
            return Ok(());
        };
        let total = overlay.candidates.len();
        let selected = overlay.selected;
        let restore_mode = overlay.restore_mode;

        let outcome = match restore_mode {
            CheckpointRestoreMode::Both => {
                let Some(checkpoint) = candidate.checkpoint.as_ref() else {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Selected turn has no checkpoint; only transcript rewind is available"
                                .to_string();
                    });
                    return Ok(());
                };
                self.run_ui::<CheckpointRestoreOutcome>(UIAsyncCommand::RestoreCheckpoint {
                    checkpoint_id: checkpoint.checkpoint_id.to_string(),
                    restore_mode,
                })
                .await
                .map(|result| {
                    let transcript = result.transcript.clone();
                    let removed_message_count = result.removed_message_count;
                    (transcript, removed_message_count, Some(result))
                })
            }
            _ => self
                .run_ui::<HistoryRollbackOutcome>(UIAsyncCommand::RollbackVisibleHistoryToMessage {
                    message_id: candidate.message_id.to_string(),
                })
                .await
                .map(|result| (result.transcript, result.removed_message_count, None)),
        };

        match outcome {
            Ok(outcome) => {
                let transcript = format_visible_transcript_lines(&outcome.0);
                let preview = state::preview_text(&candidate.prompt, 48);
                self.ui_state.mutate(move |state| {
                    state.clear_history_rollback();
                    state.show_transcript_pane();
                    state.transcript = transcript;
                    state.follow_transcript = true;
                    state.transcript_scroll = u16::MAX;
                    state.restore_input_draft(candidate.draft.clone());
                    state.status = match outcome.2.as_ref() {
                        Some(restore) => {
                            if candidate.draft.text.trim().is_empty()
                                && candidate.draft.draft_attachments.is_empty()
                            {
                                format!(
                                    "Restored {} file(s) and rolled back {} message(s). Selected turn had no text to restore",
                                    restore.restore.restored_file_count,
                                    outcome.1
                                )
                            } else {
                                format!(
                                    "Restored {} file(s) and rolled back {} message(s). Edit the restored prompt and press Enter",
                                    restore.restore.restored_file_count,
                                    outcome.1
                                )
                            }
                        }
                        None if candidate.draft.text.trim().is_empty()
                            && candidate.draft.draft_attachments.is_empty() =>
                        {
                            format!(
                                "Rolled back {} message(s). Selected turn had no text to restore",
                                outcome.1
                            )
                        }
                        None => {
                            format!(
                                "Rolled back {} message(s). Edit the restored prompt and press Enter",
                                outcome.1
                            )
                        }
                    };
                    state.push_activity(format!(
                        "{} history to turn {} of {}: {}",
                        if outcome.2.is_some() {
                            "restored checkpoint and rolled back"
                        } else {
                            "rolled back"
                        },
                        selected + 1,
                        total,
                        preview
                    ));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("history rollback", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("History rollback failed: {message}");
                    state.push_activity(format!(
                        "history rollback failed: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
        Ok(())
    }
}
