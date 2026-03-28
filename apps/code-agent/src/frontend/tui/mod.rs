mod approval;
mod commands;
mod history;
mod observer;
mod render;
mod state;

use crate::backend::{CodeAgentSession, preview_id};
pub(crate) use approval::{ApprovalBridge, InteractiveToolApprovalHandler};
use commands::{SlashCommand, parse_slash_command};
use history::{
    format_export_result, format_run_inspector, format_run_search_line, format_run_summary_line,
    format_transcript_lines,
};
use observer::SharedRenderObserver;
use render::render;
pub(crate) use state::SharedUiState;
use state::TuiState;

use agent::{RuntimeCommand, RuntimeCommandQueue};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};
use std::sync::Arc;
use tokio::task::{JoinHandle, spawn_local};
use tokio::time::{Duration, sleep};

pub(crate) fn make_tui_support() -> (
    SharedUiState,
    ApprovalBridge,
    Arc<InteractiveToolApprovalHandler>,
) {
    let ui_state = SharedUiState::new();
    let approval_bridge = ApprovalBridge::default();
    let handler = Arc::new(InteractiveToolApprovalHandler::new(
        approval_bridge.clone(),
        ui_state.clone(),
    ));
    (ui_state, approval_bridge, handler)
}

pub struct CodeAgentTui {
    session: CodeAgentSession,
    initial_prompt: Option<String>,
    ui_state: SharedUiState,
    approval_bridge: ApprovalBridge,
    command_queue: RuntimeCommandQueue,
    turn_task: Option<JoinHandle<Result<()>>>,
}

impl CodeAgentTui {
    pub fn new(
        session: CodeAgentSession,
        initial_prompt: Option<String>,
        ui_state: SharedUiState,
        approval_bridge: ApprovalBridge,
    ) -> Self {
        Self {
            session,
            initial_prompt,
            ui_state,
            approval_bridge,
            command_queue: RuntimeCommandQueue::new(),
            turn_task: None,
        }
    }

    pub async fn run(mut self) -> Result<()> {
        self.ui_state.replace(self.startup_state());

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        if let Some(prompt) = self.initial_prompt.take() {
            self.start_turn(prompt).await;
        }

        let result = self.event_loop(&mut terminal).await;
        let _ = self
            .session
            .end_session(Some("operator_exit".to_string()))
            .await;

        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        loop {
            self.maybe_finish_turn().await?;

            let snapshot = self.ui_state.snapshot();
            let approval = self.approval_bridge.snapshot();
            terminal.draw(|frame| render(frame, &snapshot, approval.as_ref()))?;

            if !event::poll(Duration::ZERO)? {
                sleep(Duration::from_millis(16)).await;
                continue;
            }
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if self.handle_approval_key(key) {
                    continue;
                }
                match key.code {
                    KeyCode::Tab => {
                        self.ui_state.mutate(|state| {
                            state.cycle_focus_forward();
                            state.status =
                                format!("Focus moved to {}", state.focus.title().to_lowercase());
                        });
                    }
                    KeyCode::BackTab => {
                        self.ui_state.mutate(|state| {
                            state.cycle_focus_backward();
                            state.status =
                                format!("Focus moved to {}", state.focus.title().to_lowercase());
                        });
                    }
                    KeyCode::Up => {
                        self.ui_state.mutate(|state| state.scroll_focused(-1));
                    }
                    KeyCode::Down => {
                        self.ui_state.mutate(|state| state.scroll_focused(1));
                    }
                    KeyCode::PageUp => {
                        self.ui_state.mutate(|state| state.scroll_focused(-8));
                    }
                    KeyCode::PageDown => {
                        self.ui_state.mutate(|state| state.scroll_focused(8));
                    }
                    KeyCode::Home => {
                        self.ui_state.mutate(|state| state.scroll_focused_home());
                    }
                    KeyCode::End => {
                        self.ui_state.mutate(|state| state.scroll_focused_end());
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        let input = self.ui_state.take_input();
                        if input.trim().is_empty() {
                            continue;
                        }
                        if input.starts_with('/') {
                            if self.apply_command(&input).await? {
                                return Ok(());
                            }
                        } else {
                            self.start_turn(input).await;
                        }
                    }
                    KeyCode::Backspace => {
                        self.ui_state.mutate(|state| {
                            state.input.pop();
                        });
                    }
                    KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.ui_state.mutate(|state| {
                            state.input.push(ch);
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_approval_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.approval_bridge.snapshot() else {
            return false;
        };
        let outcome = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                Some(agent::runtime::ToolApprovalOutcome::Approve)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                Some(agent::runtime::ToolApprovalOutcome::Deny {
                    reason: Some("user denied tool call".to_string()),
                })
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(agent::runtime::ToolApprovalOutcome::Deny {
                    reason: Some("user cancelled tool approval".to_string()),
                })
            }
            _ => None,
        };
        if let Some(outcome) = outcome {
            let approved = matches!(outcome, agent::runtime::ToolApprovalOutcome::Approve);
            if self.approval_bridge.respond(outcome) {
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

    async fn maybe_finish_turn(&mut self) -> Result<()> {
        let finished = self
            .turn_task
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if !finished {
            return Ok(());
        }
        let git = state::git_snapshot(self.session.workspace_root());
        if let Some(task) = self.turn_task.take() {
            match task.await {
                Ok(Ok(())) => {
                    let stored_run_count = self.session.refresh_stored_run_count().await.ok();
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.session.git = git.clone();
                        if let Some(stored_run_count) = stored_run_count {
                            state.session.stored_run_count = stored_run_count;
                        }
                    });
                }
                Ok(Err(error)) => {
                    let message = error.to_string();
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.session.git = git.clone();
                        state.status = format!("Error: {message}");
                        state.push_transcript(format!("error> {message}"));
                        state.push_activity(format!(
                            "turn failed: {}",
                            state::preview_text(&message, 56)
                        ));
                    });
                }
                Err(error) => {
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.session.git = git.clone();
                        state.status = format!("Task join error: {error}");
                        state.push_activity(format!("task join error: {error}"));
                    });
                }
            }
        }
        self.sync_queue_depth().await;
        if self.turn_task.is_none() {
            if let Some(queued) = self.command_queue.pop_next() {
                self.sync_queue_depth().await;
                self.start_command(queued.command).await;
            }
        }
        Ok(())
    }

    async fn start_turn(&mut self, prompt: String) {
        if self.turn_task.is_some() {
            let queued = self.command_queue.push_prompt(prompt.clone()).await;
            let depth = self.command_queue.len().await;
            self.ui_state.mutate(|state| {
                state.session.queued_commands = depth;
                state.status = "Queued prompt behind the active turn".to_string();
                state.push_activity(format!(
                    "queued prompt {}: {}",
                    queued.id,
                    state::preview_text(&prompt, 40)
                ));
            });
            return;
        }

        self.start_command(RuntimeCommand::Prompt { prompt }).await;
    }

    async fn start_command(&mut self, command: RuntimeCommand) {
        let preview = queued_command_preview(&command);
        self.ui_state.mutate(|state| {
            state.turn_running = true;
            state.status = match &command {
                RuntimeCommand::Prompt { .. } => "Running prompt".to_string(),
                RuntimeCommand::Steer { .. } => "Applying steer".to_string(),
            };
            state.push_activity(preview.clone());
        });

        let session = self.session.clone();
        let ui_state = self.ui_state.clone();
        self.turn_task = Some(spawn_local(async move {
            let mut observer = SharedRenderObserver::new(ui_state.clone());
            session
                .apply_control_with_observer(command, &mut observer)
                .await
        }));
    }

    async fn apply_command(&mut self, input: &str) -> Result<bool> {
        match parse_slash_command(input) {
            SlashCommand::Quit => Ok(true),
            SlashCommand::Status => {
                self.ui_state.mutate(|state| {
                    state.inspector_title = "Guide".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = build_startup_inspector(&state.session);
                    state.status = "Restored session overview".to_string();
                    state.push_activity("restored session overview");
                });
                Ok(false)
            }
            SlashCommand::Help => {
                self.ui_state.mutate(|state| {
                    state.inspector_title = "Command Palette".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = vec![
                        "Slash commands".to_string(),
                        "  /status".to_string(),
                        "  /help".to_string(),
                        "  /runs [query]".to_string(),
                        "  /run <id-prefix>".to_string(),
                        "  /export_run <id-prefix> <path>".to_string(),
                        "  /export_transcript <id-prefix> <path>".to_string(),
                        "  /tools".to_string(),
                        "  /skills".to_string(),
                        "  /steer <notes>".to_string(),
                        "  /compact [notes]".to_string(),
                        "  /clear".to_string(),
                        "  /quit".to_string(),
                    ];
                    state.status = "Opened command palette".to_string();
                    state.push_activity("opened command palette");
                });
                Ok(false)
            }
            SlashCommand::Tools => {
                let tool_names = self.session.startup_snapshot().tool_names;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Tool Catalog".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = tool_names
                        .iter()
                        .map(|tool| format!("tool: {tool}"))
                        .collect();
                    state.status = "Listed core tools".to_string();
                    state.push_activity("inspected tool catalog");
                });
                Ok(false)
            }
            SlashCommand::Skills => {
                let skills = self.session.skills().to_vec();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Skill Catalog".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if skills.is_empty() {
                        vec!["No skills are available in the configured roots.".to_string()]
                    } else {
                        skills
                            .iter()
                            .map(|skill| {
                                format!(
                                    "{}: {}",
                                    skill.name,
                                    state::preview_text(&skill.description, 72)
                                )
                            })
                            .collect()
                    };
                    state.status = "Listed available skills".to_string();
                    state.push_activity("inspected skill catalog");
                });
                Ok(false)
            }
            SlashCommand::Steer { message } => {
                let Some(message) = message.map(ToOwned::to_owned) else {
                    self.ui_state.mutate(|state| {
                        state.status = "Usage: /steer <notes>".to_string();
                        state.push_activity("invalid /steer invocation");
                    });
                    return Ok(false);
                };
                if self.turn_task.is_some() {
                    let queued = self
                        .command_queue
                        .push_steer(message.clone(), Some("queued_command".to_string()))
                        .await;
                    let depth = self.command_queue.len().await;
                    self.ui_state.mutate(|state| {
                        state.session.queued_commands = depth;
                        state.status = "Queued steer behind the active turn".to_string();
                        state.push_activity(format!(
                            "queued steer {}: {}",
                            queued.id,
                            state::preview_text(&message, 40)
                        ));
                    });
                    return Ok(false);
                }
                self.session
                    .steer(message.clone(), Some("manual_command".to_string()))
                    .await?;
                self.ui_state.mutate(|state| {
                    state.push_transcript(format!("system> {message}"));
                    state.status = "Applied steer".to_string();
                    state.push_activity(format!("steer: {}", state::preview_text(&message, 48)));
                });
                Ok(false)
            }
            SlashCommand::Clear => {
                let mut startup = self.startup_state();
                startup.session.queued_commands = self.command_queue.len().await;
                startup.status = "Cleared conversation pane".to_string();
                startup.push_activity("cleared transcript");
                self.ui_state.replace(startup);
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
                let notes = notes.map(ToOwned::to_owned);
                let compacted = self.session.compact_now(notes).await?;
                self.ui_state.mutate(|state| {
                    if compacted {
                        state.status = "Compacted visible history".to_string();
                        state.push_activity("compacted visible history");
                    } else {
                        state.status = "Compaction skipped".to_string();
                        state.push_activity("compaction skipped");
                    }
                });
                Ok(false)
            }
            command @ (SlashCommand::Runs { .. }
            | SlashCommand::Run { .. }
            | SlashCommand::ExportRun { .. }
            | SlashCommand::ExportTranscript { .. }) => self.apply_history_command(command).await,
            SlashCommand::InvalidUsage(message) => {
                self.ui_state.mutate(|state| {
                    state.status = message.to_string();
                    state.push_activity(format!("invalid command: {message}"));
                });
                Ok(false)
            }
            SlashCommand::Unknown(input) => {
                let input = input.to_string();
                self.ui_state.mutate(move |state| {
                    state.status = format!("Unknown command: {input}");
                    state.push_activity(format!("unknown command: {input}"));
                });
                Ok(false)
            }
        }
    }

    async fn apply_history_command(&mut self, command: SlashCommand<'_>) -> Result<bool> {
        match command {
            SlashCommand::Runs { query } => {
                if let Some(query) = query {
                    let query = query.to_string();
                    let matches = self.session.search_runs(&query).await?;
                    let stored_run_count = self.session.refresh_stored_run_count().await.ok();
                    self.ui_state.mutate(move |state| {
                        if let Some(stored_run_count) = stored_run_count {
                            state.session.stored_run_count = stored_run_count;
                        }
                        state.inspector_title = "Run Search".to_string();
                        state.inspector_scroll = 0;
                        state.inspector = if matches.is_empty() {
                            vec![format!("no runs matched `{query}`")]
                        } else {
                            matches
                                .iter()
                                .take(12)
                                .map(format_run_search_line)
                                .collect()
                        };
                        state.status = if matches.is_empty() {
                            format!("No runs matched `{query}`")
                        } else {
                            format!(
                                "Found {} matching runs. Use /run <id-prefix> to replay one.",
                                matches.len()
                            )
                        };
                        state.push_activity(format!(
                            "searched runs: {}",
                            state::preview_text(&query, 40)
                        ));
                    });
                } else {
                    let runs = self.session.list_runs().await?;
                    let stored_run_count = runs.len();
                    self.ui_state.mutate(move |state| {
                        state.session.stored_run_count = stored_run_count;
                        state.inspector_title = "Runs".to_string();
                        state.inspector_scroll = 0;
                        state.inspector = if runs.is_empty() {
                            vec!["no runs recorded yet".to_string()]
                        } else {
                            runs.iter().take(12).map(format_run_summary_line).collect()
                        };
                        state.status = if runs.is_empty() {
                            "No runs available yet".to_string()
                        } else {
                            format!(
                                "Listed {} runs. Use /run <id-prefix> to replay one.",
                                runs.len()
                            )
                        };
                        state.push_activity("listed persisted runs");
                    });
                }
                Ok(false)
            }
            SlashCommand::Run { run_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before replaying another run".to_string();
                        state.push_activity("run replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded = self.session.load_run(run_ref).await?;
                let inspector = format_run_inspector(&loaded);
                let transcript = format_transcript_lines(&loaded);
                let run_id_preview = preview_id(loaded.summary.run_id.as_str());
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Run".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.status = format!(
                        "Loaded run {} with {} transcript messages",
                        run_id_preview, transcript_count
                    );
                    state.push_activity(format!("loaded run {}", run_id_preview));
                });
                Ok(false)
            }
            SlashCommand::ExportRun { run_ref, path } => {
                let export = self.session.export_run_events(run_ref, path).await?;
                let inspector = format_export_result(&export);
                let run_id_preview = preview_id(export.run_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Export".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.status = format!("Exported run {} to {}", run_id_preview, output_path);
                    state.push_activity(format!("exported run {}", run_id_preview));
                });
                Ok(false)
            }
            SlashCommand::ExportTranscript { run_ref, path } => {
                let export = self.session.export_run_transcript(run_ref, path).await?;
                let inspector = format_export_result(&export);
                let run_id_preview = preview_id(export.run_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Export".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.status =
                        format!("Exported transcript {} to {}", run_id_preview, output_path);
                    state.push_activity(format!("exported transcript {}", run_id_preview));
                });
                Ok(false)
            }
            _ => unreachable!("history handler received non-history command"),
        }
    }

    fn startup_state(&self) -> TuiState {
        let snapshot = self.session.startup_snapshot();
        let workspace_root = snapshot.workspace_root.clone();

        let mut state = TuiState {
            session: state::SessionSummary {
                workspace_name: snapshot.workspace_name.clone(),
                provider_label: snapshot.provider_label.clone(),
                model: snapshot.model.clone(),
                summary_model: snapshot.summary_model.clone(),
                memory_model: snapshot.memory_model.clone(),
                workspace_root: workspace_root.clone(),
                git: state::git_snapshot(&workspace_root),
                tool_names: snapshot.tool_names.clone(),
                skill_names: snapshot.skill_names.clone(),
                store_label: snapshot.store_label.clone(),
                store_warning: snapshot.store_warning.clone(),
                stored_run_count: snapshot.stored_run_count,
                sandbox_summary: snapshot.sandbox_summary.clone(),
                queued_commands: 0,
                token_ledger: Default::default(),
            },
            inspector_title: "Guide".to_string(),
            inspector: build_startup_inspector(&state::SessionSummary {
                workspace_name: snapshot.workspace_name.clone(),
                provider_label: snapshot.provider_label.clone(),
                model: snapshot.model.clone(),
                summary_model: snapshot.summary_model.clone(),
                memory_model: snapshot.memory_model.clone(),
                workspace_root,
                git: Default::default(),
                tool_names: snapshot.tool_names.clone(),
                skill_names: snapshot.skill_names.clone(),
                store_label: snapshot.store_label.clone(),
                store_warning: snapshot.store_warning.clone(),
                stored_run_count: snapshot.stored_run_count,
                sandbox_summary: snapshot.sandbox_summary.clone(),
                queued_commands: 0,
                token_ledger: Default::default(),
            }),
            status: "Ready for your next instruction".to_string(),
            ..TuiState::default()
        };
        state.push_activity("session ready");
        state
    }

    async fn sync_queue_depth(&self) {
        let depth = self.command_queue.len().await;
        self.ui_state
            .mutate(|state| state.session.queued_commands = depth);
    }
}

fn queued_command_preview(command: &RuntimeCommand) -> String {
    match command {
        RuntimeCommand::Prompt { prompt } => {
            format!("running prompt: {}", state::preview_text(prompt, 40))
        }
        RuntimeCommand::Steer { message, .. } => {
            format!("applying steer: {}", state::preview_text(message, 40))
        }
    }
}

fn build_startup_inspector(session: &state::SessionSummary) -> Vec<String> {
    let mut lines = vec![
        "Ask for repo inspection, edits, tests, or debugging.".to_string(),
        "Use /status, /runs, /run, /export_run, /export_transcript, /tools, /skills,".to_string(),
        "/steer, or /compact.".to_string(),
        "Approvals stay in-line above the composer instead of replacing the screen.".to_string(),
        format!(
            "Primary lane: {} / {}",
            session.provider_label, session.model
        ),
        format!("Summary lane: {}", session.summary_model),
        format!("Memory lane: {}", session.memory_model),
        format!(
            "Store: {} ({} runs)",
            session.store_label, session.stored_run_count
        ),
        format!("Sandbox: {}", session.sandbox_summary),
    ];
    if let Some(warning) = &session.store_warning {
        lines.push(format!(
            "Store warning: {}",
            state::preview_text(warning, 72)
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::build_startup_inspector;
    use super::state::SessionSummary;
    use std::path::PathBuf;

    #[test]
    fn startup_inspector_surfaces_backend_boot_snapshot() {
        let lines = build_startup_inspector(&SessionSummary {
            workspace_name: "nanoclaw".to_string(),
            provider_label: "openai".to_string(),
            model: "gpt-5.4".to_string(),
            summary_model: "gpt-5.4-mini".to_string(),
            memory_model: "gpt-5.4-nano".to_string(),
            workspace_root: PathBuf::from("/workspace"),
            git: Default::default(),
            tool_names: vec!["read".to_string(), "write".to_string()],
            skill_names: vec!["rust".to_string()],
            store_label: "file /workspace/.nanoclaw/store".to_string(),
            store_warning: Some("falling back soon".to_string()),
            stored_run_count: 12,
            sandbox_summary: "enforced via seatbelt".to_string(),
            queued_commands: 0,
            token_ledger: Default::default(),
        });

        assert!(
            lines
                .iter()
                .any(|line| line == "Store: file /workspace/.nanoclaw/store (12 runs)")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "Sandbox: enforced via seatbelt")
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Store warning: falling back soon"))
        );
    }
}
