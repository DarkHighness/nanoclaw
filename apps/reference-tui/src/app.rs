use crate::{TuiCommand, config::AgentCoreConfig, parse_command, render};
use agent::mcp::{ConnectedMcpServer, McpPrompt, McpResource};
use agent::skills::Skill;
use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{
    cursor::MoveTo,
    execute,
    terminal::{Clear, ClearType},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use runtime::{
    AgentRuntime, Result as RuntimeResult, RunTurnOutcome, RuntimeError, RuntimeObserver,
    RuntimeProgressEvent, ToolApprovalHandler, ToolApprovalOutcome, ToolApprovalRequest,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use store::{RunSearchResult, RunStore, RunSummary};
use types::{
    Message, MessagePart, MessageRole, RunEventEnvelope, RunEventKind, RunId, SessionId,
    ToolOrigin, ToolSpec,
};

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
            TuiCommand::Status => {
                self.restore_startup_summary(state);
                state.status = self.startup_summary.status.clone();
                Ok(false)
            }
            TuiCommand::Clear => {
                state.transcript.clear();
                self.restore_startup_summary(state);
                state.status = "Cleared transcript".to_string();
                Ok(false)
            }
            TuiCommand::Compact { instructions } => {
                if self.runtime.compact_now(instructions.clone()).await? {
                    state.transcript = self.replay_run_lines(&self.runtime.run_id()).await?;
                    let events = self.store.events(&self.runtime.run_id()).await?;
                    state.sidebar = build_turn_sidebar(&events);
                    state.sidebar_title = "Turn".to_string();
                    state.status = if let Some(instructions) = instructions {
                        format!(
                            "Compacted visible history with notes: {}",
                            preview_text(&instructions, 48)
                        )
                    } else {
                        "Compacted visible history".to_string()
                    };
                } else {
                    state.status = "Compaction skipped".to_string();
                }
                Ok(false)
            }
            TuiCommand::Runs { query } => {
                if let Some(query) = query {
                    let runs = self.store.search_runs(&query).await?;
                    state.sidebar = if runs.is_empty() {
                        vec![format!("no runs matched `{query}`")]
                    } else {
                        runs.iter().take(12).map(format_run_search_line).collect()
                    };
                    state.sidebar_title = "Run Search".to_string();
                    state.status = if runs.is_empty() {
                        format!("No runs matched `{query}`")
                    } else {
                        format!(
                            "Found {} matching runs. Use {}run <id-prefix> to replay one.",
                            runs.len(),
                            self.command_prefix
                        )
                    };
                } else {
                    let runs = self.store.list_runs().await?;
                    state.sidebar = if runs.is_empty() {
                        vec!["no runs recorded yet".to_string()]
                    } else {
                        runs.iter().take(12).map(format_run_summary_line).collect()
                    };
                    state.sidebar_title = "Runs".to_string();
                    state.status = if runs.is_empty() {
                        "No runs available yet".to_string()
                    } else {
                        format!(
                            "Listed {} runs. Use {}run <id-prefix> to replay one.",
                            runs.len(),
                            self.command_prefix
                        )
                    };
                }
                Ok(false)
            }
            TuiCommand::Run { run_ref } => {
                let runs = self.store.list_runs().await?;
                let run_id = resolve_run_reference(&runs, &run_ref)?;
                let summary = runs
                    .iter()
                    .find(|summary| summary.run_id == run_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("run missing from store listing: {}", run_id))?;
                let events = self.store.events(&run_id).await?;
                let session_ids = self.store.session_ids(&run_id).await?;
                state.transcript = self.replay_run_lines(&run_id).await?;
                state.sidebar = format_run_sidebar(&summary, &session_ids, &events);
                state.sidebar_title = "Run".to_string();
                state.status = format!(
                    "Loaded run {} with {} transcript messages",
                    preview_id(run_id.as_str()),
                    summary.transcript_message_count
                );
                Ok(false)
            }
            TuiCommand::ExportRun { run_ref, path } => {
                let runs = self.store.list_runs().await?;
                let run_id = resolve_run_reference(&runs, &run_ref)?;
                let events = self.store.events(&run_id).await?;
                let output_path = self
                    .write_output_file(&path, encode_run_events_jsonl(&events)?)
                    .await?;
                state.sidebar = vec![
                    format!("exported run: {}", run_id),
                    format!("path: {}", output_path.display()),
                    format!("events: {}", events.len()),
                ];
                state.sidebar_title = "Export".to_string();
                state.status = format!(
                    "Exported run {} to {}",
                    preview_id(run_id.as_str()),
                    output_path.display()
                );
                Ok(false)
            }
            TuiCommand::ExportTranscript { run_ref, path } => {
                let runs = self.store.list_runs().await?;
                let run_id = resolve_run_reference(&runs, &run_ref)?;
                let transcript = self.replay_run_lines(&run_id).await?;
                let content = if transcript.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", transcript.join("\n\n"))
                };
                let output_path = self.write_output_file(&path, content).await?;
                state.sidebar = vec![
                    format!("exported transcript: {}", run_id),
                    format!("path: {}", output_path.display()),
                    format!("lines: {}", transcript.len()),
                ];
                state.sidebar_title = "Export".to_string();
                state.status = format!(
                    "Exported transcript {} to {}",
                    preview_id(run_id.as_str()),
                    output_path.display()
                );
                Ok(false)
            }
            TuiCommand::Skills { query } => {
                let skills = filter_skills(&self.skills, query.as_deref());
                state.sidebar = if skills.is_empty() {
                    vec!["no skills matched".to_string()]
                } else {
                    skills
                        .iter()
                        .take(16)
                        .map(|skill| format_skill_line(skill))
                        .collect()
                };
                state.sidebar_title = "Skills".to_string();
                state.status = if let Some(query) = query {
                    if skills.is_empty() {
                        format!("No skills matched `{query}`")
                    } else {
                        format!(
                            "Listed {} matching skills. Use {}skill <name> for details.",
                            skills.len(),
                            self.command_prefix
                        )
                    }
                } else if skills.is_empty() {
                    "No skills loaded".to_string()
                } else {
                    format!(
                        "Listed {} skills. Use {}skill <name> for details.",
                        skills.len(),
                        self.command_prefix
                    )
                };
                Ok(false)
            }
            TuiCommand::Skill { skill_name } => {
                let skill = resolve_skill_reference(&self.skills, &skill_name)?;
                state.sidebar = format_skill_sidebar(skill);
                state.sidebar_title = "Skill".to_string();
                state.status = format!("Loaded skill {}", skill.name);
                Ok(false)
            }
            TuiCommand::Tools => {
                state.sidebar = self.runtime_tools().iter().map(format_tool_line).collect();
                state.sidebar_title = "Tools".to_string();
                state.status = "Listed tools".to_string();
                Ok(false)
            }
            TuiCommand::Hooks => {
                state.sidebar = vec![
                    "Claude-style hooks enabled".to_string(),
                    "SessionStart".to_string(),
                    "UserPromptSubmit".to_string(),
                    "PreToolUse/PostToolUse".to_string(),
                    "Stop/SessionEnd".to_string(),
                ];
                state.sidebar_title = "Hooks".to_string();
                state.status = "Listed hooks".to_string();
                Ok(false)
            }
            TuiCommand::Mcp => {
                state.sidebar = self
                    .mcp_servers
                    .iter()
                    .map(|server| {
                        format!(
                            "server: {}  tools={} prompts={} resources={}",
                            server.server_name,
                            server.catalog.tools.len(),
                            server.catalog.prompts.len(),
                            server.catalog.resources.len()
                        )
                    })
                    .collect();
                state.sidebar_title = "MCP".to_string();
                state.status = "Listed MCP servers".to_string();
                Ok(false)
            }
            TuiCommand::Prompts => {
                state.sidebar = self
                    .mcp_servers
                    .iter()
                    .flat_map(|server| {
                        server.catalog.prompts.iter().map(|prompt| {
                            let args = prompt
                                .arguments
                                .iter()
                                .map(|argument| {
                                    if argument.required {
                                        format!("{}*", argument.name)
                                    } else {
                                        argument.name.clone()
                                    }
                                })
                                .collect::<Vec<_>>();
                            let suffix = if args.is_empty() {
                                String::new()
                            } else {
                                format!(" ({})", args.join(", "))
                            };
                            format!(
                                "{}:{}{}{}",
                                server.server_name,
                                prompt.name,
                                suffix,
                                if prompt.description.is_empty() {
                                    String::new()
                                } else {
                                    format!(" - {}", prompt.description)
                                }
                            )
                        })
                    })
                    .collect();
                state.sidebar_title = "Prompts".to_string();
                state.status = "Listed MCP prompts".to_string();
                Ok(false)
            }
            TuiCommand::Resources => {
                state.sidebar = self
                    .mcp_servers
                    .iter()
                    .flat_map(|server| {
                        server.catalog.resources.iter().map(|resource| {
                            format!(
                                "{}:{}{}",
                                server.server_name,
                                resource.uri,
                                resource
                                    .mime_type
                                    .as_deref()
                                    .map(|mime| format!(" [{mime}]"))
                                    .unwrap_or_default()
                            )
                        })
                    })
                    .collect();
                state.sidebar_title = "Resources".to_string();
                state.status = "Listed MCP resources".to_string();
                Ok(false)
            }
            TuiCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                let server = self
                    .mcp_servers
                    .iter()
                    .find(|server| server.server_name == server_name)
                    .ok_or_else(|| anyhow::anyhow!("unknown MCP server: {server_name}"))?;
                let prompt = server.client.get_prompt(&prompt_name, Value::Null).await?;
                state.input = prompt_to_text(&prompt);
                state.sidebar = vec![
                    format!("prompt: {server_name}/{prompt_name}"),
                    format!(
                        "arguments: {}",
                        if prompt.arguments.is_empty() {
                            "none".to_string()
                        } else {
                            prompt
                                .arguments
                                .iter()
                                .map(|argument| {
                                    if argument.required {
                                        format!("{}*", argument.name)
                                    } else {
                                        argument.name.clone()
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                ];
                state.sidebar_title = "Prompt".to_string();
                state.status = format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
                Ok(false)
            }
            TuiCommand::Resource { server_name, uri } => {
                let server = self
                    .mcp_servers
                    .iter()
                    .find(|server| server.server_name == server_name)
                    .ok_or_else(|| anyhow::anyhow!("unknown MCP server: {server_name}"))?;
                let resource = server.client.read_resource(&uri).await?;
                state.input = resource_to_text(&resource);
                state.sidebar = vec![
                    format!("resource: {server_name}:{}", resource.uri),
                    format!(
                        "mime: {}",
                        resource
                            .mime_type
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                ];
                state.sidebar_title = "Resource".to_string();
                state.status = format!(
                    "Loaded MCP resource {server_name}:{} into input",
                    resource.uri
                );
                Ok(false)
            }
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

    async fn replay_run_lines(&self, run_id: &RunId) -> anyhow::Result<Vec<String>> {
        Ok(self
            .store
            .replay_transcript(run_id)
            .await?
            .iter()
            .map(message_to_text)
            .collect())
    }

    async fn write_output_file(
        &self,
        relative_or_absolute: &str,
        content: String,
    ) -> anyhow::Result<PathBuf> {
        let path = resolve_output_path(&self.workspace_root, relative_or_absolute);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(path)
    }
}

struct LiveRenderObserver<'a> {
    terminal: &'a mut Terminal<CrosstermBackend<Stdout>>,
    state: &'a mut TuiState,
    active_assistant_line: Option<usize>,
}

impl<'a> LiveRenderObserver<'a> {
    fn new(terminal: &'a mut Terminal<CrosstermBackend<Stdout>>, state: &'a mut TuiState) -> Self {
        Self {
            terminal,
            state,
            active_assistant_line: None,
        }
    }

    fn redraw(&mut self) -> Result<()> {
        self.terminal.draw(|frame| render(frame, self.state))?;
        Ok(())
    }
}

impl RuntimeObserver for LiveRenderObserver<'_> {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> RuntimeResult<()> {
        match event {
            RuntimeProgressEvent::SteerApplied { message, reason } => {
                self.active_assistant_line = None;
                self.state.transcript.push(format!("system> {message}"));
                self.state.status = match reason {
                    Some(reason) => format!("Steer applied ({reason})"),
                    None => "Steer applied".to_string(),
                };
            }
            RuntimeProgressEvent::UserPromptAdded { prompt } => {
                self.active_assistant_line = None;
                self.state.transcript.push(format!("user> {prompt}"));
                self.state.status = "Thinking...".to_string();
            }
            RuntimeProgressEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                summary,
            } => {
                self.state.status = format!(
                    "Compacted {} messages, kept {} recent ({reason})",
                    source_message_count, retained_message_count
                );
                self.state.sidebar_title = "Compaction".to_string();
                self.state.sidebar = vec![
                    format!("reason: {reason}"),
                    format!("source messages: {source_message_count}"),
                    format!("retained messages: {retained_message_count}"),
                    format!("summary: {}", preview_text(&summary, 120)),
                ];
            }
            RuntimeProgressEvent::ModelRequestStarted { iteration, .. } => {
                self.state.status = if iteration == 1 {
                    "Waiting for model response...".to_string()
                } else {
                    format!("Continuing tool loop (iteration {iteration})...")
                };
            }
            RuntimeProgressEvent::AssistantTextDelta { delta } => {
                if let Some(index) = self.active_assistant_line {
                    self.state.transcript[index].push_str(&delta);
                } else {
                    self.state.transcript.push(format!("assistant> {delta}"));
                    self.active_assistant_line = Some(self.state.transcript.len() - 1);
                }
                self.state.status = "Streaming response...".to_string();
            }
            RuntimeProgressEvent::ToolCallRequested { call } => {
                self.state.status = format!("Model requested tool `{}`", call.tool_name);
            }
            RuntimeProgressEvent::ModelResponseCompleted { tool_calls, .. } => {
                self.active_assistant_line = None;
                self.state.status = if tool_calls.is_empty() {
                    "Model response complete".to_string()
                } else {
                    format!("Model response requested {} tool(s)", tool_calls.len())
                };
            }
            RuntimeProgressEvent::ToolApprovalRequested { call, .. } => {
                self.state.status = format!("Approval required for `{}`", call.tool_name);
            }
            RuntimeProgressEvent::ToolApprovalResolved {
                call,
                approved,
                reason,
            } => {
                self.state.status = if approved {
                    format!("Approved `{}`", call.tool_name)
                } else {
                    format!(
                        "Denied `{}`: {}",
                        call.tool_name,
                        reason.unwrap_or_else(|| "permission denied".to_string())
                    )
                };
            }
            RuntimeProgressEvent::ToolCallStarted { call } => {
                self.state.status = format!("Running tool `{}`...", call.tool_name);
            }
            RuntimeProgressEvent::ToolCallCompleted { call, .. } => {
                self.state.status = format!("Tool `{}` completed", call.tool_name);
            }
            RuntimeProgressEvent::ToolCallFailed { call, error } => {
                self.state.status = format!(
                    "Tool `{}` failed: {}",
                    call.tool_name,
                    preview_text(&error, 64)
                );
            }
            RuntimeProgressEvent::TurnCompleted { .. } => {
                self.active_assistant_line = None;
                self.state.status = "Turn complete".to_string();
            }
        }
        self.redraw()
            .map_err(|error| RuntimeError::invalid_state(error.to_string()))
    }
}

#[derive(Default)]
pub struct InteractiveToolApprovalHandler {
    session_decisions: RwLock<BTreeMap<ToolApprovalCacheKey, SessionApprovalDecision>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ToolApprovalCacheKey {
    tool_name: String,
    origin_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionApprovalDecision {
    Approve,
    Deny,
}

impl ToolApprovalCacheKey {
    fn from_request(request: &ToolApprovalRequest) -> Self {
        Self {
            tool_name: request.call.tool_name.clone(),
            origin_key: tool_origin_cache_key(&request.call.origin),
        }
    }
}

impl SessionApprovalDecision {
    fn to_outcome(self) -> ToolApprovalOutcome {
        match self {
            SessionApprovalDecision::Approve => ToolApprovalOutcome::Approve,
            SessionApprovalDecision::Deny => ToolApprovalOutcome::Deny {
                reason: Some("tool denied for the rest of the session".to_string()),
            },
        }
    }
}

impl InteractiveToolApprovalHandler {
    fn cached_outcome(&self, request: &ToolApprovalRequest) -> Option<ToolApprovalOutcome> {
        self.session_decisions
            .read()
            .unwrap()
            .get(&ToolApprovalCacheKey::from_request(request))
            .copied()
            .map(SessionApprovalDecision::to_outcome)
    }

    fn remember_outcome(&self, request: &ToolApprovalRequest, decision: SessionApprovalDecision) {
        self.session_decisions
            .write()
            .unwrap()
            .insert(ToolApprovalCacheKey::from_request(request), decision);
    }
}

#[async_trait]
impl ToolApprovalHandler for InteractiveToolApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> RuntimeResult<ToolApprovalOutcome> {
        if let Some(outcome) = self.cached_outcome(&request) {
            return Ok(outcome);
        }
        let mut stdout = io::stdout();
        loop {
            render_tool_approval_prompt(&mut stdout, &request)
                .map_err(|error| RuntimeError::invalid_state(error.to_string()))?;
            match event::read().map_err(|error| RuntimeError::invalid_state(error.to_string()))? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        return Ok(ToolApprovalOutcome::Approve);
                    }
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        self.remember_outcome(&request, SessionApprovalDecision::Approve);
                        return Ok(ToolApprovalOutcome::Approve);
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        return Ok(ToolApprovalOutcome::Deny {
                            reason: Some("user denied tool call".to_string()),
                        });
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        self.remember_outcome(&request, SessionApprovalDecision::Deny);
                        return Ok(ToolApprovalOutcome::Deny {
                            reason: Some("tool denied for the rest of the session".to_string()),
                        });
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(ToolApprovalOutcome::Deny {
                            reason: Some("user cancelled tool approval".to_string()),
                        });
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn format_tool_line(spec: &ToolSpec) -> String {
    let origin = match &spec.origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    };
    let title = spec
        .annotations
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(spec.name.as_str());
    format!(
        "{} [{}] ro={} destructive={} open_world={}",
        title,
        origin,
        tool_annotation_bool(spec, "readOnlyHint").unwrap_or(false),
        tool_annotation_bool(spec, "destructiveHint").unwrap_or(true),
        tool_annotation_bool(spec, "openWorldHint").unwrap_or(true),
    )
}

fn format_run_summary_line(summary: &RunSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!(
        "{}  msg={} ev={} sess={}  {}",
        preview_id(summary.run_id.as_str()),
        summary.transcript_message_count,
        summary.event_count,
        summary.session_count,
        prompt
    )
}

fn format_run_search_line(result: &RunSearchResult) -> String {
    let base = format_run_summary_line(&result.summary);
    if result.preview_matches.is_empty() {
        format!("{base}  matches={}", result.matched_event_count)
    } else {
        format!(
            "{base}  matches={}  {}",
            result.matched_event_count,
            preview_text(&result.preview_matches.join(" | "), 80)
        )
    }
}

fn format_run_sidebar(
    summary: &RunSummary,
    session_ids: &[SessionId],
    events: &[RunEventEnvelope],
) -> Vec<String> {
    let mut sidebar = vec![
        format!("run: {}", summary.run_id),
        format!("events: {}", summary.event_count),
        format!("messages: {}", summary.transcript_message_count),
        format!("sessions: {}", summary.session_count),
    ];
    if let Some(prompt) = &summary.last_user_prompt {
        sidebar.push(format!("last prompt: {}", preview_text(prompt, 80)));
    }
    if !session_ids.is_empty() {
        sidebar.push(format!(
            "session ids: {}",
            session_ids
                .iter()
                .map(|session_id| preview_id(session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !events.is_empty() {
        sidebar.push("recent events:".to_string());
        sidebar.extend(
            events
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(format_run_event_line),
        );
    }
    sidebar
}

fn format_run_event_line(event: &RunEventEnvelope) -> String {
    match &event.event {
        RunEventKind::SessionStart { reason } => {
            format!("session_start {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
        RunEventKind::InstructionsLoaded { count } => format!("instructions_loaded count={count}"),
        RunEventKind::SteerApplied { message, reason } => format!(
            "steer {} {}",
            reason.as_deref().unwrap_or(""),
            preview_text(message, 24)
        )
        .trim()
        .to_string(),
        RunEventKind::UserPromptSubmit { prompt } => {
            format!("user_prompt {}", preview_text(prompt, 42))
        }
        RunEventKind::ModelRequestStarted { request } => format!(
            "model_request messages={} tools={}",
            request.messages.len(),
            request.tools.len()
        ),
        RunEventKind::CompactionCompleted {
            reason,
            source_message_count,
            retained_message_count,
            summary_chars,
        } => format!(
            "compaction {} messages={} kept={} summary_chars={}",
            reason, source_message_count, retained_message_count, summary_chars
        ),
        RunEventKind::ModelResponseCompleted {
            assistant_text,
            tool_calls,
            ..
        } => format!(
            "model_response text={} tool_calls={}",
            preview_text(assistant_text, 24),
            tool_calls.len()
        ),
        RunEventKind::HookInvoked { hook_name, event } => {
            format!("hook_invoked {hook_name} {:?}", event)
        }
        RunEventKind::HookCompleted {
            hook_name, output, ..
        } => format!(
            "hook_completed {hook_name} continue={} decision={:?}",
            output.r#continue, output.decision
        ),
        RunEventKind::TranscriptMessage { message } => {
            format!("transcript {}", preview_text(&message_to_text(message), 42))
        }
        RunEventKind::ToolApprovalRequested { call, .. } => {
            format!("approval_requested {}", call.tool_name)
        }
        RunEventKind::ToolApprovalResolved { call, approved, .. } => {
            format!("approval_resolved {} approved={approved}", call.tool_name)
        }
        RunEventKind::ToolCallStarted { call } => format!("tool_started {}", call.tool_name),
        RunEventKind::ToolCallCompleted { call, output } => format!(
            "tool_completed {} {}",
            call.tool_name,
            preview_text(&output.text_content(), 24)
        ),
        RunEventKind::ToolCallFailed { call, error } => {
            format!("tool_failed {} {}", call.tool_name, preview_text(error, 24))
        }
        RunEventKind::Notification { source, message } => {
            format!("notification {source} {}", preview_text(message, 24))
        }
        RunEventKind::Stop { reason } => format!("stop {}", reason.as_deref().unwrap_or(""))
            .trim()
            .to_string(),
        RunEventKind::StopFailure { reason } => {
            format!("stop_failure {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
        RunEventKind::SessionEnd { reason } => {
            format!("session_end {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
    }
}

fn resolve_run_reference(runs: &[RunSummary], run_ref: &str) -> anyhow::Result<RunId> {
    if let Some(run) = runs
        .iter()
        .find(|summary| summary.run_id.as_str() == run_ref)
    {
        return Ok(run.run_id.clone());
    }

    let matches = runs
        .iter()
        .filter(|summary| summary.run_id.as_str().starts_with(run_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!("unknown run id or prefix: {run_ref}")),
        [run] => Ok(run.run_id.clone()),
        _ => Err(anyhow::anyhow!(
            "ambiguous run prefix {run_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|run| preview_id(run.run_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn build_turn_sidebar(events: &[RunEventEnvelope]) -> Vec<String> {
    let mut sidebar = Vec::new();
    if !events.is_empty() {
        sidebar.push("recent events:".to_string());
        sidebar.extend(
            events
                .iter()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(format_run_event_line),
        );
    }
    sidebar
}

fn filter_skills<'a>(skills: &'a [Skill], query: Option<&str>) -> Vec<&'a Skill> {
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return skills.iter().collect();
    };
    let query = query.to_lowercase();
    skills
        .iter()
        .filter(|skill| {
            skill.name.to_lowercase().contains(&query)
                || skill.description.to_lowercase().contains(&query)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&query))
                || skill
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&query))
        })
        .collect()
}

fn resolve_skill_reference<'a>(skills: &'a [Skill], skill_ref: &str) -> anyhow::Result<&'a Skill> {
    if let Some(skill) = skills.iter().find(|skill| skill.name == skill_ref) {
        return Ok(skill);
    }
    if let Some(skill) = skills
        .iter()
        .find(|skill| skill.aliases.iter().any(|alias| alias == skill_ref))
    {
        return Ok(skill);
    }

    let matches = skills
        .iter()
        .filter(|skill| {
            skill.name.starts_with(skill_ref)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.starts_with(skill_ref))
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!("unknown skill: {skill_ref}")),
        [skill] => Ok(skill),
        _ => Err(anyhow::anyhow!(
            "ambiguous skill reference {skill_ref}: {}",
            matches
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn format_skill_line(skill: &Skill) -> String {
    let aliases = if skill.aliases.is_empty() {
        String::new()
    } else {
        format!(" aliases={}", skill.aliases.join(","))
    };
    let tags = if skill.tags.is_empty() {
        String::new()
    } else {
        format!(" tags={}", skill.tags.join(","))
    };
    format!(
        "{}{}{} hooks={} refs={} scripts={} assets={}  {}",
        skill.name,
        aliases,
        tags,
        skill.hooks.len(),
        skill.references.len(),
        skill.scripts.len(),
        skill.assets.len(),
        preview_text(&skill.description, 42)
    )
}

fn format_skill_sidebar(skill: &Skill) -> Vec<String> {
    let mut sidebar = vec![
        format!("name: {}", skill.name),
        format!("root: {}", skill.root_dir.display()),
        format!("description: {}", skill.description),
    ];
    if !skill.aliases.is_empty() {
        sidebar.push(format!("aliases: {}", skill.aliases.join(", ")));
    }
    if !skill.tags.is_empty() {
        sidebar.push(format!("tags: {}", skill.tags.join(", ")));
    }
    sidebar.push(format!("hooks: {}", skill.hooks.len()));
    sidebar.push(format!("references: {}", skill.references.len()));
    sidebar.push(format!("scripts: {}", skill.scripts.len()));
    sidebar.push(format!("assets: {}", skill.assets.len()));
    sidebar.push(format!(
        "instruction: {}",
        preview_text(&skill.system_instruction(), 120)
    ));
    sidebar
}

fn resolve_output_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn encode_run_events_jsonl(events: &[RunEventEnvelope]) -> anyhow::Result<String> {
    let mut lines = Vec::with_capacity(events.len());
    for event in events {
        lines.push(serde_json::to_string(event)?);
    }
    Ok(if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    })
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        format!(
            "{}...",
            collapsed
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn tool_origin_cache_key(origin: &ToolOrigin) -> String {
    match origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    }
}

fn render_tool_approval_prompt(stdout: &mut Stdout, request: &ToolApprovalRequest) -> Result<()> {
    let title = request
        .spec
        .annotations
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(request.spec.name.as_str());
    let origin = match &request.spec.origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    };
    let arguments = serde_json::to_string_pretty(&request.call.arguments)
        .unwrap_or_else(|_| request.call.arguments.to_string());
    let mut lines = vec![
        "Tool Approval Required".to_string(),
        String::new(),
        format!("tool: {title} ({})", request.call.tool_name),
        format!("origin: {origin}"),
    ];
    if !request.reasons.is_empty() {
        lines.push("reasons:".to_string());
        lines.extend(request.reasons.iter().map(|reason| format!("- {reason}")));
    }
    lines.push("arguments:".to_string());
    lines.extend(truncate_preview(&arguments, 18, 120));
    lines.push(String::new());
    lines.push(
        "y = allow once    a = allow this tool for session    n / Esc = deny once".to_string(),
    );
    lines.push("d = deny this tool for session    Ctrl+C = deny once".to_string());

    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
    write!(stdout, "{}\r\n", lines.join("\r\n"))?;
    stdout.flush()?;
    Ok(())
}

fn truncate_preview(value: &str, max_lines: usize, max_columns: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in value.lines() {
        let rendered = if line.chars().count() > max_columns {
            format!(
                "{}...",
                line.chars()
                    .take(max_columns.saturating_sub(3))
                    .collect::<String>()
            )
        } else {
            line.to_string()
        };
        lines.push(rendered);
        if lines.len() == max_lines {
            lines.push("[truncated]".to_string());
            break;
        }
    }
    if lines.is_empty() {
        lines.push("{}".to_string());
    }
    lines
}

fn tool_annotation_bool(spec: &ToolSpec, key: &str) -> Option<bool> {
    spec.annotations
        .get(key)
        .and_then(Value::as_bool)
        .or_else(|| {
            spec.annotations
                .get("mcp_annotations")
                .and_then(Value::as_object)
                .and_then(|value| value.get(key))
                .and_then(Value::as_bool)
        })
}

fn prompt_to_text(prompt: &McpPrompt) -> String {
    if prompt.messages.is_empty() {
        return prompt.description.clone();
    }
    prompt
        .messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn resource_to_text(resource: &McpResource) -> String {
    let parts = resource
        .parts
        .iter()
        .map(message_part_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    if parts.is_empty() {
        resource.description.clone()
    } else {
        parts
    }
}

fn message_to_text(message: &Message) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!(
        "{role}> {}",
        message
            .parts
            .iter()
            .map(message_part_to_text)
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn message_part_to_text(part: &MessagePart) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Image { mime_type, .. } => format!("[image:{mime_type}]"),
        MessagePart::File {
            file_name,
            mime_type,
            uri,
            ..
        } => format!(
            "[file:{}{}{}]",
            file_name.clone().unwrap_or_else(|| "unnamed".to_string()),
            mime_type
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
            uri.as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default()
        ),
        MessagePart::ToolCall { call } => format!("[tool_call:{}]", call.tool_name),
        MessagePart::ToolResult { result } => result.text_content(),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            if text.is_empty() {
                "[reasoning]".to_string()
            } else {
                format!("[reasoning:{}]", preview_text(&text, 48))
            }
        }
        MessagePart::Resource { uri, text, .. } => text.clone().unwrap_or_else(|| uri.clone()),
        MessagePart::Json { value } => value.to_string(),
        MessagePart::ProviderExtension { provider, kind, .. } => {
            format!("[provider_extension:{provider}:{kind}]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InteractiveToolApprovalHandler, SessionApprovalDecision, ToolApprovalCacheKey,
        resolve_run_reference, resolve_skill_reference,
    };
    use agent::skills::Skill;
    use runtime::{ToolApprovalOutcome, ToolApprovalRequest};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use store::RunSummary;
    use types::{RunId, ToolCall, ToolCallId, ToolOrigin, ToolOutputMode, ToolSpec};

    fn sample_request(tool_name: &str, origin: ToolOrigin) -> ToolApprovalRequest {
        ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: "call-1".into(),
                tool_name: tool_name.to_string(),
                arguments: json!({"path":"sample.txt"}),
                origin: origin.clone(),
            },
            spec: ToolSpec {
                name: tool_name.to_string(),
                description: "sample".to_string(),
                input_schema: json!({"type":"object"}),
                output_mode: ToolOutputMode::Text,
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

    #[test]
    fn resolves_unique_run_prefix() {
        let runs = vec![
            RunSummary {
                run_id: RunId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("first".to_string()),
            },
            RunSummary {
                run_id: RunId::from("def67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("second".to_string()),
            },
        ];

        assert_eq!(
            resolve_run_reference(&runs, "abc").unwrap(),
            RunId::from("abc12345")
        );
    }

    #[test]
    fn rejects_ambiguous_run_prefix() {
        let runs = vec![
            RunSummary {
                run_id: RunId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
            },
            RunSummary {
                run_id: RunId::from("abc67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
            },
        ];

        assert!(resolve_run_reference(&runs, "abc").is_err());
    }

    #[test]
    fn resolves_skill_by_alias() {
        let skills = vec![Skill {
            name: "pdf".to_string(),
            description: "Use for PDF tasks".to_string(),
            aliases: vec!["acrobat".to_string()],
            body: "Do PDF things.".to_string(),
            root_dir: PathBuf::from("/tmp/pdf"),
            tags: vec!["document".to_string()],
            hooks: Vec::new(),
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            metadata: BTreeMap::new(),
            extension_metadata: BTreeMap::new(),
        }];

        let resolved = resolve_skill_reference(&skills, "acrobat").unwrap();
        assert_eq!(resolved.name, "pdf");
    }
}
