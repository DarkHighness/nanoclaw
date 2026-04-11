use super::*;

mod history;
mod live_tasks;

impl CodeAgentTui {
    pub(super) async fn apply_command(&mut self, input: &str) -> Result<bool> {
        match parse_slash_command(input) {
            SlashCommand::Quit => Ok(true),
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
            SlashCommand::Image { path } => {
                self.attach_composer_image(&path).await;
                Ok(false)
            }
            SlashCommand::File { path } => {
                self.attach_composer_file(&path).await;
                Ok(false)
            }
            SlashCommand::Detach { index } => {
                self.detach_composer_attachment(index);
                Ok(false)
            }
            SlashCommand::MoveAttachment { from, to } => {
                self.move_composer_attachment(from, to);
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
            SlashCommand::Tools => {
                let tool_names = self.startup_snapshot().tool_names;
                self.ui_state.mutate(move |state| {
                    let lines = if tool_names.is_empty() {
                        vec![
                            InspectorEntry::section("Tools"),
                            InspectorEntry::Muted("No tools registered.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Tools"))
                            .chain(tool_names.iter().map(|tool| {
                                InspectorEntry::collection(tool.clone(), None::<String>)
                            }))
                            .collect()
                    };
                    state.show_main_view("Tool Catalog", lines);
                    state.status = "Listed core tools".to_string();
                    state.push_activity("inspected tool catalog");
                });
                Ok(false)
            }
            SlashCommand::Skills => {
                let skills = self.skills();
                self.ui_state.mutate(move |state| {
                    let lines = if skills.is_empty() {
                        vec![
                            InspectorEntry::section("Skills"),
                            InspectorEntry::Muted(
                                "No skills are available in the configured roots.".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Skills"))
                            .chain(skills.iter().map(|skill| {
                                InspectorEntry::collection(
                                    skill.name.clone(),
                                    Some(state::preview_text(&skill.description, 72)),
                                )
                            }))
                            .collect()
                    };
                    state.show_main_view("Skill Catalog", lines);
                    state.status = "Listed available skills".to_string();
                    state.push_activity("inspected skill catalog");
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
            SlashCommand::Mcp => {
                let servers: Vec<McpServerSummary> =
                    self.run_ui(UIAsyncCommand::ListMcpServers).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if servers.is_empty() {
                        vec![
                            InspectorEntry::section("MCP"),
                            InspectorEntry::Muted("No MCP servers connected.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP"))
                            .chain(servers.iter().map(format_mcp_server_summary_line))
                            .collect()
                    };
                    state.show_main_view("MCP", lines);
                    state.status = "Listed MCP servers".to_string();
                    state.push_activity("listed mcp servers");
                });
                Ok(false)
            }
            SlashCommand::Prompts => {
                let prompts: Vec<McpPromptSummary> =
                    self.run_ui(UIAsyncCommand::ListMcpPrompts).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if prompts.is_empty() {
                        vec![
                            InspectorEntry::section("MCP Prompts"),
                            InspectorEntry::Muted("No MCP prompts available.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP Prompts"))
                            .chain(prompts.iter().map(format_mcp_prompt_summary_line))
                            .collect()
                    };
                    state.show_main_view("Prompts", lines);
                    state.status = "Listed MCP prompts".to_string();
                    state.push_activity("listed mcp prompts");
                });
                Ok(false)
            }
            SlashCommand::Resources => {
                let resources: Vec<McpResourceSummary> =
                    self.run_ui(UIAsyncCommand::ListMcpResources).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if resources.is_empty() {
                        vec![
                            InspectorEntry::section("MCP Resources"),
                            InspectorEntry::Muted("No MCP resources available.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP Resources"))
                            .chain(resources.iter().map(format_mcp_resource_summary_line))
                            .collect()
                    };
                    state.show_main_view("Resources", lines);
                    state.status = "Listed MCP resources".to_string();
                    state.push_activity("listed mcp resources");
                });
                Ok(false)
            }
            SlashCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                let loaded: LoadedMcpPrompt = self
                    .run_ui(UIAsyncCommand::LoadMcpPrompt {
                        server_name: server_name.clone(),
                        prompt_name: prompt_name.clone(),
                    })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let inspector = build_mcp_prompt_inspector(&loaded);
                    state.restore_input_draft(state::composer_draft_from_messages(
                        &loaded.input_messages,
                    ));
                    state.show_main_view("Prompt", inspector);
                    state.status =
                        format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
                    state.push_activity(format!("loaded mcp prompt {server_name}/{prompt_name}"));
                });
                Ok(false)
            }
            SlashCommand::Resource { server_name, uri } => {
                let loaded: LoadedMcpResource = self
                    .run_ui(UIAsyncCommand::LoadMcpResource {
                        server_name: server_name.clone(),
                        uri: uri.clone(),
                    })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let inspector = build_mcp_resource_inspector(&loaded);
                    state
                        .restore_input_draft(state::composer_draft_from_parts(&loaded.input_parts));
                    state.show_main_view("Resource", inspector);
                    state.status = format!("Loaded MCP resource {server_name}:{uri} into input");
                    state.push_activity(format!("loaded mcp resource {server_name}:{uri}"));
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
                        Some("manual_command".to_string()),
                    )
                    .await;
                    return Ok(false);
                }
                self.start_command(RuntimeCommand::Steer {
                    message,
                    reason: Some("manual_command".to_string()),
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
            SlashCommand::LiveTasks => {
                let live_tasks: Vec<LiveTaskSummary> =
                    self.run_ui(UIAsyncCommand::ListLiveTasks).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if live_tasks.is_empty() {
                        vec![
                            InspectorEntry::section("Live Tasks"),
                            InspectorEntry::Muted(
                                "no live child tasks attached to the active root agent".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Live Tasks"))
                            .chain(live_tasks.iter().map(|task| {
                                InspectorEntry::transcript(format_live_task_summary_line(task))
                            }))
                            .collect()
                    };
                    state.show_main_view("Live Tasks", lines);
                    state.status = if live_tasks.is_empty() {
                        "No live child tasks attached".to_string()
                    } else {
                        format!(
                            "Listed {} live child task(s). Use /cancel_task <task-or-agent-ref> to stop one.",
                            live_tasks.len()
                        )
                    };
                    state.push_activity("listed live child tasks");
                });
                Ok(false)
            }
            SlashCommand::SpawnTask { role, prompt } => {
                let outcome: LiveTaskSpawnOutcome = self
                    .run_ui(UIAsyncCommand::SpawnLiveTask {
                        role: role.clone(),
                        prompt: prompt.clone(),
                    })
                    .await?;
                let inspector = format_live_task_spawn_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Live Task Spawn", inspector);
                    state.status = format!("Spawned live task {}", outcome.task.task_id);
                    state.push_activity(format!(
                        "spawned live task {} ({})",
                        outcome.task.task_id, outcome.task.role
                    ));
                });
                Ok(false)
            }
            SlashCommand::SendTask {
                task_or_agent_ref,
                message,
            } => {
                let Some(message) = message else {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Usage: /send_task <task-or-agent-ref> <message>".to_string();
                        state.push_activity("invalid /send_task invocation");
                    });
                    return Ok(false);
                };
                let outcome: LiveTaskMessageOutcome = self
                    .run_ui(UIAsyncCommand::SendLiveTask {
                        task_or_agent_ref: task_or_agent_ref.clone(),
                        message: message.clone(),
                    })
                    .await?;
                let inspector = format_live_task_message_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Live Task Message", inspector);
                    state.status = match outcome.action {
                        LiveTaskMessageAction::Sent => {
                            format!("Sent steer to live task {}", outcome.task_id)
                        }
                        LiveTaskMessageAction::AlreadyTerminal => {
                            format!("Live task {} was already terminal", outcome.task_id)
                        }
                    };
                    state.push_activity(match outcome.action {
                        LiveTaskMessageAction::Sent => {
                            format!("sent steer to {}", outcome.task_id)
                        }
                        LiveTaskMessageAction::AlreadyTerminal => {
                            format!("live task {} already terminal", outcome.task_id)
                        }
                    });
                });
                Ok(false)
            }
            SlashCommand::WaitTask { task_or_agent_ref } => {
                if self.operator_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current live-task operator action to finish".to_string();
                        state.push_activity("live task wait blocked by existing operator task");
                    });
                    return Ok(false);
                }
                self.start_wait_task(task_or_agent_ref);
                Ok(false)
            }
            SlashCommand::CancelTask {
                task_or_agent_ref,
                reason,
            } => {
                let outcome: LiveTaskControlOutcome = self
                    .run_ui(UIAsyncCommand::CancelLiveTask {
                        task_or_agent_ref: task_or_agent_ref.clone(),
                        reason: reason.clone(),
                    })
                    .await?;
                let inspector = format_live_task_control_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Live Task Control", inspector);
                    state.status = match outcome.action {
                        LiveTaskControlAction::Cancelled => {
                            format!("Cancelled live task {}", outcome.task_id)
                        }
                        LiveTaskControlAction::AlreadyTerminal => {
                            format!("Live task {} was already terminal", outcome.task_id)
                        }
                    };
                    state.push_activity(match outcome.action {
                        LiveTaskControlAction::Cancelled => {
                            format!("cancelled live task {}", outcome.task_id)
                        }
                        LiveTaskControlAction::AlreadyTerminal => {
                            format!("live task {} already terminal", outcome.task_id)
                        }
                    });
                });
                Ok(false)
            }
            command @ (SlashCommand::AgentSessions { .. }
            | SlashCommand::AgentSession { .. }
            | SlashCommand::Tasks { .. }
            | SlashCommand::Task { .. }
            | SlashCommand::Sessions { .. }
            | SlashCommand::Session { .. }
            | SlashCommand::Resume { .. }
            | SlashCommand::ExportSession { .. }
            | SlashCommand::ExportTranscript { .. }) => self.apply_history_command(command).await,
            SlashCommand::InvalidUsage(message) => {
                let lines = build_command_error_view(input, &message);
                self.ui_state.mutate(|state| {
                    state.status = "Command syntax error".to_string();
                    state.show_main_view("Command Error", lines);
                    state.push_activity("command parse error");
                });
                Ok(false)
            }
        }
    }
}
