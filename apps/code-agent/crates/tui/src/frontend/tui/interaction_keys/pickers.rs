use super::*;

impl CodeAgentTui {
    pub(crate) fn move_command_selection(&mut self, backwards: bool) -> bool {
        let snapshot = self.ui_state.snapshot();
        let Some(index) = move_slash_command_selection(
            &snapshot.input,
            snapshot.command_completion_index,
            backwards,
        ) else {
            return false;
        };
        self.ui_state
            .mutate(|state| state.command_completion_index = index);
        true
    }

    pub(crate) fn handle_statusline_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.statusline_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_statusline_picker(true);
                });
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_statusline_picker(false);
                });
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.statusline_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.statusline_picker.as_mut() {
                        picker.selected = status_line_fields().len().saturating_sub(1);
                    }
                });
                true
            }
            KeyCode::Char(' ') => {
                self.ui_state.mutate(|state| {
                    if let Some((field, enabled)) = state.toggle_selected_statusline_field() {
                        let label = status_line_fields()
                            .iter()
                            .find(|spec| spec.field == field)
                            .map(|spec| spec.label)
                            .unwrap_or("field");
                        state.status = format!(
                            "Status line {} {}",
                            label,
                            if enabled { "enabled" } else { "hidden" }
                        );
                        state.push_activity(format!(
                            "status line {} {}",
                            label,
                            if enabled { "enabled" } else { "hidden" }
                        ));
                    }
                });
                true
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.ui_state.mutate(|state| {
                    state.close_statusline_picker();
                    state.status = "Closed status line picker".to_string();
                    state.push_activity("closed status line picker");
                });
                true
            }
            _ => false,
        }
    }

    pub(crate) fn handle_pending_control_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.pending_control_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_pending_control_picker(true);
                });
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_pending_control_picker(false);
                });
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.pending_control_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.pending_control_picker.as_mut() {
                        picker.selected = state.pending_controls.len().saturating_sub(1);
                    }
                });
                true
            }
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || matches!(key.code, KeyCode::Delete | KeyCode::Backspace) =>
            {
                if let Some(selected) = snapshot.selected_pending_control() {
                    match self.remove_pending_control(&selected.id) {
                        Ok(removed) => {
                            let removed_id = removed.id.clone();
                            self.sync_runtime_control_state();
                            self.ui_state.mutate(|state| {
                                if state
                                    .editing_pending_control
                                    .as_ref()
                                    .is_some_and(|editing| editing.id == removed_id)
                                {
                                    state.clear_pending_control_edit();
                                    state.clear_input();
                                }
                                if state.pending_controls.is_empty() {
                                    state.close_pending_control_picker();
                                }
                                state.status = format!(
                                    "Withdrew queued {} {}",
                                    pending_control_kind_label(removed.kind),
                                    preview_id(&removed.id)
                                );
                                state.push_activity(format!(
                                    "withdrew queued {} {}",
                                    pending_control_kind_label(removed.kind),
                                    preview_id(&removed.id)
                                ));
                            });
                        }
                        Err(error) => {
                            let message =
                                summarize_nonfatal_error("withdraw pending control", &error);
                            self.ui_state.mutate(|state| {
                                state.status =
                                    format!("Failed to withdraw pending control: {message}");
                                state.push_activity(format!(
                                    "failed to withdraw pending control: {}",
                                    state::preview_text(&message, 56)
                                ));
                            });
                        }
                    }
                }
                true
            }
            KeyCode::Enter => {
                if let Some(selected) = snapshot.selected_pending_control() {
                    self.ui_state.mutate(|state| {
                        state.begin_pending_control_edit();
                        state.status = format!(
                            "Editing queued {} {}",
                            pending_control_kind_label(selected.kind),
                            preview_id(&selected.id)
                        );
                        state.push_activity(format!(
                            "editing queued {} {}",
                            pending_control_kind_label(selected.kind),
                            preview_id(&selected.id)
                        ));
                    });
                }
                true
            }
            KeyCode::Esc => {
                self.ui_state.mutate(|state| {
                    state.close_pending_control_picker();
                    state.status = "Closed pending controls".to_string();
                    state.push_activity("closed pending controls");
                });
                true
            }
            _ => false,
        }
    }

    pub(crate) fn handle_thinking_effort_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.thinking_effort_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_thinking_effort_picker(true);
                });
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_thinking_effort_picker(false);
                });
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.thinking_effort_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.thinking_effort_picker.as_mut() {
                        picker.selected = state
                            .session
                            .supported_model_reasoning_efforts
                            .len()
                            .saturating_sub(1);
                    }
                });
                true
            }
            KeyCode::Enter => {
                if let Some(level) = snapshot.selected_thinking_effort() {
                    self.set_model_reasoning_effort(&level);
                }
                self.ui_state
                    .mutate(|state| state.close_thinking_effort_picker());
                true
            }
            KeyCode::Esc => {
                self.ui_state.mutate(|state| {
                    state.close_thinking_effort_picker();
                    state.status = "Closed thinking effort picker".to_string();
                    state.push_activity("closed thinking effort picker");
                });
                true
            }
            _ => false,
        }
    }

    pub(crate) fn handle_theme_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.theme_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_theme_picker(true);
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_theme_picker(false);
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.theme_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.theme_picker.as_mut() {
                        picker.selected = state.themes.len().saturating_sub(1);
                    }
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::Enter => {
                if let Some(theme_id) = snapshot.selected_theme() {
                    self.apply_tui_theme(&theme_id, true, snapshot.original_theme());
                }
                self.ui_state.mutate(|state| state.close_theme_picker());
                true
            }
            KeyCode::Esc => {
                let previous = snapshot.original_theme();
                self.ui_state.mutate(|state| state.close_theme_picker());
                if let Some(previous) = previous {
                    self.apply_tui_theme(&previous, false, None);
                    self.ui_state.mutate(|state| {
                        state.status = format!("Restored theme {previous}");
                        state.push_activity(format!("restored theme {previous}"));
                    });
                } else {
                    self.ui_state.mutate(|state| {
                        state.status = "Closed theme picker".to_string();
                        state.push_activity("closed theme picker");
                    });
                }
                true
            }
            _ => false,
        }
    }
}
