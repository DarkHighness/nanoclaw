mod approval;
mod commands;
mod observer;
mod presenters;
mod run_history;

use crate::{TuiCommand, config::AgentCoreConfig, parse_command, render};
use agent::mcp::ConnectedMcpServer;
use agent::skills::Skill;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use observer::LiveRenderObserver;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use runtime::{AgentRuntime, RunTurnOutcome};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use store::RunStore;
use types::ToolSpec;

pub use approval::InteractiveToolApprovalHandler;
use presenters::*;

pub struct RuntimeTui {
    runtime: AgentRuntime,
    store: Arc<dyn RunStore>,
    workspace_root: PathBuf,
    command_prefix: String,
    mcp_servers: Vec<ConnectedMcpServer>,
    skills: Vec<Skill>,
    startup_summary: TuiStartupSummary,
}

#[derive(Clone, Debug, Default)]
pub struct TuiState {
    pub input: String,
    pub transcript: Vec<String>,
    pub sidebar: Vec<String>,
    pub sidebar_title: String,
    pub status: String,
}

#[derive(Clone, Debug, Default)]
pub struct TuiStartupSummary {
    pub sidebar_title: String,
    pub sidebar: Vec<String>,
    pub status: String,
}

impl TuiState {
    pub fn transcript_text(&self) -> String {
        self.transcript.join("\n\n")
    }

    pub fn sidebar_text(&self) -> String {
        self.sidebar.join("\n")
    }
}

impl RuntimeTui {
    #[must_use]
    pub fn new(
        runtime: AgentRuntime,
        store: Arc<dyn RunStore>,
        workspace_root: PathBuf,
        config: &AgentCoreConfig,
        mcp_servers: Vec<ConnectedMcpServer>,
        skills: Vec<Skill>,
        startup_summary: TuiStartupSummary,
    ) -> Self {
        Self {
            runtime,
            store,
            workspace_root,
            command_prefix: config.tui.command_prefix.clone(),
            mcp_servers,
            skills,
            startup_summary,
        }
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        let mut state = TuiState {
            sidebar: self.startup_summary.sidebar.clone(),
            sidebar_title: self.startup_summary.sidebar_title.clone(),
            status: self.startup_summary.status.clone(),
            ..TuiState::default()
        };

        let result = self.event_loop(&mut terminal, &mut state).await;

        disable_raw_mode()?;
        crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        state: &mut TuiState,
    ) -> anyhow::Result<()> {
        loop {
            terminal.draw(|frame| render(frame, state))?;
            if !event::poll(std::time::Duration::from_millis(100))? {
                continue;
            }
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        let input = std::mem::take(&mut state.input);
                        if let Some(command) = parse_command(&input, &self.command_prefix) {
                            match self.apply_command(command, state).await {
                                Ok(true) => return Ok(()),
                                Ok(false) => {}
                                Err(error) => state.status = format!("Command error: {error}"),
                            }
                            continue;
                        }
                        if input.trim().is_empty() {
                            continue;
                        }
                        state.status = "Running...".to_string();
                        let mut observer = LiveRenderObserver::new(terminal, state);
                        match self
                            .runtime
                            .run_user_prompt_with_observer(input.clone(), &mut observer)
                            .await
                        {
                            Ok(outcome) => self.apply_outcome(state, outcome).await?,
                            Err(error) => {
                                if let Ok(lines) =
                                    self.replay_run_lines(&self.runtime.run_id()).await
                                {
                                    if !lines.is_empty() {
                                        state.transcript = lines;
                                    }
                                }
                                if state.transcript.is_empty() {
                                    state.transcript.push(format!("user> {input}"));
                                }
                                state.transcript.push(format!("error> {error}"));
                                state.status = format!("Error: {error}");
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        state.input.pop();
                    }
                    KeyCode::Char(ch) => {
                        state.input.push(ch);
                    }
                    _ => {}
                }
            }
        }
    }

    async fn apply_command(
        &mut self,
        command: TuiCommand,
        state: &mut TuiState,
    ) -> anyhow::Result<bool> {
        match command {
            TuiCommand::Quit => Ok(true),
            command @ (TuiCommand::Status | TuiCommand::Clear | TuiCommand::Compact { .. }) => {
                self.apply_session_command(command, state).await
            }
            command @ (TuiCommand::Runs { .. }
            | TuiCommand::Run { .. }
            | TuiCommand::ExportRun { .. }
            | TuiCommand::ExportTranscript { .. }) => self.apply_runs_command(command, state).await,
            command @ (TuiCommand::Skills { .. }
            | TuiCommand::Skill { .. }
            | TuiCommand::Tools
            | TuiCommand::Hooks) => self.apply_catalog_command(command, state).await,
            command @ (TuiCommand::Mcp
            | TuiCommand::Prompts
            | TuiCommand::Resources
            | TuiCommand::Prompt { .. }
            | TuiCommand::Resource { .. }) => self.apply_mcp_command(command, state).await,
        }
    }

    async fn apply_outcome(
        &self,
        state: &mut TuiState,
        outcome: RunTurnOutcome,
    ) -> anyhow::Result<()> {
        state.transcript = self.replay_run_lines(&self.runtime.run_id()).await?;
        let events = self.store.events(&self.runtime.run_id()).await?;
        state.sidebar = build_turn_sidebar(&events);
        state.sidebar_title = "Turn".to_string();
        state.status = format!(
            "Turn complete. Assistant: {}",
            preview_text(&outcome.assistant_text, 64)
        );
        Ok(())
    }

    fn runtime_tools(&self) -> Vec<ToolSpec> {
        self.runtime.tool_specs()
    }

    fn restore_startup_summary(&self, state: &mut TuiState) {
        state.sidebar = self.startup_summary.sidebar.clone();
        state.sidebar_title = self.startup_summary.sidebar_title.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::InteractiveToolApprovalHandler;
    use super::approval::{SessionApprovalDecision, ToolApprovalCacheKey};
    use runtime::{ToolApprovalOutcome, ToolApprovalRequest};
    use serde_json::json;
    use std::collections::BTreeMap;
    use types::{ToolCall, ToolCallId, ToolOrigin, ToolOutputMode, ToolSpec};

    fn sample_request(tool_name: &str, origin: ToolOrigin) -> ToolApprovalRequest {
        ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: "call-1".into(),
                tool_name: tool_name.to_string().into(),
                arguments: json!({"path":"sample.txt"}),
                origin: origin.clone(),
            },
            spec: ToolSpec {
                name: tool_name.to_string().into(),
                description: "sample".to_string(),
                input_schema: json!({"type":"object"}),
                output_mode: ToolOutputMode::Text,
                output_schema: None,
                origin,
                annotations: BTreeMap::new(),
            },
            reasons: vec!["sample reason".to_string()],
        }
    }

    #[test]
    fn session_allow_is_reused_for_same_tool_origin() {
        let handler = InteractiveToolApprovalHandler::default();
        let request = sample_request("bash", ToolOrigin::Local);

        handler.remember_outcome(&request, SessionApprovalDecision::Approve);

        assert_eq!(
            handler.cached_outcome(&request),
            Some(ToolApprovalOutcome::Approve)
        );
        assert_eq!(
            handler.cached_outcome(&sample_request(
                "bash",
                ToolOrigin::Mcp {
                    server_name: "remote".to_string()
                }
            )),
            None
        );
    }

    #[test]
    fn session_deny_returns_cached_denial_reason() {
        let handler = InteractiveToolApprovalHandler::default();
        let request = sample_request(
            "search_web",
            ToolOrigin::Mcp {
                server_name: "remote".to_string(),
            },
        );

        handler.remember_outcome(&request, SessionApprovalDecision::Deny);

        assert_eq!(
            handler.cached_outcome(&request),
            Some(ToolApprovalOutcome::Deny {
                reason: Some("tool denied for the rest of the session".to_string()),
            })
        );
        assert_eq!(
            ToolApprovalCacheKey::from_request(&request),
            ToolApprovalCacheKey {
                tool_name: "search_web".to_string(),
                origin_key: "mcp:remote".to_string(),
            }
        );
    }
}
