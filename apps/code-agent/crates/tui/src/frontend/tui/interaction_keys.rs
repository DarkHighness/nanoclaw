use super::*;

impl CodeAgentTui {
    pub(super) fn move_command_selection(&mut self, backwards: bool) -> bool {
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

    pub(super) fn handle_approval_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.approval_prompt() else {
            return false;
        };
        if let Some(decision) = approval_decision_for_key(key) {
            let approved = matches!(decision, crate::interaction::ApprovalDecision::Approve);
            if self.resolve_approval(decision) {
                self.ui_state.mutate(|state| {
                    if approved {
                        state.status = format!("Approved {}", prompt.tool_name);
                        state.push_activity(format!("approved {}", prompt.tool_name));
                    } else {
                        state.status = format!("Denied {}", prompt.tool_name);
                        state.push_activity(format!("denied {}", prompt.tool_name));
                    }
                });
            }
            return true;
        }
        true
    }

    pub(super) fn handle_permission_request_key(&mut self, key: KeyEvent) -> bool {
        let Some(_prompt) = self.permission_request_prompt() else {
            return false;
        };
        let decision = match key.code {
            KeyCode::Char('y') => Some(PermissionRequestDecision::GrantOnce),
            KeyCode::Char('a') => Some(PermissionRequestDecision::GrantForSession),
            KeyCode::Char('n') | KeyCode::Esc => Some(PermissionRequestDecision::Deny),
            _ => None,
        };
        if let Some(decision) = decision {
            if self.resolve_permission_request(decision) {
                self.ui_state.mutate(|state| match decision {
                    PermissionRequestDecision::GrantOnce => {
                        state.status = "Granted additional permissions for the turn".to_string();
                        state.push_activity("granted additional permissions for the turn");
                    }
                    PermissionRequestDecision::GrantForSession => {
                        state.status = "Granted additional permissions for the session".to_string();
                        state.push_activity("granted additional permissions for the session");
                    }
                    PermissionRequestDecision::Deny => {
                        state.status = "Denied additional permissions".to_string();
                        state.push_activity("denied additional permissions");
                    }
                });
            }
            return true;
        }
        true
    }

    pub(super) fn handle_user_input_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            return false;
        }
        let Some(prompt) = self.user_input_prompt() else {
            return false;
        };
        self.sync_user_input_prompt(Some(&prompt));
        let Some(flow) = self.active_user_input.as_mut() else {
            return true;
        };
        let Some(question) = prompt.questions.get(flow.current_question) else {
            return true;
        };

        if flow.collecting_other_note {
            match key.code {
                KeyCode::Enter => {
                    let note = self.ui_state.take_input();
                    let note = note.trim();
                    if note.is_empty() {
                        self.ui_state.mutate(|state| {
                            state.status =
                                format!("Other note for {} cannot be empty", question.header);
                            state.push_activity(format!(
                                "rejected empty other note for {}",
                                question.id
                            ));
                        });
                        return true;
                    }
                    flow.answers.insert(
                        question.id.clone(),
                        UserInputAnswer {
                            answers: vec!["Other".to_string(), format!("user_note: {note}")],
                        },
                    );
                    flow.collecting_other_note = false;
                    self.advance_user_input_flow(&prompt);
                    true
                }
                KeyCode::Esc => {
                    flow.collecting_other_note = false;
                    self.ui_state.mutate(|state| {
                        state.clear_input();
                        state.status = format!("Returned to {} options", question.header);
                        state.push_activity(format!("returned to {} options", question.id));
                    });
                    true
                }
                KeyCode::Up => {
                    self.ui_state.mutate(|state| {
                        if !state.browse_input_history(true) {
                            let _ = state.move_input_cursor_vertical(true)
                                || state.move_input_cursor_home();
                        }
                    });
                    true
                }
                KeyCode::Down => {
                    self.ui_state.mutate(|state| {
                        if !state.browse_input_history(false) {
                            let _ = state.move_input_cursor_vertical(false)
                                || state.move_input_cursor_end();
                        }
                    });
                    true
                }
                KeyCode::Left => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_left();
                    });
                    true
                }
                KeyCode::Right => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_right();
                    });
                    true
                }
                KeyCode::Home => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_home();
                    });
                    true
                }
                KeyCode::End => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_end();
                    });
                    true
                }
                KeyCode::Backspace => {
                    self.ui_state.mutate(|state| {
                        state.pop_input_char();
                    });
                    true
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = self.kill_input_to_end();
                    true
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = self.yank_kill_buffer();
                    true
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.ui_state.mutate(|state| {
                        state.push_input_char(ch);
                    });
                    true
                }
                _ => true,
            }
        } else {
            match key.code {
                KeyCode::Esc => {
                    if self.cancel_user_input("operator cancelled user input request") {
                        self.active_user_input = None;
                        self.ui_state.mutate(|state| {
                            state.clear_input();
                            state.status = "Cancelled user input request".to_string();
                            state.push_activity("cancelled user input request");
                        });
                    }
                    true
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let Some(digit) = ch.to_digit(10) else {
                        return true;
                    };
                    if digit == 0 {
                        flow.collecting_other_note = true;
                        self.ui_state.mutate(|state| {
                            state.clear_input();
                            state.status =
                                format!("Provide an alternate answer for {}", question.header);
                            state.push_activity(format!(
                                "collecting other note for {}",
                                question.id
                            ));
                        });
                        return true;
                    }
                    let option_index = digit as usize - 1;
                    if let Some(option) = question.options.get(option_index) {
                        flow.answers.insert(
                            question.id.clone(),
                            UserInputAnswer {
                                answers: vec![option.label.clone()],
                            },
                        );
                        self.advance_user_input_flow(&prompt);
                    } else {
                        self.ui_state.mutate(|state| {
                            state.status = format!("{} has no option {}", question.header, digit);
                            state.push_activity(format!(
                                "invalid selection {} for {}",
                                digit, question.id
                            ));
                        });
                    }
                    true
                }
                _ => true,
            }
        }
    }

    pub(super) fn handle_statusline_picker_key(&mut self, key: KeyEvent) -> bool {
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

    pub(super) fn handle_pending_control_picker_key(&mut self, key: KeyEvent) -> bool {
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

    pub(super) fn handle_thinking_effort_picker_key(&mut self, key: KeyEvent) -> bool {
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

    pub(super) fn handle_theme_picker_key(&mut self, key: KeyEvent) -> bool {
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

    pub(super) fn sync_user_input_prompt(&mut self, prompt: Option<&UserInputPrompt>) {
        match prompt {
            Some(prompt)
                if self
                    .active_user_input
                    .as_ref()
                    .is_some_and(|flow| flow.prompt_id == prompt.prompt_id) => {}
            Some(prompt) => {
                self.active_user_input = Some(ActiveUserInputState::new(prompt.prompt_id.clone()));
                let status = format!(
                    "Awaiting user input · {} question(s)",
                    prompt.questions.len()
                );
                self.ui_state.mutate(|state| {
                    state.clear_input();
                    state.status = status;
                    state.push_activity("opened user input prompt");
                });
            }
            None if self.active_user_input.take().is_some() => {
                self.ui_state.mutate(|state| {
                    state.clear_input();
                });
            }
            None => {}
        }
    }

    pub(super) fn advance_user_input_flow(&mut self, prompt: &UserInputPrompt) {
        let Some(flow) = self.active_user_input.as_mut() else {
            return;
        };
        let next_question = flow.current_question + 1;
        if next_question >= prompt.questions.len() {
            let submission = UserInputSubmission {
                answers: flow.answers.clone(),
            };
            if self.resolve_user_input(submission) {
                self.active_user_input = None;
                self.ui_state.mutate(|state| {
                    state.clear_input();
                    state.status = "Submitted user input answers".to_string();
                    state.push_activity("submitted user input answers");
                });
            }
            return;
        }

        flow.current_question = next_question;
        flow.collecting_other_note = false;
        let next_header = prompt.questions[next_question].header.clone();
        self.ui_state.mutate(|state| {
            state.clear_input();
            state.status = format!("Next user input question · {next_header}");
            state.push_activity(format!("advanced to user input question {next_header}"));
        });
    }
}
