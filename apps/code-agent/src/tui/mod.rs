mod approval;
mod observer;
mod render;
mod state;

pub(crate) use approval::{ApprovalBridge, InteractiveToolApprovalHandler};
use observer::SharedRenderObserver;
use render::render;
pub(crate) use state::SharedUiState;
use state::TuiState;

use agent::{AgentRuntime, RuntimeCommand, RuntimeCommandQueue, Skill};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
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
    runtime: Arc<AsyncMutex<AgentRuntime>>,
    workspace_root: PathBuf,
    provider_label: String,
    model: String,
    tool_names: Vec<String>,
    skills: Vec<Skill>,
    initial_prompt: Option<String>,
    ui_state: SharedUiState,
    approval_bridge: ApprovalBridge,
    command_queue: RuntimeCommandQueue,
    turn_task: Option<JoinHandle<Result<()>>>,
}

impl CodeAgentTui {
    pub fn new(
        runtime: AgentRuntime,
        workspace_root: PathBuf,
        provider_label: String,
        model: String,
        skills: Vec<Skill>,
        initial_prompt: Option<String>,
        ui_state: SharedUiState,
        approval_bridge: ApprovalBridge,
    ) -> Self {
        let tool_names = runtime.tool_registry_names();
        Self {
            runtime: Arc::new(AsyncMutex::new(runtime)),
            workspace_root,
            provider_label,
            model,
            tool_names,
            skills,
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
        {
            let mut runtime = self.runtime.lock().await;
            let _ = runtime.end_session(Some("operator_exit".to_string())).await;
        }

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
        let git = state::git_snapshot(&self.workspace_root);
        if let Some(task) = self.turn_task.take() {
            match task.await {
                Ok(Ok(())) => {
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.session.git = git.clone();
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

        let runtime = self.runtime.clone();
        let ui_state = self.ui_state.clone();
        self.turn_task = Some(spawn_local(async move {
            let mut observer = SharedRenderObserver::new(ui_state.clone());
            let mut runtime = runtime.lock().await;
            runtime
                .apply_control_with_observer(command, &mut observer)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        }));
    }

    async fn apply_command(&mut self, input: &str) -> Result<bool> {
        match input.trim() {
            "/quit" | "/exit" => Ok(true),
            "/help" => {
                self.ui_state.mutate(|state| {
                    state.inspector_title = "Command Palette".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = vec![
                        "Slash commands".to_string(),
                        "  /help".to_string(),
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
            "/tools" => {
                let tool_names = self.tool_names.clone();
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
            "/skills" => {
                let skills = self.skills.clone();
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
            _ if input.trim().starts_with("/steer") => {
                let notes = input
                    .trim()
                    .strip_prefix("/steer")
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                let Some(message) = notes else {
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
                {
                    let mut runtime = self.runtime.lock().await;
                    runtime
                        .steer(message.clone(), Some("manual_command".to_string()))
                        .await?;
                }
                self.ui_state.mutate(|state| {
                    state.push_transcript(format!("system> {message}"));
                    state.status = "Applied steer".to_string();
                    state.push_activity(format!("steer: {}", state::preview_text(&message, 48)));
                });
                Ok(false)
            }
            "/clear" => {
                let mut startup = self.startup_state();
                startup.session.queued_commands = self.command_queue.len().await;
                startup.status = "Cleared conversation pane".to_string();
                startup.push_activity("cleared transcript");
                self.ui_state.replace(startup);
                Ok(false)
            }
            _ if input.trim().starts_with("/compact") => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status = "Wait for the current turn before compacting".to_string();
                        state.push_activity("compact blocked while turn running");
                    });
                    return Ok(false);
                }
                let notes = input
                    .trim()
                    .strip_prefix("/compact")
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                let compacted = {
                    let mut runtime = self.runtime.lock().await;
                    runtime.compact_now(notes).await?
                };
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
            _ => {
                let input = input.to_string();
                self.ui_state.mutate(move |state| {
                    state.status = format!("Unknown command: {input}");
                    state.push_activity(format!("unknown command: {input}"));
                });
                Ok(false)
            }
        }
    }

    fn startup_state(&self) -> TuiState {
        let skill_names = if self.skills.is_empty() {
            Vec::new()
        } else {
            self.skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect::<Vec<_>>()
        };
        let workspace_name = self
            .workspace_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace")
            .to_string();

        let mut state = TuiState {
            session: state::SessionSummary {
                workspace_name,
                provider_label: self.provider_label.clone(),
                model: self.model.clone(),
                workspace_root: self.workspace_root.clone(),
                git: state::git_snapshot(&self.workspace_root),
                tool_names: self.tool_names.clone(),
                skill_names,
                queued_commands: 0,
            },
            inspector_title: "Guide".to_string(),
            inspector: vec![
                "Ask for repo inspection, edits, tests, or debugging.".to_string(),
                "Use /help, /tools, /skills, /steer, or /compact from the composer.".to_string(),
                "Approvals stay in-line above the composer instead of replacing the screen."
                    .to_string(),
            ],
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
