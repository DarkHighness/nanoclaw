use super::*;

impl CodeAgentTui {
    pub(super) async fn apply_history_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::AgentSessions { session_ref } => {
                let agent_sessions: Vec<PersistedAgentSessionSummary> = self
                    .run_ui(UIAsyncCommand::ListAgentSessions {
                        session_ref: session_ref.clone(),
                    })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let lines = if agent_sessions.is_empty() {
                        vec![
                            InspectorEntry::section("Agent Sessions"),
                            InspectorEntry::Muted(
                                "no persisted agent sessions recorded yet".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Agent Sessions"))
                            .chain(agent_sessions.iter().take(16).map(|summary| {
                                format_agent_session_summary_collection(summary)
                            }))
                            .collect()
                    };
                    state.show_main_view("Agent Sessions", lines);
                    state.status = if agent_sessions.is_empty() {
                        "No agent sessions available yet".to_string()
                    } else {
                        format!(
                            "Listed {} agent sessions. Enter opens details; r resumes when available.",
                            agent_sessions.len()
                        )
                    };
                    state.push_activity("listed persisted agent sessions");
                });
                Ok(false)
            }
            SlashCommand::AgentSession { agent_session_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before opening another agent session"
                                .to_string();
                        state.push_activity("agent session replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded: LoadedAgentSession = self
                    .run_ui(UIAsyncCommand::LoadAgentSession {
                        agent_session_ref: agent_session_ref.clone(),
                    })
                    .await?;
                let inspector = format_agent_session_inspector(&loaded);
                let transcript = format_visible_transcript_lines(&loaded.transcript);
                let tracked_tasks = restore_tracked_tasks(&loaded.events);
                let agent_session_ref_preview = preview_id(&loaded.summary.agent_session_ref);
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.show_transcript_pane();
                    state.follow_transcript = false;
                    state.inspector_title = "Agent Session".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.replace_tracked_tasks(tracked_tasks);
                    state.status = format!(
                        "Loaded agent session {} with {} transcript messages",
                        agent_session_ref_preview, transcript_count
                    );
                    state.push_activity(format!(
                        "loaded agent session {}",
                        agent_session_ref_preview
                    ));
                });
                Ok(false)
            }
            SlashCommand::Tasks { session_ref } => {
                let tasks: Vec<PersistedTaskSummary> = self
                    .run_ui(UIAsyncCommand::ListTasks {
                        session_ref: session_ref.clone(),
                    })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let lines = if tasks.is_empty() {
                        vec![
                            InspectorEntry::section("Tasks"),
                            InspectorEntry::Muted("no persisted tasks recorded yet".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Tasks"))
                            .chain(tasks.iter().take(16).map(format_task_summary_collection))
                            .collect()
                    };
                    state.show_main_view("Tasks", lines);
                    state.status = if tasks.is_empty() {
                        "No tasks available yet".to_string()
                    } else {
                        format!(
                            "Listed {} tasks. Enter opens the selected task.",
                            tasks.len()
                        )
                    };
                    state.push_activity("listed persisted tasks");
                });
                Ok(false)
            }
            SlashCommand::Sessions { query } => {
                if let Some(query) = query {
                    let matches: Vec<PersistedSessionSearchMatch> = self
                        .run_ui(UIAsyncCommand::SearchSessions {
                            query: query.clone(),
                        })
                        .await?;
                    let stored_session_count = self.refresh_stored_session_count().await.ok();
                    self.ui_state.mutate(move |state| {
                        if let Some(stored_session_count) = stored_session_count {
                            state.session.stored_session_count = stored_session_count;
                        }
                        let lines = if matches.is_empty() {
                            vec![
                                InspectorEntry::section("Session Search"),
                                InspectorEntry::Muted(format!("no sessions matched `{query}`")),
                            ]
                        } else {
                            std::iter::once(InspectorEntry::section("Session Search"))
                                .chain(
                                    matches
                                        .iter()
                                        .take(12)
                                        .map(format_session_search_collection),
                                )
                                .collect()
                        };
                        state.show_main_view("Session Search", lines);
                        state.status = if matches.is_empty() {
                            format!("No sessions matched `{query}`")
                        } else {
                            format!(
                                "Found {} matching sessions. Enter opens the selected session.",
                                matches.len()
                            )
                        };
                        state.push_activity(format!(
                            "searched sessions: {}",
                            state::preview_text(&query, 40)
                        ));
                    });
                } else {
                    let sessions: Vec<PersistedSessionSummary> =
                        self.run_ui(UIAsyncCommand::ListSessions).await?;
                    let stored_session_count = sessions.len();
                    self.ui_state.mutate(move |state| {
                        state.session.stored_session_count = stored_session_count;
                        let lines = if sessions.is_empty() {
                            vec![
                                InspectorEntry::section("Sessions"),
                                InspectorEntry::Muted(
                                    "no persisted sessions recorded yet".to_string(),
                                ),
                            ]
                        } else {
                            std::iter::once(InspectorEntry::section("Sessions"))
                                .chain(
                                    sessions
                                        .iter()
                                        .take(12)
                                        .map(format_session_summary_collection),
                                )
                                .collect()
                        };
                        state.show_main_view("Sessions", lines);
                        state.status = if sessions.is_empty() {
                            "No sessions available yet".to_string()
                        } else {
                            format!(
                                "Listed {} sessions. Enter opens the selected session.",
                                sessions.len()
                            )
                        };
                        state.push_activity("listed persisted sessions");
                    });
                }
                Ok(false)
            }
            SlashCommand::Session { session_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before opening another session".to_string();
                        state.push_activity("session replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded: LoadedSession = self
                    .run_ui(UIAsyncCommand::LoadSession {
                        session_ref: session_ref.clone(),
                    })
                    .await?;
                let inspector = format_session_inspector(&loaded);
                let transcript = format_session_transcript_lines(&loaded);
                let tracked_tasks = restore_tracked_tasks(&loaded.events);
                let session_ref_preview = preview_id(loaded.summary.session_id.as_str());
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.show_transcript_pane();
                    state.follow_transcript = false;
                    state.inspector_title = "Session".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.replace_tracked_tasks(tracked_tasks);
                    state.status = format!(
                        "Loaded session {} with {} transcript messages",
                        session_ref_preview, transcript_count
                    );
                    state.push_activity(format!("loaded session {}", session_ref_preview));
                });
                Ok(false)
            }
            SlashCommand::Task { task_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before opening another task".to_string();
                        state.push_activity("task replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded: LoadedTask = self
                    .run_ui(UIAsyncCommand::LoadTask {
                        task_ref: task_ref.clone(),
                    })
                    .await?;
                let inspector = format_task_inspector(&loaded);
                let transcript = format_visible_transcript_lines(&loaded.child_transcript);
                let task_id = loaded.summary.task_id.clone();
                let transcript_count = loaded.child_transcript.len();
                self.ui_state.mutate(move |state| {
                    state.show_transcript_pane();
                    state.follow_transcript = false;
                    state.inspector_title = "Task".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.replace_tracked_tasks(vec![
                        crate::frontend::tui::state::TrackedTaskSummary {
                            task_id: loaded.summary.task_id.clone(),
                            role: loaded.summary.role.clone(),
                            origin: loaded.summary.origin,
                            status: loaded.summary.status,
                            summary: Some(loaded.summary.summary.clone()),
                            parent_agent_id: None,
                            child_agent_id: None,
                        },
                    ]);
                    state.status = format!(
                        "Loaded task {} with {} child transcript messages",
                        task_id, transcript_count
                    );
                    state.push_activity(format!("loaded task {}", task_id));
                });
                Ok(false)
            }
            SlashCommand::Resume { agent_session_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before resuming another session".to_string();
                        state.push_activity("resume blocked while turn running");
                    });
                    return Ok(false);
                }
                let outcome: SessionOperationOutcome = self
                    .run_ui(UIAsyncCommand::ApplySessionOperation {
                        operation: SessionOperation::ResumeAgentSession { agent_session_ref },
                    })
                    .await?;
                self.replace_after_session_operation(outcome, 0);
                Ok(false)
            }
            SlashCommand::ExportSession { session_ref, path } => {
                let export: SessionExportArtifact = self
                    .run_ui(UIAsyncCommand::ExportSession {
                        session_ref: session_ref.clone(),
                        path: path.clone(),
                    })
                    .await?;
                let inspector = format_session_export_result(&export);
                let session_ref_preview = preview_id(export.session_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Export", inspector);
                    state.status = format!(
                        "Exported session {} to {}",
                        session_ref_preview, output_path
                    );
                    state.push_activity(format!("exported session {}", session_ref_preview));
                });
                Ok(false)
            }
            SlashCommand::ExportTranscript { session_ref, path } => {
                let export: SessionExportArtifact = self
                    .run_ui(UIAsyncCommand::ExportSessionTranscript {
                        session_ref: session_ref.clone(),
                        path: path.clone(),
                    })
                    .await?;
                let inspector = format_session_export_result(&export);
                let session_ref_preview = preview_id(export.session_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Export", inspector);
                    state.status = format!(
                        "Exported transcript {} to {}",
                        session_ref_preview, output_path
                    );
                    state.push_activity(format!("exported transcript {}", session_ref_preview));
                });
                Ok(false)
            }
            _ => unreachable!("history handler received non-history command"),
        }
    }
}
