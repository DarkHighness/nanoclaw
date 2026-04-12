use super::*;

impl CodeAgentTui {
    pub(crate) async fn apply_runtime_activity_command(
        &mut self,
        command: SlashCommand,
    ) -> Result<bool> {
        match command {
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
            SlashCommand::Monitors { include_closed } => {
                let monitors: Vec<LiveMonitorSummary> = self
                    .run_ui(UIAsyncCommand::ListMonitors { include_closed })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let lines = if monitors.is_empty() {
                        vec![
                            InspectorEntry::section("Monitors"),
                            InspectorEntry::Muted(
                                "no background monitors attached to the active session".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Monitors"))
                            .chain(
                                monitors
                                    .iter()
                                    .map(|monitor| {
                                        InspectorEntry::transcript(
                                            format_live_monitor_summary_line(monitor),
                                        )
                                    }),
                            )
                            .collect()
                    };
                    state.show_main_view("Monitors", lines);
                    state.status = if monitors.is_empty() {
                        "No background monitors attached".to_string()
                    } else if include_closed {
                        format!(
                            "Listed {} monitor(s), including closed ones. Use /stop_monitor <monitor-ref> to stop an active monitor.",
                            monitors.len()
                        )
                    } else {
                        format!(
                            "Listed {} active monitor(s). Use /stop_monitor <monitor-ref> to stop one.",
                            monitors.len()
                        )
                    };
                    state.push_activity("listed background monitors");
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
            SlashCommand::StopMonitor {
                monitor_ref,
                reason,
            } => {
                let outcome: LiveMonitorControlOutcome = self
                    .run_ui(UIAsyncCommand::StopMonitor {
                        monitor_ref: monitor_ref.clone(),
                        reason: reason.clone(),
                    })
                    .await?;
                let inspector = format_live_monitor_control_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Monitor Control", inspector);
                    state.status = match outcome.action {
                        LiveMonitorControlAction::Stopped => {
                            format!("Stopped monitor {}", outcome.monitor.monitor_id)
                        }
                        LiveMonitorControlAction::AlreadyTerminal => {
                            format!(
                                "Monitor {} was already terminal",
                                outcome.monitor.monitor_id
                            )
                        }
                    };
                    state.push_activity(match outcome.action {
                        LiveMonitorControlAction::Stopped => {
                            format!("stopped monitor {}", outcome.monitor.monitor_id)
                        }
                        LiveMonitorControlAction::AlreadyTerminal => {
                            format!("monitor {} already terminal", outcome.monitor.monitor_id)
                        }
                    });
                });
                Ok(false)
            }
            _ => unreachable!("runtime activity handler received unexpected command"),
        }
    }
}
