use super::*;

impl CodeAgentTui {
    pub(crate) fn handle_user_input_key(&mut self, key: KeyEvent) -> bool {
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

    pub(crate) fn sync_user_input_prompt(&mut self, prompt: Option<&UserInputPrompt>) {
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

    pub(crate) fn advance_user_input_flow(&mut self, prompt: &UserInputPrompt) {
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
