mod approval;
mod commands;
mod history;
mod observer;
mod render;
mod state;

use crate::backend::{
    CodeAgentSession, LiveTaskControlAction, LiveTaskMessageAction, LiveTaskWaitOutcome,
    SessionOperation, SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot,
    preview_id,
};
use approval::approval_decision_for_key;
use commands::{SlashCommand, parse_slash_command};
use history::{
    format_agent_session_inspector, format_agent_session_summary_line,
    format_live_task_control_outcome, format_live_task_message_outcome,
    format_live_task_spawn_outcome, format_live_task_summary_line, format_live_task_wait_outcome,
    format_mcp_prompt_summary_line, format_mcp_resource_summary_line,
    format_mcp_server_summary_line, format_session_export_result, format_session_inspector,
    format_session_operation_outcome, format_session_search_line, format_session_summary_line,
    format_session_transcript_lines, format_startup_diagnostics, format_task_inspector,
    format_task_summary_line, format_visible_transcript_lines,
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
use tokio::task::{JoinHandle, spawn_local};
use tokio::time::{Duration, sleep};

pub struct CodeAgentTui {
    session: CodeAgentSession,
    initial_prompt: Option<String>,
    ui_state: SharedUiState,
    event_renderer: SharedRenderObserver,
    command_queue: RuntimeCommandQueue,
    turn_task: Option<JoinHandle<Result<()>>>,
    operator_task: Option<JoinHandle<Result<OperatorTaskOutcome>>>,
}

enum OperatorTaskOutcome {
    WaitLiveTask(LiveTaskWaitOutcome),
}

impl CodeAgentTui {
    pub fn new(
        session: CodeAgentSession,
        initial_prompt: Option<String>,
        ui_state: SharedUiState,
    ) -> Self {
        Self {
            session,
            initial_prompt,
            event_renderer: SharedRenderObserver::new(ui_state.clone()),
            ui_state,
            command_queue: RuntimeCommandQueue::new(),
            turn_task: None,
            operator_task: None,
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
        if let Some(task) = self.operator_task.take() {
            task.abort();
        }
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
            self.apply_backend_events();
            self.maybe_finish_operator_task().await?;

            let snapshot = self.ui_state.snapshot();
            let approval = self.session.approval_prompt();
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
        let Some(prompt) = self.session.approval_prompt() else {
            return false;
        };
        if let Some(decision) = approval_decision_for_key(key) {
            let approved = matches!(decision, crate::backend::ApprovalDecision::Approve);
            if self.session.resolve_approval(decision) {
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
                    let stored_session_count =
                        self.session.refresh_stored_session_count().await.ok();
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.session.git = git.clone();
                        if let Some(stored_session_count) = stored_session_count {
                            state.session.stored_session_count = stored_session_count;
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

    async fn maybe_finish_operator_task(&mut self) -> Result<()> {
        let finished = self
            .operator_task
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if !finished {
            return Ok(());
        }
        if let Some(task) = self.operator_task.take() {
            match task.await {
                Ok(Ok(OperatorTaskOutcome::WaitLiveTask(outcome))) => {
                    let inspector = format_live_task_wait_outcome(&outcome);
                    self.ui_state.mutate(move |state| {
                        state.inspector_title = "Live Task Wait".to_string();
                        state.inspector_scroll = 0;
                        state.inspector = inspector;
                        state.status = format!(
                            "Live task {} finished with status {}",
                            outcome.task_id, outcome.status
                        );
                        state.push_activity(format!(
                            "wait completed for {} ({})",
                            outcome.task_id, outcome.status
                        ));
                    });
                }
                Ok(Err(error)) => {
                    let message = error.to_string();
                    self.ui_state.mutate(|state| {
                        state.status = format!("Operator task failed: {message}");
                        state.inspector_title = "Operator Error".to_string();
                        state.inspector_scroll = 0;
                        state.inspector = vec!["## Operator Error".to_string(), message.clone()];
                        state.push_activity(format!(
                            "operator task failed: {}",
                            state::preview_text(&message, 56)
                        ));
                    });
                }
                Err(error) => {
                    self.ui_state.mutate(|state| {
                        state.status = format!("Operator task join error: {error}");
                        state.push_activity(format!("operator task join error: {error}"));
                    });
                }
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
        self.turn_task = Some(spawn_local(
            async move { session.apply_control(command).await },
        ));
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
                    state.inspector = command_palette_lines();
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
                    state.inspector = if tool_names.is_empty() {
                        vec!["## Tools".to_string(), "No tools registered.".to_string()]
                    } else {
                        std::iter::once("## Tools".to_string())
                            .chain(tool_names.iter().map(|tool| format!("tool: {tool}")))
                            .collect()
                    };
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
                        vec![
                            "## Skills".to_string(),
                            "No skills are available in the configured roots.".to_string(),
                        ]
                    } else {
                        std::iter::once("## Skills".to_string())
                            .chain(skills.iter().map(|skill| {
                                format!(
                                    "{}: {}",
                                    skill.name,
                                    state::preview_text(&skill.description, 72)
                                )
                            }))
                            .collect()
                    };
                    state.status = "Listed available skills".to_string();
                    state.push_activity("inspected skill catalog");
                });
                Ok(false)
            }
            SlashCommand::Diagnostics => {
                let diagnostics = self.session.startup_diagnostics();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Diagnostics".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = format_startup_diagnostics(&diagnostics);
                    state.status = "Opened startup diagnostics".to_string();
                    state.push_activity("inspected startup diagnostics");
                });
                Ok(false)
            }
            SlashCommand::Mcp => {
                let servers = self.session.list_mcp_servers().await;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "MCP".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if servers.is_empty() {
                        vec![
                            "## MCP".to_string(),
                            "No MCP servers connected.".to_string(),
                        ]
                    } else {
                        std::iter::once("## MCP".to_string())
                            .chain(servers.iter().map(format_mcp_server_summary_line))
                            .collect()
                    };
                    state.status = "Listed MCP servers".to_string();
                    state.push_activity("listed mcp servers");
                });
                Ok(false)
            }
            SlashCommand::Prompts => {
                let prompts = self.session.list_mcp_prompts().await;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Prompts".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if prompts.is_empty() {
                        vec![
                            "## MCP Prompts".to_string(),
                            "No MCP prompts available.".to_string(),
                        ]
                    } else {
                        std::iter::once("## MCP Prompts".to_string())
                            .chain(prompts.iter().map(format_mcp_prompt_summary_line))
                            .collect()
                    };
                    state.status = "Listed MCP prompts".to_string();
                    state.push_activity("listed mcp prompts");
                });
                Ok(false)
            }
            SlashCommand::Resources => {
                let resources = self.session.list_mcp_resources().await;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Resources".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if resources.is_empty() {
                        vec![
                            "## MCP Resources".to_string(),
                            "No MCP resources available.".to_string(),
                        ]
                    } else {
                        std::iter::once("## MCP Resources".to_string())
                            .chain(resources.iter().map(format_mcp_resource_summary_line))
                            .collect()
                    };
                    state.status = "Listed MCP resources".to_string();
                    state.push_activity("listed mcp resources");
                });
                Ok(false)
            }
            SlashCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                let loaded = self
                    .session
                    .load_mcp_prompt(&server_name, &prompt_name)
                    .await?;
                self.ui_state.mutate(move |state| {
                    state.input = loaded.input_text;
                    state.inspector_title = "Prompt".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = loaded.inspector_lines;
                    state.status =
                        format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
                    state.push_activity(format!("loaded mcp prompt {server_name}/{prompt_name}"));
                });
                Ok(false)
            }
            SlashCommand::Resource { server_name, uri } => {
                let loaded = self.session.load_mcp_resource(&server_name, &uri).await?;
                self.ui_state.mutate(move |state| {
                    state.input = loaded.input_text;
                    state.inspector_title = "Resource".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = loaded.inspector_lines;
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
                self.start_command(RuntimeCommand::Steer {
                    message,
                    reason: Some("manual_command".to_string()),
                })
                .await;
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

                let dropped_commands = self.command_queue.clear();
                let outcome = self
                    .session
                    .apply_session_operation(SessionOperation::StartFresh)
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
                let compacted = self.session.compact_now(notes).await?;
                self.apply_backend_events();
                if !compacted {
                    self.ui_state.mutate(|state| {
                        state.status = "Compaction skipped".to_string();
                        state.push_activity("compaction skipped");
                    });
                }
                Ok(false)
            }
            SlashCommand::LiveTasks => {
                let live_tasks = self.session.list_live_tasks().await?;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Live Tasks".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if live_tasks.is_empty() {
                        vec![
                            "## Live Tasks".to_string(),
                            "no live child tasks attached to the active root agent".to_string(),
                        ]
                    } else {
                        std::iter::once("## Live Tasks".to_string())
                            .chain(live_tasks.iter().map(format_live_task_summary_line))
                            .collect()
                    };
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
                let outcome = self.session.spawn_live_task(&role, &prompt).await?;
                let inspector = format_live_task_spawn_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Live Task Spawn".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
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
                let outcome = self
                    .session
                    .send_live_task(&task_or_agent_ref, &message)
                    .await?;
                let inspector = format_live_task_message_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Live Task Message".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
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
                let outcome = self
                    .session
                    .cancel_live_task(&task_or_agent_ref, reason.clone())
                    .await?;
                let inspector = format_live_task_control_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Live Task Control".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
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
                self.ui_state.mutate(|state| {
                    state.status = "Command syntax error".to_string();
                    state.inspector_title = "Command Error".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = message.lines().map(ToOwned::to_owned).collect();
                    state.push_activity("command parse error");
                });
                Ok(false)
            }
        }
    }

    fn start_wait_task(&mut self, task_or_agent_ref: String) {
        let wait_ref = task_or_agent_ref.clone();
        self.ui_state.mutate(|state| {
            state.status = format!("Waiting for live task {}", preview_id(&wait_ref));
            state.push_activity(format!("waiting for live task {}", preview_id(&wait_ref)));
        });
        let session = self.session.clone();
        self.operator_task = Some(spawn_local(async move {
            let outcome = session.wait_live_task(&task_or_agent_ref).await?;
            Ok(OperatorTaskOutcome::WaitLiveTask(outcome))
        }));
    }

    async fn apply_history_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::AgentSessions { session_ref } => {
                let agent_sessions = self
                    .session
                    .list_agent_sessions(session_ref.as_deref())
                    .await?;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Agent Sessions".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if agent_sessions.is_empty() {
                        vec![
                            "## Agent Sessions".to_string(),
                            "no persisted agent sessions recorded yet".to_string(),
                        ]
                    } else {
                        std::iter::once("## Agent Sessions".to_string())
                            .chain(
                                agent_sessions
                                    .iter()
                                    .take(16)
                                    .map(format_agent_session_summary_line),
                            )
                            .collect()
                    };
                    state.status = if agent_sessions.is_empty() {
                        "No agent sessions available yet".to_string()
                    } else {
                        format!(
                            "Listed {} agent sessions. Use /agent_session <agent-session-ref> to open one.",
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
                let loaded = self.session.load_agent_session(&agent_session_ref).await?;
                let inspector = format_agent_session_inspector(&loaded);
                let transcript = format_visible_transcript_lines(&loaded.transcript);
                let agent_session_ref_preview = preview_id(&loaded.summary.agent_session_ref);
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Agent Session".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
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
                let tasks = self.session.list_tasks(session_ref.as_deref()).await?;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Tasks".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = if tasks.is_empty() {
                        vec![
                            "## Tasks".to_string(),
                            "no persisted tasks recorded yet".to_string(),
                        ]
                    } else {
                        std::iter::once("## Tasks".to_string())
                            .chain(tasks.iter().take(16).map(format_task_summary_line))
                            .collect()
                    };
                    state.status = if tasks.is_empty() {
                        "No tasks available yet".to_string()
                    } else {
                        format!(
                            "Listed {} tasks. Use /task <task-id> to open one.",
                            tasks.len()
                        )
                    };
                    state.push_activity("listed persisted tasks");
                });
                Ok(false)
            }
            SlashCommand::Sessions { query } => {
                if let Some(query) = query {
                    let matches = self.session.search_sessions(&query).await?;
                    let stored_session_count =
                        self.session.refresh_stored_session_count().await.ok();
                    self.ui_state.mutate(move |state| {
                        if let Some(stored_session_count) = stored_session_count {
                            state.session.stored_session_count = stored_session_count;
                        }
                        state.inspector_title = "Session Search".to_string();
                        state.inspector_scroll = 0;
                        state.inspector = if matches.is_empty() {
                            vec![
                                "## Session Search".to_string(),
                                format!("no sessions matched `{query}`"),
                            ]
                        } else {
                            std::iter::once("## Session Search".to_string())
                                .chain(matches.iter().take(12).map(format_session_search_line))
                                .collect()
                        };
                        state.status = if matches.is_empty() {
                            format!("No sessions matched `{query}`")
                        } else {
                            format!(
                                "Found {} matching sessions. Use /session <session-ref> to open one.",
                                matches.len()
                            )
                        };
                        state.push_activity(format!(
                            "searched sessions: {}",
                            state::preview_text(&query, 40)
                        ));
                    });
                } else {
                    let sessions = self.session.list_sessions().await?;
                    let stored_session_count = sessions.len();
                    self.ui_state.mutate(move |state| {
                        state.session.stored_session_count = stored_session_count;
                        state.inspector_title = "Sessions".to_string();
                        state.inspector_scroll = 0;
                        state.inspector = if sessions.is_empty() {
                            vec![
                                "## Sessions".to_string(),
                                "no persisted sessions recorded yet".to_string(),
                            ]
                        } else {
                            std::iter::once("## Sessions".to_string())
                                .chain(sessions.iter().take(12).map(format_session_summary_line))
                                .collect()
                        };
                        state.status = if sessions.is_empty() {
                            "No sessions available yet".to_string()
                        } else {
                            format!(
                                "Listed {} sessions. Use /session <session-ref> to open one.",
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
                let loaded = self.session.load_session(&session_ref).await?;
                let inspector = format_session_inspector(&loaded);
                let transcript = format_session_transcript_lines(&loaded);
                let session_ref_preview = preview_id(loaded.summary.session_id.as_str());
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Session".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
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
                let loaded = self.session.load_task(&task_ref).await?;
                let inspector = format_task_inspector(&loaded);
                let transcript = format_visible_transcript_lines(&loaded.child_transcript);
                let task_id = loaded.summary.task_id.clone();
                let transcript_count = loaded.child_transcript.len();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Task".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
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
                let outcome = self
                    .session
                    .apply_session_operation(SessionOperation::ResumeAgentSession {
                        agent_session_ref,
                    })
                    .await?;
                self.replace_after_session_operation(outcome, 0);
                Ok(false)
            }
            SlashCommand::ExportSession { session_ref, path } => {
                let export = self.session.export_session(&session_ref, &path).await?;
                let inspector = format_session_export_result(&export);
                let session_ref_preview = preview_id(export.session_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Export".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.status = format!(
                        "Exported session {} to {}",
                        session_ref_preview, output_path
                    );
                    state.push_activity(format!("exported session {}", session_ref_preview));
                });
                Ok(false)
            }
            SlashCommand::ExportTranscript { session_ref, path } => {
                let export = self
                    .session
                    .export_session_transcript(&session_ref, &path)
                    .await?;
                let inspector = format_session_export_result(&export);
                let session_ref_preview = preview_id(export.session_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.inspector_title = "Export".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
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

    fn startup_state(&self) -> TuiState {
        self.startup_state_from_snapshot(&self.session.startup_snapshot())
    }

    fn startup_state_from_snapshot(&self, snapshot: &SessionStartupSnapshot) -> TuiState {
        let workspace_root = snapshot.workspace_root.clone();
        let mut state = TuiState {
            session: state::SessionSummary {
                workspace_name: snapshot.workspace_name.clone(),
                active_session_ref: snapshot.active_session_ref.clone(),
                root_agent_session_id: snapshot.root_agent_session_id.clone(),
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
                stored_session_count: snapshot.stored_session_count,
                sandbox_summary: snapshot.sandbox_summary.clone(),
                startup_diagnostics: snapshot.startup_diagnostics.clone(),
                queued_commands: 0,
                token_ledger: Default::default(),
            },
            inspector_title: "Guide".to_string(),
            inspector: build_startup_inspector(&state::SessionSummary {
                workspace_name: snapshot.workspace_name.clone(),
                active_session_ref: snapshot.active_session_ref.clone(),
                root_agent_session_id: snapshot.root_agent_session_id.clone(),
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
                stored_session_count: snapshot.stored_session_count,
                sandbox_summary: snapshot.sandbox_summary.clone(),
                startup_diagnostics: snapshot.startup_diagnostics.clone(),
                queued_commands: 0,
                token_ledger: Default::default(),
            }),
            status: "Ready for your next instruction".to_string(),
            ..TuiState::default()
        };
        state.push_activity("session ready");
        state
    }

    fn replace_after_session_operation(
        &mut self,
        outcome: SessionOperationOutcome,
        dropped_commands: usize,
    ) {
        let aborted_operator_task = self.abort_operator_task();
        let mut startup = self.startup_state_from_snapshot(&outcome.startup);
        startup.session.queued_commands = 0;
        startup.transcript = format_visible_transcript_lines(&outcome.transcript);
        startup.transcript_scroll = 0;

        match outcome.action {
            SessionOperationAction::StartedFresh => {
                startup.status = "Started new session".to_string();
                startup.push_activity(format!(
                    "started new session {}",
                    preview_id(&outcome.session_ref)
                ));
            }
            SessionOperationAction::AlreadyAttached => {
                let requested = outcome
                    .requested_agent_session_ref
                    .as_deref()
                    .unwrap_or(outcome.active_agent_session_ref.as_str());
                startup.inspector_title = "Resume".to_string();
                startup.inspector_scroll = 0;
                startup.inspector = format_session_operation_outcome(&outcome);
                startup.status = format!(
                    "Agent session {} is already attached",
                    preview_id(requested)
                );
                startup.push_activity(format!("resume no-op {}", preview_id(requested)));
            }
            SessionOperationAction::Reattached => {
                startup.inspector_title = "Resume".to_string();
                startup.inspector_scroll = 0;
                startup.inspector = format_session_operation_outcome(&outcome);
                startup.status = format!(
                    "Reattached session {} as {}",
                    preview_id(&outcome.session_ref),
                    preview_id(&outcome.active_agent_session_ref)
                );
                startup.push_activity(format!(
                    "resumed session {} as {}",
                    preview_id(&outcome.session_ref),
                    preview_id(&outcome.active_agent_session_ref)
                ));
            }
        }

        if dropped_commands > 0 {
            startup.push_activity(format!("discarded {} queued command(s)", dropped_commands));
        }
        if aborted_operator_task {
            startup.push_activity("aborted pending live-task operator wait after session switch");
        }
        self.ui_state.replace(startup);
    }

    async fn sync_queue_depth(&self) {
        let depth = self.command_queue.len().await;
        self.ui_state
            .mutate(|state| state.session.queued_commands = depth);
    }

    fn apply_backend_events(&mut self) {
        for event in self.session.drain_events() {
            self.event_renderer.apply_event(event);
        }
    }

    fn abort_operator_task(&mut self) -> bool {
        if let Some(task) = self.operator_task.take() {
            task.abort();
            true
        } else {
            false
        }
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

fn command_palette_lines() -> Vec<String> {
    vec![
        "## Commands".to_string(),
        "/status".to_string(),
        "/help".to_string(),
        "/agent_sessions [session-ref]".to_string(),
        "/agent_session <agent-session-ref>".to_string(),
        "/live_tasks".to_string(),
        "/spawn_task <role> <prompt>".to_string(),
        "/send_task <task-or-agent-ref> <message>".to_string(),
        "/wait_task <task-or-agent-ref>".to_string(),
        "/cancel_task <task-or-agent-ref> [reason]".to_string(),
        "/tasks [session-ref]".to_string(),
        "/task <task-id>".to_string(),
        "/sessions [query]".to_string(),
        "/session <session-ref>".to_string(),
        "/resume <agent-session-ref>".to_string(),
        "/export_session <session-ref> <path>".to_string(),
        "/export_transcript <session-ref> <path>".to_string(),
        "/tools".to_string(),
        "/skills".to_string(),
        "/diagnostics".to_string(),
        "/mcp".to_string(),
        "/prompts".to_string(),
        "/resources".to_string(),
        "/prompt <server> <name>".to_string(),
        "/resource <server> <uri>".to_string(),
        "/steer <notes>".to_string(),
        "/new".to_string(),
        "/compact [notes]".to_string(),
        "/clear  (alias of /new)".to_string(),
        "/quit".to_string(),
    ]
}

fn build_startup_inspector(session: &state::SessionSummary) -> Vec<String> {
    let mut lines = vec![
        "## Session".to_string(),
        format!("session ref: {}", session.active_session_ref),
        format!("agent session id: {}", session.root_agent_session_id),
        "## Workflow".to_string(),
        "Use /sessions to browse persisted sessions and /session <ref> to open one.".to_string(),
        "Use /agent_sessions to browse persisted agent sessions, /agent_session <ref> to inspect one, and /resume <agent-session-ref> to reattach one.".to_string(),
        "Use /spawn_task <role> <prompt> to launch a new live child task, /live_tasks to inspect active child agents, /send_task <task-or-agent-ref> <message> to steer one, /wait_task <task-or-agent-ref> to wait for one, and /cancel_task <task-or-agent-ref> to stop one without leaving the current session.".to_string(),
        "Use /tasks to browse persisted child tasks and /task <task-id> to inspect one.".to_string(),
        "Use /new or /clear to start a fresh top-level session without deleting prior history.".to_string(),
        "Use /export_session or /export_transcript to write durable artifacts.".to_string(),
        "Approvals stay in-line above the composer instead of replacing the screen.".to_string(),
        "## Models".to_string(),
        format!(
            "primary lane: {} / {}",
            session.provider_label, session.model
        ),
        format!("summary lane: {}", session.summary_model),
        format!("memory lane: {}", session.memory_model),
        "## Store".to_string(),
        format!(
            "store: {} ({} sessions)",
            session.store_label, session.stored_session_count
        ),
        format!("sandbox: {}", session.sandbox_summary),
        "## Diagnostics".to_string(),
        format!(
            "tools: {} local / {} mcp",
            session.startup_diagnostics.local_tool_count,
            session.startup_diagnostics.mcp_tool_count
        ),
        format!(
            "plugins: {} enabled / {} total",
            session.startup_diagnostics.enabled_plugin_count,
            session.startup_diagnostics.total_plugin_count
        ),
        format!(
            "mcp servers: {}",
            session.startup_diagnostics.mcp_servers.len()
        ),
    ];
    if let Some(warning) = &session.store_warning {
        lines.push(format!(
            "store warning: {}",
            state::preview_text(warning, 72)
        ));
    }
    if !session.startup_diagnostics.warnings.is_empty() {
        lines.push(format!(
            "warnings: {}",
            session.startup_diagnostics.warnings.join(" | ")
        ));
    }
    if !session.startup_diagnostics.diagnostics.is_empty() {
        lines.push(format!(
            "driver diagnostics: {}",
            session.startup_diagnostics.diagnostics.join(" | ")
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
            active_session_ref: "session_123".to_string(),
            root_agent_session_id: "session_123".to_string(),
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
            stored_session_count: 12,
            sandbox_summary: "enforced via seatbelt".to_string(),
            startup_diagnostics: Default::default(),
            queued_commands: 0,
            token_ledger: Default::default(),
        });

        assert!(
            lines
                .iter()
                .any(|line| line == "store: file /workspace/.nanoclaw/store (12 sessions)")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "sandbox: enforced via seatbelt")
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("store warning: falling back soon"))
        );
    }
}
