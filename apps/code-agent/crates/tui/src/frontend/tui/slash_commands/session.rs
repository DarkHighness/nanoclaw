use super::*;

impl CodeAgentTui {
    pub(crate) async fn apply_session_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::Status => {
                self.ui_state.mutate(|state| {
                    state.show_main_view("Guide", build_startup_inspector(&state.session));
                    state.status = "Restored session overview".to_string();
                    state.push_activity("restored session overview");
                });
                Ok(false)
            }
            SlashCommand::Details => {
                self.ui_state.mutate(|state| {
                    state.show_tool_details = !state.show_tool_details;
                    let visibility = if state.show_tool_details {
                        "expanded"
                    } else {
                        "collapsed"
                    };
                    state.status = format!("Tool details {visibility}");
                    state.push_activity(format!("tool details {visibility}"));
                });
                Ok(false)
            }
            SlashCommand::StatusLine => {
                self.ui_state.mutate(|state| {
                    state.open_statusline_picker();
                    state.status = "Opened status line picker".to_string();
                    state.push_activity("opened status line picker");
                });
                Ok(false)
            }
            SlashCommand::Thinking { effort } => {
                match effort.as_deref() {
                    Some(effort) => self.set_model_reasoning_effort(effort),
                    None => self.ui_state.mutate(|state| {
                        state.open_thinking_effort_picker();
                        state.status = "Opened thinking effort picker".to_string();
                        state.push_activity("opened thinking effort picker");
                    }),
                }
                Ok(false)
            }
            SlashCommand::Theme { name } => {
                match name.as_deref() {
                    Some(theme_id) => self.apply_tui_theme(theme_id, true, None),
                    None => self.ui_state.mutate(|state| {
                        state.open_theme_picker();
                        state.status =
                            "Opened theme picker; move to preview, Enter to save".to_string();
                        state.push_activity("opened theme picker");
                    }),
                }
                Ok(false)
            }
            SlashCommand::Help { query } => {
                let title = query
                    .as_deref()
                    .filter(|query| !query.trim().is_empty())
                    .map(|query| format!("Command Palette · {}", query.trim()))
                    .unwrap_or_else(|| "Command Palette".to_string());
                let lines = command_palette_lines_for(query.as_deref());
                self.ui_state.mutate(|state| {
                    state.show_main_view(title, lines);
                    state.status = "Opened command palette".to_string();
                    state.push_activity("opened command palette");
                });
                Ok(false)
            }
            SlashCommand::Diagnostics => {
                let diagnostics = self.startup_diagnostics();
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Diagnostics", format_startup_diagnostics(&diagnostics));
                    state.status = "Opened startup diagnostics".to_string();
                    state.push_activity("inspected startup diagnostics");
                });
                Ok(false)
            }
            SlashCommand::Steer { message } => {
                let Some(message) = message else {
                    self.ui_state.mutate(|state| {
                        state.status = "Usage: /steer <notes>".to_string();
                        state.push_activity("invalid /steer invocation");
                    });
                    return Ok(false);
                };
                if self.turn_task.is_some() {
                    self.schedule_runtime_steer_while_active(
                        message,
                        Some(crate::interaction::PendingControlReason::ManualCommand),
                    )
                    .await;
                    return Ok(false);
                }
                self.start_command(RuntimeCommand::Steer {
                    message,
                    reason: Some(
                        crate::interaction::PendingControlReason::ManualCommand.runtime_value(),
                    ),
                })
                .await;
                Ok(false)
            }
            SlashCommand::Queue => {
                let pending = self.pending_controls();
                let opened = !pending.is_empty();
                self.ui_state.mutate(|state| {
                    state.sync_pending_controls(pending);
                    if opened {
                        let _ = state.open_pending_control_picker(true);
                    }
                });
                self.ui_state.mutate(|state| {
                    if opened {
                        state.status = "Opened pending controls".to_string();
                        state.push_activity("opened pending controls");
                    } else {
                        state.status = "No pending prompts or steers".to_string();
                        state.push_activity("no pending controls");
                    }
                });
                Ok(false)
            }
            SlashCommand::Permissions { mode } => {
                if let Some(mode) = mode {
                    if self.turn_task.is_some() {
                        self.ui_state.mutate(|state| {
                            state.status =
                                "Wait for the current turn before switching sandbox mode"
                                    .to_string();
                            state.push_activity(
                                "permissions mode switch blocked while turn running",
                            );
                        });
                        return Ok(false);
                    }

                    let outcome: crate::interaction::SessionPermissionModeOutcome = self
                        .run_ui(UIAsyncCommand::SetPermissionMode { mode })
                        .await?;
                    let snapshot = self.startup_snapshot();
                    let (turn_grants, session_grants) = self.permission_grant_profiles();
                    let inspector =
                        build_permissions_inspector(&snapshot, &turn_grants, &session_grants);
                    self.sync_session_summary_from_snapshot(&snapshot);
                    self.ui_state.mutate(move |state| {
                        state.show_main_view("Permissions", inspector);
                        if outcome.previous == outcome.current {
                            state.status =
                                format!("Permissions mode already {}", outcome.current.as_str());
                            state.push_activity(format!(
                                "inspected permissions mode {}",
                                outcome.current.as_str()
                            ));
                        } else {
                            state.status =
                                format!("Permissions mode set to {}", outcome.current.as_str());
                            state.push_activity(format!(
                                "permissions mode {} -> {}",
                                outcome.previous.as_str(),
                                outcome.current.as_str()
                            ));
                        }
                    });
                } else {
                    let snapshot = self.startup_snapshot();
                    let (turn_grants, session_grants) = self.permission_grant_profiles();
                    let inspector =
                        build_permissions_inspector(&snapshot, &turn_grants, &session_grants);
                    self.ui_state.mutate(move |state| {
                        state.show_main_view("Permissions", inspector);
                        state.status = "Opened permissions inspector".to_string();
                        state.push_activity("opened permissions inspector");
                    });
                }
                Ok(false)
            }
            SlashCommand::New => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before starting a new session".to_string();
                        state.push_activity("new session blocked while turn running");
                    });
                    return Ok(false);
                }

                let dropped_commands: usize =
                    self.run_ui(UIAsyncCommand::ClearQueuedCommands).await?;
                let outcome: SessionOperationOutcome = self
                    .run_ui(UIAsyncCommand::ApplySessionOperation {
                        operation: SessionOperation::StartFresh,
                    })
                    .await?;
                self.replace_after_session_operation(outcome, dropped_commands);
                Ok(false)
            }
            SlashCommand::Compact { notes } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status = "Wait for the current turn before compacting".to_string();
                        state.push_activity("compact blocked while turn running");
                    });
                    return Ok(false);
                }
                let compacted: bool = self.run_ui(UIAsyncCommand::CompactNow { notes }).await?;
                self.apply_backend_events();
                if !compacted {
                    self.ui_state.mutate(|state| {
                        state.status = "Compaction skipped".to_string();
                        state.push_activity("compaction skipped");
                    });
                }
                Ok(false)
            }
            SlashCommand::Btw { question } => {
                let Some(question) = question else {
                    self.ui_state.mutate(|state| {
                        state.status = "Usage: /btw <question>".to_string();
                        state.push_activity("invalid /btw invocation");
                    });
                    return Ok(false);
                };
                if self.operator_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current operator-side action before running /btw"
                                .to_string();
                        state.push_activity("/btw blocked by operator task");
                    });
                    return Ok(false);
                }
                self.start_side_question(question);
                Ok(false)
            }
            _ => unreachable!("session handler received non-session command"),
        }
    }
}
