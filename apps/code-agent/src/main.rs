use agent::AgentWorkspaceLayout;
use agent::runtime::{HostRuntimeLimits, build_host_tokio_runtime};
use agent_env::EnvMap;
use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use code_agent_backend::{
    AppOptions, CodeAgentUiSession, LoadedAgentSession, LoadedSession, LoadedTask,
    McpPromptSummary, McpResourceSummary, McpServerSummary, PersistedAgentSessionSummary,
    PersistedSessionSearchMatch, PersistedSessionSummary, PersistedTaskSummary,
    SandboxFallbackNotice, SessionApprovalMode, SessionArchiveArtifact, SessionExportArtifact,
    SessionExportKind, SessionHistoryClient, SessionImportArtifact, SessionStartupSnapshot,
    StartupDiagnosticsSnapshot, UIAsyncCommand, UIQuery, build_sandbox_fallback_notice,
    build_session_with_approval_mode, build_session_with_approval_mode_and_progress,
    inject_process_env, inspect_sandbox_preflight, message_to_text,
};
use code_agent_config::{
    ManagedPluginArtifact, ManagedSkillArtifact, add_core_mcp_server, add_managed_plugin,
    add_managed_skill, delete_core_mcp_server, delete_managed_plugin, delete_managed_skill,
    set_core_mcp_server_enabled, set_managed_plugin_enabled, set_managed_skill_enabled,
};
use code_agent_tui::theme::install_theme_catalog;
use code_agent_tui::{
    CodeAgentTui, SharedUiState, StartupLoadingScreen, confirm_unsandboxed_startup_screen,
};
use nanoclaw_config::CoreConfig;
use std::collections::BTreeMap;
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use store::SessionTokenUsageReport;
use tracing::warn;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(disable_help_subcommand = true, subcommand_precedence_over_arg = true)]
struct Cli {
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    #[arg(long = "skill-root", value_name = "PATH")]
    skill_roots: Vec<String>,
    #[arg(long = "plugin-root", value_name = "PATH")]
    plugin_roots: Vec<String>,
    #[arg(long = "memory-plugin", value_name = "ID|none")]
    memory_plugin: Option<String>,
    #[arg(long = "sandbox-fail-if-unavailable", value_name = "BOOL")]
    sandbox_fail_if_unavailable: Option<String>,
    #[arg(long = "allow-no-sandbox")]
    allow_no_sandbox: bool,
    #[command(subcommand)]
    command: Option<CliCommand>,
    #[arg(
        value_name = "PROMPT",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    prompt: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Run one prompt headlessly and exit.
    Exec(ExecCommandArgs),
    /// Reattach a persisted top-level session.
    Resume(SessionLaunchArgs),
    /// Start a fresh session seeded from a persisted session transcript.
    Fork(SessionLaunchArgs),
    /// List persisted sessions, or search them when a query is provided.
    Sessions(SessionSearchArgs),
    /// Inspect a persisted session transcript and metadata.
    Session(SessionLookupArgs),
    /// List persisted agent sessions.
    #[command(name = "agent-sessions")]
    AgentSessions(ScopedSessionRefArgs),
    /// Inspect a persisted agent session.
    #[command(name = "agent-session")]
    AgentSession(AgentSessionLookupArgs),
    /// List persisted child tasks.
    Tasks(ScopedSessionRefArgs),
    /// Inspect a persisted task.
    Task(TaskLookupArgs),
    /// Export a restorable session archive.
    Export(SessionExportArgs),
    /// Import a session archive into the local store.
    Import(SessionImportArgs),
    /// Export persisted session events as JSONL.
    #[command(name = "export-events")]
    ExportSession(SessionExportArgs),
    /// Export a persisted session transcript as plain text.
    #[command(name = "export-transcript")]
    ExportTranscript(SessionExportArgs),
    /// Print the current startup diagnostics snapshot.
    Diagnostics,
    /// Manage configured MCP servers.
    Mcp(McpCommandArgs),
    /// Manage workspace-local managed skills.
    Skill(SkillCommandArgs),
    /// Manage workspace-local plugins and plugin enablement.
    Plugin(PluginCommandArgs),
    /// List MCP prompts exposed by connected servers.
    Prompts,
    /// List MCP resources exposed by connected servers.
    Resources,
}

#[derive(Debug, Args)]
struct McpCommandArgs {
    #[command(subcommand)]
    command: McpSubcommand,
}

#[derive(Debug, Subcommand)]
enum McpSubcommand {
    /// Add a configured MCP server.
    Add(McpAddArgs),
    /// Delete a configured MCP server.
    Delete(McpNamedArgs),
    /// Enable a configured MCP server.
    Enable(McpNamedArgs),
    /// Disable a configured MCP server.
    Disable(McpNamedArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum McpAddTransportKind {
    Stdio,
    Http,
}

#[derive(Debug, Args)]
struct McpAddArgs {
    #[arg(value_name = "NAME")]
    name: String,
    #[arg(long = "type", value_enum)]
    transport: McpAddTransportKind,
    #[arg(long, value_name = "COMMAND")]
    command: Option<String>,
    #[arg(long = "arg", value_name = "ARG")]
    args: Vec<String>,
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    env: Vec<(String, String)>,
    #[arg(long, value_name = "PATH")]
    cwd: Option<String>,
    #[arg(long, value_name = "URL")]
    url: Option<String>,
    #[arg(long = "header", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    headers: Vec<(String, String)>,
    #[arg(
        value_name = "COMMAND",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    command_argv: Vec<String>,
}

#[derive(Debug, Args)]
struct McpNamedArgs {
    #[arg(value_name = "NAME")]
    name: String,
}

#[derive(Debug, Args)]
struct SkillCommandArgs {
    #[command(subcommand)]
    command: SkillSubcommand,
}

#[derive(Debug, Subcommand)]
enum SkillSubcommand {
    /// Copy a skill directory into the managed workspace root.
    Add(SkillAddArgs),
    /// Delete a managed skill copy.
    Delete(SkillNamedArgs),
    /// Re-enable a previously disabled managed skill.
    Enable(SkillNamedArgs),
    /// Disable a managed skill without deleting it.
    Disable(SkillNamedArgs),
}

#[derive(Debug, Args)]
struct SkillAddArgs {
    #[arg(value_name = "PATH")]
    path: String,
}

#[derive(Debug, Args)]
struct SkillNamedArgs {
    #[arg(value_name = "NAME")]
    name: String,
}

#[derive(Debug, Args)]
struct PluginCommandArgs {
    #[command(subcommand)]
    command: PluginSubcommand,
}

#[derive(Debug, Subcommand)]
enum PluginSubcommand {
    /// Copy a plugin directory into the managed workspace root.
    Add(PluginAddArgs),
    /// Delete a managed plugin copy.
    Delete(PluginNamedArgs),
    /// Enable a plugin in the persisted config.
    Enable(PluginNamedArgs),
    /// Disable a plugin in the persisted config.
    Disable(PluginNamedArgs),
}

#[derive(Debug, Args)]
struct PluginAddArgs {
    #[arg(value_name = "PATH")]
    path: String,
}

#[derive(Debug, Args)]
struct PluginNamedArgs {
    #[arg(value_name = "ID")]
    id: String,
}

#[derive(Debug, Args, Default)]
struct SessionLookupArgs {
    #[arg(value_name = "SESSION_ID", conflicts_with = "last")]
    session_ref: Option<String>,
    #[arg(long, conflicts_with = "session_ref")]
    last: bool,
}

#[derive(Debug, Args)]
struct SessionLaunchArgs {
    #[arg(long)]
    last: bool,
    #[arg(
        value_name = "ARG",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    args: Vec<String>,
}

#[derive(Debug, Args)]
struct ExecCommandArgs {
    #[command(subcommand)]
    command: Option<ExecLaunchCommand>,
    #[arg(
        value_name = "PROMPT",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    prompt: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum ExecLaunchCommand {
    /// Run one headless prompt against a resumed persisted session.
    Resume(SessionLaunchArgs),
    /// Run one headless prompt against a forked persisted session.
    Fork(SessionLaunchArgs),
}

#[derive(Debug, Args, Default)]
struct SessionSearchArgs {
    #[arg(
        value_name = "QUERY",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    query: Vec<String>,
}

#[derive(Debug, Args, Default)]
struct ScopedSessionRefArgs {
    #[arg(
        value_name = "SESSION_ID",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    session_ref: Vec<String>,
}

#[derive(Debug, Args)]
struct AgentSessionLookupArgs {
    #[arg(
        value_name = "AGENT_SESSION_ID",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    agent_session_ref: Vec<String>,
}

#[derive(Debug, Args)]
struct TaskLookupArgs {
    #[arg(value_name = "TASK_ID", allow_hyphen_values = true)]
    task_ref: String,
}

#[derive(Debug, Args)]
struct SessionExportArgs {
    #[arg(long)]
    last: bool,
    #[arg(
        value_name = "ARG",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    args: Vec<String>,
}

#[derive(Debug, Args)]
struct SessionImportArgs {
    #[arg(value_name = "PATH")]
    path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LaunchMode {
    Default,
    Resume(SessionSelector),
    Fork(SessionSelector),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ManagementCommand {
    Mcp(McpManagementCommand),
    Skill(SkillManagementCommand),
    Plugin(PluginManagementCommand),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum McpManagementCommand {
    Add { server: agent::mcp::McpServerConfig },
    Delete { name: String },
    SetEnabled { name: String, enabled: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SkillManagementCommand {
    Add { path: String },
    Delete { name: String },
    SetEnabled { name: String, enabled: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PluginManagementCommand {
    Add { path: String },
    Delete { id: String },
    SetEnabled { id: String, enabled: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReadOnlyCommand {
    Sessions {
        query: Option<String>,
    },
    Session {
        selector: SessionSelector,
    },
    AgentSessions {
        session_ref: Option<String>,
    },
    AgentSession {
        agent_session_ref: String,
    },
    Tasks {
        session_ref: Option<String>,
    },
    Task {
        task_ref: String,
    },
    ExportArchive {
        selector: SessionSelector,
        output_path: String,
    },
    ImportArchive {
        input_path: String,
    },
    ExportSession {
        selector: SessionSelector,
        output_path: String,
    },
    ExportTranscript {
        selector: SessionSelector,
        output_path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LiveInspectCommand {
    Diagnostics,
    Prompts,
    Resources,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExplicitExecCommand {
    launch_mode: LaunchMode,
    prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionSelector {
    Reference(String),
    Last,
}

impl Cli {
    fn app_option_flag_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(system_prompt) = &self.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(system_prompt.clone());
        }
        for skill_root in &self.skill_roots {
            args.push("--skill-root".to_string());
            args.push(skill_root.clone());
        }
        for plugin_root in &self.plugin_roots {
            args.push("--plugin-root".to_string());
            args.push(plugin_root.clone());
        }
        if let Some(memory_plugin) = &self.memory_plugin {
            args.push("--memory-plugin".to_string());
            args.push(memory_plugin.clone());
        }
        if let Some(sandbox_fail_if_unavailable) = &self.sandbox_fail_if_unavailable {
            args.push("--sandbox-fail-if-unavailable".to_string());
            args.push(sandbox_fail_if_unavailable.clone());
        }
        if self.allow_no_sandbox {
            args.push("--allow-no-sandbox".to_string());
        }
        args
    }

    fn app_option_args(&self) -> Vec<String> {
        let mut args = self.app_option_flag_args();
        match &self.command {
            Some(CliCommand::Resume(command)) | Some(CliCommand::Fork(command)) => {
                args.extend(command.prompt_parts());
            }
            None => args.extend(self.prompt.clone()),
            Some(
                CliCommand::Exec(_)
                | CliCommand::Sessions(_)
                | CliCommand::Session(_)
                | CliCommand::AgentSessions(_)
                | CliCommand::AgentSession(_)
                | CliCommand::Tasks(_)
                | CliCommand::Task(_)
                | CliCommand::Export(_)
                | CliCommand::Import(_)
                | CliCommand::ExportSession(_)
                | CliCommand::ExportTranscript(_)
                | CliCommand::Diagnostics
                | CliCommand::Mcp(_)
                | CliCommand::Skill(_)
                | CliCommand::Plugin(_)
                | CliCommand::Prompts
                | CliCommand::Resources,
            ) => {}
        }
        args
    }

    fn launch_mode(&self) -> Result<LaunchMode> {
        match &self.command {
            Some(CliCommand::Exec(_)) => Ok(LaunchMode::Default),
            Some(CliCommand::Resume(command)) => {
                Ok(LaunchMode::Resume(command.selector("resume")?))
            }
            Some(CliCommand::Fork(command)) => Ok(LaunchMode::Fork(command.selector("fork")?)),
            Some(
                CliCommand::Sessions(_)
                | CliCommand::Session(_)
                | CliCommand::AgentSessions(_)
                | CliCommand::AgentSession(_)
                | CliCommand::Tasks(_)
                | CliCommand::Task(_)
                | CliCommand::Export(_)
                | CliCommand::Import(_)
                | CliCommand::ExportSession(_)
                | CliCommand::ExportTranscript(_)
                | CliCommand::Diagnostics
                | CliCommand::Mcp(_)
                | CliCommand::Skill(_)
                | CliCommand::Plugin(_)
                | CliCommand::Prompts
                | CliCommand::Resources,
            ) => Ok(LaunchMode::Default),
            None => Ok(LaunchMode::Default),
        }
    }

    fn management_command(&self) -> Result<Option<ManagementCommand>> {
        match &self.command {
            Some(CliCommand::Mcp(command)) => {
                Ok(Some(ManagementCommand::Mcp(command.management_command()?)))
            }
            Some(CliCommand::Skill(command)) => {
                Ok(Some(ManagementCommand::Skill(command.management_command())))
            }
            Some(CliCommand::Plugin(command)) => Ok(Some(ManagementCommand::Plugin(
                command.management_command(),
            ))),
            Some(
                CliCommand::Exec(_)
                | CliCommand::Resume(_)
                | CliCommand::Fork(_)
                | CliCommand::Sessions(_)
                | CliCommand::Session(_)
                | CliCommand::AgentSessions(_)
                | CliCommand::AgentSession(_)
                | CliCommand::Tasks(_)
                | CliCommand::Task(_)
                | CliCommand::Export(_)
                | CliCommand::Import(_)
                | CliCommand::ExportSession(_)
                | CliCommand::ExportTranscript(_)
                | CliCommand::Diagnostics
                | CliCommand::Prompts
                | CliCommand::Resources,
            )
            | None => Ok(None),
        }
    }

    fn read_only_command(&self) -> Result<Option<ReadOnlyCommand>> {
        match &self.command {
            Some(CliCommand::Exec(_)) => Ok(None),
            Some(CliCommand::Sessions(command)) => Ok(Some(ReadOnlyCommand::Sessions {
                query: command.query(),
            })),
            Some(CliCommand::Session(command)) => Ok(Some(ReadOnlyCommand::Session {
                selector: command.selector("session")?,
            })),
            Some(CliCommand::AgentSessions(command)) => Ok(Some(ReadOnlyCommand::AgentSessions {
                session_ref: command.session_ref(),
            })),
            Some(CliCommand::AgentSession(command)) => Ok(Some(ReadOnlyCommand::AgentSession {
                agent_session_ref: command.agent_session_ref()?,
            })),
            Some(CliCommand::Tasks(command)) => Ok(Some(ReadOnlyCommand::Tasks {
                session_ref: command.session_ref(),
            })),
            Some(CliCommand::Task(command)) => Ok(Some(ReadOnlyCommand::Task {
                task_ref: command.task_ref.clone(),
            })),
            Some(CliCommand::Export(command)) => {
                let (selector, output_path) = command.selector_and_path("export")?;
                Ok(Some(ReadOnlyCommand::ExportArchive {
                    selector,
                    output_path,
                }))
            }
            Some(CliCommand::Import(command)) => Ok(Some(ReadOnlyCommand::ImportArchive {
                input_path: command.path.clone(),
            })),
            Some(CliCommand::ExportSession(command)) => {
                let (selector, output_path) = command.selector_and_path("export-events")?;
                Ok(Some(ReadOnlyCommand::ExportSession {
                    selector,
                    output_path,
                }))
            }
            Some(CliCommand::ExportTranscript(command)) => {
                let (selector, output_path) = command.selector_and_path("export-transcript")?;
                Ok(Some(ReadOnlyCommand::ExportTranscript {
                    selector,
                    output_path,
                }))
            }
            Some(
                CliCommand::Resume(_)
                | CliCommand::Fork(_)
                | CliCommand::Diagnostics
                | CliCommand::Mcp(_)
                | CliCommand::Skill(_)
                | CliCommand::Plugin(_)
                | CliCommand::Prompts
                | CliCommand::Resources,
            )
            | None => Ok(None),
        }
    }

    fn explicit_exec_command(&self) -> Result<Option<ExplicitExecCommand>> {
        match &self.command {
            Some(CliCommand::Exec(command)) => Ok(Some(command.explicit_exec_command()?)),
            Some(
                CliCommand::Resume(_)
                | CliCommand::Fork(_)
                | CliCommand::Sessions(_)
                | CliCommand::Session(_)
                | CliCommand::AgentSessions(_)
                | CliCommand::AgentSession(_)
                | CliCommand::Tasks(_)
                | CliCommand::Task(_)
                | CliCommand::Export(_)
                | CliCommand::Import(_)
                | CliCommand::ExportSession(_)
                | CliCommand::ExportTranscript(_)
                | CliCommand::Diagnostics
                | CliCommand::Mcp(_)
                | CliCommand::Skill(_)
                | CliCommand::Plugin(_)
                | CliCommand::Prompts
                | CliCommand::Resources,
            )
            | None => Ok(None),
        }
    }

    fn live_inspect_command(&self) -> Option<LiveInspectCommand> {
        match self.command {
            Some(CliCommand::Diagnostics) => Some(LiveInspectCommand::Diagnostics),
            Some(CliCommand::Prompts) => Some(LiveInspectCommand::Prompts),
            Some(CliCommand::Resources) => Some(LiveInspectCommand::Resources),
            Some(
                CliCommand::Exec(_)
                | CliCommand::Resume(_)
                | CliCommand::Fork(_)
                | CliCommand::Sessions(_)
                | CliCommand::Session(_)
                | CliCommand::AgentSessions(_)
                | CliCommand::AgentSession(_)
                | CliCommand::Tasks(_)
                | CliCommand::Task(_)
                | CliCommand::Export(_)
                | CliCommand::Import(_)
                | CliCommand::ExportSession(_)
                | CliCommand::ExportTranscript(_)
                | CliCommand::Mcp(_)
                | CliCommand::Skill(_)
                | CliCommand::Plugin(_),
            )
            | None => None,
        }
    }
}

impl SessionLookupArgs {
    fn selector(&self, verb: &str) -> Result<SessionSelector> {
        match (&self.session_ref, self.last) {
            (Some(session_ref), false) => Ok(SessionSelector::Reference(session_ref.clone())),
            (None, true) => Ok(SessionSelector::Last),
            (None, false) => {
                bail!("`{verb}` requires a session id or `--last`; run `--help` for usage")
            }
            (Some(_), true) => unreachable!("clap enforces selector conflicts"),
        }
    }
}

impl McpCommandArgs {
    fn management_command(&self) -> Result<McpManagementCommand> {
        match &self.command {
            McpSubcommand::Add(command) => Ok(McpManagementCommand::Add {
                server: command.server_config()?,
            }),
            McpSubcommand::Delete(command) => Ok(McpManagementCommand::Delete {
                name: command.name.clone(),
            }),
            McpSubcommand::Enable(command) => Ok(McpManagementCommand::SetEnabled {
                name: command.name.clone(),
                enabled: true,
            }),
            McpSubcommand::Disable(command) => Ok(McpManagementCommand::SetEnabled {
                name: command.name.clone(),
                enabled: false,
            }),
        }
    }
}

impl SkillCommandArgs {
    fn management_command(&self) -> SkillManagementCommand {
        match &self.command {
            SkillSubcommand::Add(command) => SkillManagementCommand::Add {
                path: command.path.clone(),
            },
            SkillSubcommand::Delete(command) => SkillManagementCommand::Delete {
                name: command.name.clone(),
            },
            SkillSubcommand::Enable(command) => SkillManagementCommand::SetEnabled {
                name: command.name.clone(),
                enabled: true,
            },
            SkillSubcommand::Disable(command) => SkillManagementCommand::SetEnabled {
                name: command.name.clone(),
                enabled: false,
            },
        }
    }
}

impl PluginCommandArgs {
    fn management_command(&self) -> PluginManagementCommand {
        match &self.command {
            PluginSubcommand::Add(command) => PluginManagementCommand::Add {
                path: command.path.clone(),
            },
            PluginSubcommand::Delete(command) => PluginManagementCommand::Delete {
                id: command.id.clone(),
            },
            PluginSubcommand::Enable(command) => PluginManagementCommand::SetEnabled {
                id: command.id.clone(),
                enabled: true,
            },
            PluginSubcommand::Disable(command) => PluginManagementCommand::SetEnabled {
                id: command.id.clone(),
                enabled: false,
            },
        }
    }
}

impl McpAddArgs {
    fn server_config(&self) -> Result<agent::mcp::McpServerConfig> {
        let name = self.name.trim();
        if name.is_empty() {
            bail!("`mcp add` requires a non-empty server name");
        }
        let transport = match self.transport {
            McpAddTransportKind::Stdio => self.stdio_transport()?,
            McpAddTransportKind::Http => self.http_transport()?,
        };
        Ok(agent::mcp::McpServerConfig {
            name: name.into(),
            enabled: true,
            transport,
        })
    }

    fn stdio_transport(&self) -> Result<agent::mcp::McpTransportConfig> {
        if self.url.is_some() {
            bail!("`mcp add --type stdio` does not accept `--url`");
        }
        if !self.headers.is_empty() {
            bail!("`mcp add --type stdio` does not accept `--header`");
        }
        let (command, args) = if !self.command_argv.is_empty() {
            if self.command.is_some() || !self.args.is_empty() {
                bail!(
                    "`mcp add --type stdio` accepts either `--command/--arg` or a trailing `-- <command> [args...]`, not both"
                );
            }
            (
                self.command_argv[0].clone(),
                self.command_argv[1..].to_vec(),
            )
        } else {
            let command = self.command.clone().ok_or_else(|| {
                anyhow!("`mcp add --type stdio` requires `--command` or `-- <command>`")
            })?;
            (command, self.args.clone())
        };
        if command.trim().is_empty() {
            bail!("`mcp add --type stdio` requires a non-empty command");
        }
        Ok(agent::mcp::McpTransportConfig::Stdio {
            command,
            args,
            env: self.env.iter().cloned().collect::<BTreeMap<_, _>>(),
            cwd: self.cwd.clone(),
        })
    }

    fn http_transport(&self) -> Result<agent::mcp::McpTransportConfig> {
        if self.command.is_some()
            || !self.args.is_empty()
            || !self.env.is_empty()
            || self.cwd.is_some()
            || !self.command_argv.is_empty()
        {
            bail!("`mcp add --type http` only accepts `--url` and optional `--header KEY=VALUE`");
        }
        let url = self
            .url
            .clone()
            .ok_or_else(|| anyhow!("`mcp add --type http` requires `--url`"))?;
        if url.trim().is_empty() {
            bail!("`mcp add --type http` requires a non-empty URL");
        }
        Ok(agent::mcp::McpTransportConfig::StreamableHttp {
            url,
            headers: self.headers.iter().cloned().collect::<BTreeMap<_, _>>(),
        })
    }
}

impl ExecCommandArgs {
    fn explicit_exec_command(&self) -> Result<ExplicitExecCommand> {
        match &self.command {
            Some(ExecLaunchCommand::Resume(command)) => Ok(ExplicitExecCommand {
                launch_mode: LaunchMode::Resume(command.selector("exec resume")?),
                prompt: command.required_prompt("exec resume")?,
            }),
            Some(ExecLaunchCommand::Fork(command)) => Ok(ExplicitExecCommand {
                launch_mode: LaunchMode::Fork(command.selector("exec fork")?),
                prompt: command.required_prompt("exec fork")?,
            }),
            None => Ok(ExplicitExecCommand {
                launch_mode: LaunchMode::Default,
                prompt: render_cli_prompt(&self.prompt, "exec")?,
            }),
        }
    }
}

impl SessionLaunchArgs {
    fn selector(&self, verb: &str) -> Result<SessionSelector> {
        match (self.last, self.args.as_slice()) {
            (true, _) => Ok(SessionSelector::Last),
            (false, [session_ref, ..]) => Ok(SessionSelector::Reference(session_ref.clone())),
            (false, []) => {
                bail!("`{verb}` requires a session id or `--last`; run `--help` for usage")
            }
        }
    }

    fn prompt_parts(&self) -> Vec<String> {
        match (self.last, self.args.as_slice()) {
            (true, args) => args.to_vec(),
            (false, [_session_ref, prompt @ ..]) => prompt.to_vec(),
            (false, []) => Vec::new(),
        }
    }

    fn required_prompt(&self, verb: &str) -> Result<String> {
        render_cli_prompt(&self.prompt_parts(), verb)
    }
}

impl SessionSearchArgs {
    fn query(&self) -> Option<String> {
        let query = self.query.join(" ");
        let query = query.trim();
        (!query.is_empty()).then(|| query.to_string())
    }
}

fn render_cli_prompt(parts: &[String], verb: &str) -> Result<String> {
    let prompt = parts.join(" ");
    let prompt = prompt.trim();
    if prompt.is_empty() {
        bail!("`{verb}` requires a prompt; run `--help` for usage");
    }
    Ok(prompt.to_string())
}

impl ScopedSessionRefArgs {
    fn session_ref(&self) -> Option<String> {
        let session_ref = self.session_ref.join(" ");
        let session_ref = session_ref.trim();
        (!session_ref.is_empty()).then(|| session_ref.to_string())
    }
}

impl AgentSessionLookupArgs {
    fn agent_session_ref(&self) -> Result<String> {
        let agent_session_ref = self.agent_session_ref.join(" ");
        let agent_session_ref = agent_session_ref.trim();
        if agent_session_ref.is_empty() {
            bail!("`agent-session` requires an agent session id; run `--help` for usage");
        }
        Ok(agent_session_ref.to_string())
    }
}

impl SessionExportArgs {
    fn selector_and_path(&self, verb: &str) -> Result<(SessionSelector, String)> {
        match (self.last, self.args.as_slice()) {
            (false, [session_ref, output_path]) => Ok((
                SessionSelector::Reference(session_ref.clone()),
                output_path.clone(),
            )),
            (true, [output_path]) => Ok((SessionSelector::Last, output_path.clone())),
            (false, _) => bail!(
                "`{verb}` requires <session-id> <path>; use `--last <path>` to export the latest session"
            ),
            (true, _) => bail!("`{verb} --last` requires exactly one output path"),
        }
    }
}

fn parse_key_value_arg(raw: &str) -> Result<(String, String), String> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err("expected KEY=VALUE".to_string());
    };
    let key = key.trim();
    if key.is_empty() {
        return Err("expected non-empty KEY in KEY=VALUE".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            print_fatal_error(&error);
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<()> {
    let cli = Cli::parse();
    let workspace_root = env::current_dir().context("failed to resolve current workspace")?;
    let env_map = EnvMap::from_workspace_dir(&workspace_root)?;
    inject_process_env(&env_map);
    let _tracing_guard = init_tracing(&workspace_root)?;
    if let Some(command) = cli.management_command()? {
        let runtime = build_host_tokio_runtime(HostRuntimeLimits::default())
            .context("failed to build tokio runtime")?;
        return runtime.block_on(run_management_command(workspace_root, command));
    }
    if let Some(command) = cli.read_only_command()? {
        let core = CoreConfig::load_from_dir(&workspace_root)?;
        let runtime = build_host_tokio_runtime(HostRuntimeLimits {
            worker_threads: core.host.tokio_worker_threads,
            max_blocking_threads: core.host.tokio_max_blocking_threads,
        })
        .context("failed to build tokio runtime")?;
        return runtime.block_on(run_read_only_command(workspace_root, core, command));
    }
    if let Some(command) = cli.explicit_exec_command()? {
        let mut args = cli.app_option_flag_args();
        args.push(command.prompt);
        let mut options = AppOptions::from_env_and_args_iter(&workspace_root, &env_map, args)?;
        // Explicit `exec` is a scripting surface, so keep sandbox fallback
        // non-interactive even when the caller happens to be attached to a TTY.
        confirm_unsandboxed_startup_if_needed(&workspace_root, &mut options, false)?;
        let runtime = build_host_tokio_runtime(HostRuntimeLimits {
            worker_threads: options.tokio_worker_threads,
            max_blocking_threads: options.tokio_max_blocking_threads,
        })
        .context("failed to build tokio runtime")?;
        return runtime.block_on(run_headless_one_shot(
            workspace_root,
            options,
            command.launch_mode,
        ));
    }
    let options =
        AppOptions::from_env_and_args_iter(&workspace_root, &env_map, cli.app_option_args())?;
    if let Some(command) = cli.live_inspect_command() {
        let mut options = options;
        let stdin_is_terminal = io::stdin().is_terminal();
        let stdout_is_terminal = io::stdout().is_terminal();
        confirm_unsandboxed_startup_if_needed(
            &workspace_root,
            &mut options,
            stdin_is_terminal && stdout_is_terminal,
        )?;
        let runtime = build_host_tokio_runtime(HostRuntimeLimits {
            worker_threads: options.tokio_worker_threads,
            max_blocking_threads: options.tokio_max_blocking_threads,
        })
        .context("failed to build tokio runtime")?;
        return runtime.block_on(run_live_inspection_command(
            workspace_root,
            options,
            command,
        ));
    }
    let launch_mode = cli.launch_mode()?;
    // Startup loading and the unsandboxed-risk prompt both render before the
    // main `CodeAgentTui` exists, so install the configured theme catalog as
    // soon as config parsing succeeds instead of letting those early surfaces
    // fall back to the builtin default theme.
    install_theme_catalog(options.theme_catalog.clone());

    let runtime = build_host_tokio_runtime(HostRuntimeLimits {
        worker_threads: options.tokio_worker_threads,
        max_blocking_threads: options.tokio_max_blocking_threads,
    })
    .context("failed to build tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async_main(workspace_root, options, launch_mode)))
}

async fn run_management_command(workspace_root: PathBuf, command: ManagementCommand) -> Result<()> {
    let mut stdout = io::stdout().lock();
    match command {
        ManagementCommand::Mcp(command) => match command {
            McpManagementCommand::Add { server } => {
                let path = add_core_mcp_server(&workspace_root, server.clone())?;
                write_mcp_management_artifact(
                    &mut stdout,
                    "Added",
                    server.name.as_str(),
                    &path,
                    Some(server.enabled),
                )?;
            }
            McpManagementCommand::Delete { name } => {
                let path = delete_core_mcp_server(&workspace_root, &name)?;
                write_mcp_management_artifact(&mut stdout, "Deleted", &name, &path, None)?;
            }
            McpManagementCommand::SetEnabled { name, enabled } => {
                let path = set_core_mcp_server_enabled(&workspace_root, &name, enabled)?;
                write_mcp_management_artifact(
                    &mut stdout,
                    if enabled { "Enabled" } else { "Disabled" },
                    &name,
                    &path,
                    Some(enabled),
                )?;
            }
        },
        ManagementCommand::Skill(command) => match command {
            SkillManagementCommand::Add { path } => {
                let artifact = add_managed_skill(&workspace_root, Path::new(&path)).await?;
                write_skill_management_artifact(&mut stdout, "Added", &artifact)?;
            }
            SkillManagementCommand::Delete { name } => {
                let artifact = delete_managed_skill(&workspace_root, &name).await?;
                write_skill_management_artifact(&mut stdout, "Deleted", &artifact)?;
            }
            SkillManagementCommand::SetEnabled { name, enabled } => {
                let artifact = set_managed_skill_enabled(&workspace_root, &name, enabled).await?;
                write_skill_management_artifact(
                    &mut stdout,
                    if enabled { "Enabled" } else { "Disabled" },
                    &artifact,
                )?;
            }
        },
        ManagementCommand::Plugin(command) => match command {
            PluginManagementCommand::Add { path } => {
                let artifact = add_managed_plugin(&workspace_root, Path::new(&path)).await?;
                write_plugin_copy_artifact(&mut stdout, "Added", &artifact)?;
            }
            PluginManagementCommand::Delete { id } => {
                let artifact = delete_managed_plugin(&workspace_root, &id).await?;
                write_plugin_copy_artifact(&mut stdout, "Deleted", &artifact)?;
            }
            PluginManagementCommand::SetEnabled { id, enabled } => {
                let path = set_managed_plugin_enabled(&workspace_root, &id, enabled)?;
                write_plugin_config_artifact(
                    &mut stdout,
                    if enabled { "Enabled" } else { "Disabled" },
                    &id,
                    &path,
                    enabled,
                )?;
            }
        },
    }
    stdout.flush()?;
    Ok(())
}

async fn run_read_only_command(
    workspace_root: PathBuf,
    core: CoreConfig,
    command: ReadOnlyCommand,
) -> Result<()> {
    let history = SessionHistoryClient::open(&core, &workspace_root).await?;
    emit_history_store_warning(&history);
    let mut stdout = io::stdout().lock();
    match command {
        ReadOnlyCommand::Sessions { query } => {
            if let Some(query) = query {
                let matches = history.search_sessions(&query).await?;
                write_session_search_results(&mut stdout, &matches)?;
            } else {
                let sessions = history.list_sessions().await?;
                write_session_summaries(&mut stdout, &sessions)?;
            }
        }
        ReadOnlyCommand::Session { selector } => {
            let session_ref = resolve_history_selector(&history, &selector, "inspect").await?;
            let summary = history
                .list_sessions()
                .await?
                .into_iter()
                .find(|summary| summary.session_ref == session_ref)
                .with_context(|| format!("missing session summary for {session_ref}"))?;
            let loaded = history.load_session(&session_ref).await?;
            write_loaded_session_details(&mut stdout, &summary, &loaded)?;
        }
        ReadOnlyCommand::AgentSessions { session_ref } => {
            let agent_sessions = history.list_agent_sessions(session_ref.as_deref()).await?;
            write_agent_session_summaries(&mut stdout, &agent_sessions)?;
        }
        ReadOnlyCommand::AgentSession { agent_session_ref } => {
            let loaded = history.load_agent_session(&agent_session_ref).await?;
            write_loaded_agent_session_details(&mut stdout, &loaded)?;
        }
        ReadOnlyCommand::Tasks { session_ref } => {
            let tasks = history.list_tasks(session_ref.as_deref()).await?;
            write_task_summaries(&mut stdout, &tasks)?;
        }
        ReadOnlyCommand::Task { task_ref } => {
            let loaded = history.load_task(&task_ref).await?;
            write_loaded_task_details(&mut stdout, &loaded)?;
        }
        ReadOnlyCommand::ExportArchive {
            selector,
            output_path,
        } => {
            let session_ref = resolve_history_selector(&history, &selector, "export").await?;
            let artifact = history
                .export_session_archive(&session_ref, &output_path)
                .await?;
            write_archive_artifact(&mut stdout, &artifact)?;
        }
        ReadOnlyCommand::ImportArchive { input_path } => {
            let artifact = history.import_session_archive(&input_path).await?;
            write_import_artifact(&mut stdout, &artifact)?;
        }
        ReadOnlyCommand::ExportSession {
            selector,
            output_path,
        } => {
            let session_ref = resolve_history_selector(&history, &selector, "export").await?;
            let artifact = history.export_session(&session_ref, &output_path).await?;
            write_export_artifact(&mut stdout, &artifact)?;
        }
        ReadOnlyCommand::ExportTranscript {
            selector,
            output_path,
        } => {
            let session_ref = resolve_history_selector(&history, &selector, "export").await?;
            let artifact = history
                .export_session_transcript(&session_ref, &output_path)
                .await?;
            write_export_artifact(&mut stdout, &artifact)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

async fn run_live_inspection_command(
    workspace_root: PathBuf,
    options: AppOptions,
    command: LiveInspectCommand,
) -> Result<()> {
    // These commands are operator-facing inspections of the live startup
    // surface, so build the same interactive session shape that the TUI uses
    // instead of the more restrictive headless one-shot variant.
    let session = build_session_with_approval_mode(
        &options,
        &workspace_root,
        SessionApprovalMode::Interactive,
    )
    .await?;
    let mut stdout = io::stdout().lock();
    match command {
        LiveInspectCommand::Diagnostics => {
            write_startup_diagnostics(&mut stdout, &session.startup_diagnostics())?;
        }
        LiveInspectCommand::Prompts => {
            let prompts = session.list_mcp_prompts().await;
            write_mcp_prompt_summaries(&mut stdout, &prompts)?;
        }
        LiveInspectCommand::Resources => {
            let resources = session.list_mcp_resources().await;
            write_mcp_resource_summaries(&mut stdout, &resources)?;
        }
    }
    stdout.flush()?;
    Ok(())
}

fn print_fatal_error(error: &anyhow::Error) {
    let _ = writeln!(io::stderr().lock(), "error: {error}");
    if should_render_diagnostic_details(error) {
        let _ = writeln!(
            io::stderr().lock(),
            "\ninternal diagnostic report:\n{error:?}"
        );
    }
}

fn should_render_diagnostic_details(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<agent::runtime::RuntimeError>()
            || cause.is::<agent::provider::ProviderError>()
            || cause.is::<agent::inference::InferenceError>()
    })
}

fn init_tracing(workspace_root: &Path) -> Result<WorkerGuard> {
    let layout = AgentWorkspaceLayout::new(workspace_root);
    layout.ensure_standard_layout().with_context(|| {
        format!(
            "failed to materialize workspace state layout at {}",
            layout.state_dir().display()
        )
    })?;
    let log_dir = layout.logs_dir();
    let file_appender = tracing_appender::rolling::never(log_dir, "code-agent.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = EnvFilter::try_new(agent_env::log_filter_or_default(
        "info,runtime=debug,provider=debug",
    ))
    .context("failed to parse tracing filter")?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;
    Ok(guard)
}

async fn async_main(
    workspace_root: PathBuf,
    options: AppOptions,
    launch_mode: LaunchMode,
) -> Result<()> {
    let mut options = options;
    let stdin_is_terminal = io::stdin().is_terminal();
    let stdout_is_terminal = io::stdout().is_terminal();
    if options.one_shot_prompt.is_none() && (!stdin_is_terminal || !stdout_is_terminal) {
        bail!(
            "code-agent requires a terminal for interactive mode; pass a prompt argument to run headless one-shot mode"
        );
    }
    confirm_unsandboxed_startup_if_needed(
        &workspace_root,
        &mut options,
        stdin_is_terminal && stdout_is_terminal,
    )?;
    // One-shot prompt invocations are also used from scripts and tests. When a
    // real terminal is unavailable, bypass the TUI so raw-mode setup does not
    // fail before the runtime can execute the prompt.
    if launch_headless_one_shot(&options, stdin_is_terminal, stdout_is_terminal) {
        return run_headless_one_shot(workspace_root, options, launch_mode).await;
    }

    let ui_state = SharedUiState::new();
    let workspace_name = workspace_root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace")
        .to_string();
    let model_reasoning_effort = options.primary_profile.reasoning_effort.clone();
    let mut loading_screen = StartupLoadingScreen::enter(
        workspace_name,
        options.primary_profile.model.model.clone(),
        model_reasoning_effort,
    )?;
    let session = build_session_with_approval_mode_and_progress(
        &options,
        &workspace_root,
        SessionApprovalMode::Interactive,
        |update| {
            let _ = loading_screen.apply(update);
        },
    )
    .await;
    loading_screen.leave()?;
    let session = session?;
    apply_launch_mode(&session, &launch_mode).await?;
    let session = CodeAgentUiSession::from(session);
    let exit_summary_session = session.clone();

    let result = CodeAgentTui::new(
        session,
        options.one_shot_prompt.clone(),
        ui_state,
        options.theme_catalog.clone(),
    )
    .run()
    .await;
    emit_ui_exit_summary(&exit_summary_session).await;
    result
}

fn launch_headless_one_shot(
    options: &AppOptions,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    options.one_shot_prompt.is_some() && (!stdin_is_terminal || !stdout_is_terminal)
}

async fn run_headless_one_shot(
    workspace_root: PathBuf,
    options: AppOptions,
    launch_mode: LaunchMode,
) -> Result<()> {
    let prompt = options
        .one_shot_prompt
        .clone()
        .context("headless one-shot mode requires a prompt")?;
    let session = build_session_with_approval_mode(
        &options,
        &workspace_root,
        SessionApprovalMode::NonInteractive,
    )
    .await?;
    apply_launch_mode(&session, &launch_mode).await?;
    let result = session.run_one_shot_prompt(&prompt).await;
    let end_reason = if result.is_ok() {
        "one_shot_complete"
    } else {
        "one_shot_failed"
    };
    let _ = session.end_session(Some(end_reason.to_string())).await;
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            emit_exit_summary(&session).await;
            return Err(error);
        }
    };
    if !outcome.assistant_text.is_empty() {
        let mut stdout = io::stdout().lock();
        stdout.write_all(outcome.assistant_text.as_bytes())?;
        if !outcome.assistant_text.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
        stdout.flush()?;
    }
    emit_exit_summary(&session).await;
    Ok(())
}

async fn apply_launch_mode(
    session: &code_agent_backend::CodeAgentSession,
    launch_mode: &LaunchMode,
) -> Result<()> {
    match launch_mode {
        LaunchMode::Default => Ok(()),
        LaunchMode::Resume(selector) => {
            let session_ref = resolve_session_selector(session, selector, "resume").await?;
            session.resume_persisted_session(&session_ref).await
        }
        LaunchMode::Fork(selector) => {
            let session_ref = resolve_session_selector(session, selector, "fork").await?;
            session.fork_persisted_session(&session_ref).await
        }
    }
}

async fn resolve_session_selector(
    session: &code_agent_backend::CodeAgentSession,
    selector: &SessionSelector,
    verb: &str,
) -> Result<String> {
    match selector {
        SessionSelector::Reference(session_ref) => Ok(session_ref.clone()),
        SessionSelector::Last => session
            .list_sessions()
            .await?
            .into_iter()
            .next()
            .map(|summary| summary.session_ref)
            .with_context(|| format!("no persisted sessions available to {verb}")),
    }
}

async fn resolve_history_selector(
    history: &SessionHistoryClient,
    selector: &SessionSelector,
    verb: &str,
) -> Result<String> {
    match selector {
        SessionSelector::Reference(session_ref) => history.resolve_session_ref(session_ref).await,
        SessionSelector::Last => history
            .resolve_last_session_ref()
            .await
            .with_context(|| format!("no persisted sessions available to {verb}")),
    }
}

fn emit_history_store_warning(history: &SessionHistoryClient) {
    let Some(warning) = history.store_warning() else {
        return;
    };
    let mut stderr = io::stderr().lock();
    if let Err(error) = writeln!(
        stderr,
        "warning: {warning}\nwarning: session history commands are using {}",
        history.store_label()
    ) {
        warn!(error = %error, "failed to print session history store warning");
    }
}

fn write_session_summaries(
    writer: &mut impl Write,
    sessions: &[PersistedSessionSummary],
) -> io::Result<()> {
    if sessions.is_empty() {
        writeln!(writer, "No persisted sessions found.")?;
        return Ok(());
    }

    for (index, summary) in sessions.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(
            writer,
            "{}  {}",
            summary.session_ref,
            session_title_or_prompt(summary)
        )?;
        writeln!(writer, "  {}", format_session_counts(summary))?;
    }
    Ok(())
}

fn write_session_search_results(
    writer: &mut impl Write,
    matches: &[PersistedSessionSearchMatch],
) -> io::Result<()> {
    if matches.is_empty() {
        writeln!(writer, "No persisted sessions matched.")?;
        return Ok(());
    }

    for (index, entry) in matches.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(
            writer,
            "{}  {}",
            entry.summary.session_ref,
            session_title_or_prompt(&entry.summary)
        )?;
        writeln!(
            writer,
            "  {} · {} matched events",
            format_session_counts(&entry.summary),
            entry.matched_event_count,
        )?;
        for preview in &entry.preview_matches {
            writeln!(writer, "  match: {preview}")?;
        }
    }
    Ok(())
}

fn write_agent_session_summaries(
    writer: &mut impl Write,
    agent_sessions: &[PersistedAgentSessionSummary],
) -> io::Result<()> {
    if agent_sessions.is_empty() {
        writeln!(writer, "No persisted agent sessions found.")?;
        return Ok(());
    }

    for (index, summary) in agent_sessions.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(
            writer,
            "{}  {}",
            summary.agent_session_ref,
            format_agent_session_heading(summary)
        )?;
        writeln!(
            writer,
            "  session={} · {} messages · {} events · resume={}",
            summary.session_ref,
            format_token_count(summary.transcript_message_count as u64),
            format_token_count(summary.event_count as u64),
            summary.resume_support.label(),
        )?;
    }
    Ok(())
}

fn write_task_summaries(writer: &mut impl Write, tasks: &[PersistedTaskSummary]) -> io::Result<()> {
    if tasks.is_empty() {
        writeln!(writer, "No persisted tasks found.")?;
        return Ok(());
    }

    for (index, summary) in tasks.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "{}  {}", summary.task_id, summary.summary)?;
        writeln!(
            writer,
            "  session={} · role={} · status={} · origin={}",
            summary.session_ref, summary.role, summary.status, summary.origin,
        )?;
    }
    Ok(())
}

fn write_loaded_session_details(
    writer: &mut impl Write,
    summary: &PersistedSessionSummary,
    loaded: &LoadedSession,
) -> io::Result<()> {
    writeln!(writer, "Session")?;
    writeln!(writer, "  ref: {}", summary.session_ref)?;
    if let Some(session_title) = summary.session_title.as_deref() {
        writeln!(writer, "  title: {session_title}")?;
    }
    if let Some(prompt) = summary.last_user_prompt.as_deref() {
        writeln!(writer, "  last prompt: {prompt}")?;
    }
    writeln!(writer, "  {}", format_session_counts(summary))?;
    if let Some(window) = loaded
        .token_usage
        .session
        .as_ref()
        .and_then(|session_usage| session_usage.ledger.context_window)
    {
        writeln!(
            writer,
            "  context: {} / {}",
            format_token_count(window.used_tokens as u64),
            format_token_count(window.max_tokens as u64),
        )?;
    }
    if !loaded.token_usage.aggregate_usage.is_zero() {
        let usage = loaded.token_usage.aggregate_usage;
        writeln!(
            writer,
            "  total tokens: in={} out={} prefill={} decode={} cache={}{}",
            format_token_count(usage.input_tokens),
            format_token_count(usage.output_tokens),
            format_token_count(usage.uncached_input_tokens()),
            format_token_count(usage.visible_decode_tokens()),
            format_token_count(usage.cache_read_tokens),
            format_reasoning_field_suffix(usage.reasoning_tokens),
        )?;
    }
    if !loaded.agent_session_ids.is_empty() {
        writeln!(
            writer,
            "  runtime sessions: {}",
            loaded
                .agent_session_ids
                .iter()
                .map(|agent_session_id| agent_session_id.as_str().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )?;
    }
    if loaded.transcript.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Transcript")?;
        writeln!(writer, "  <empty>")?;
        return Ok(());
    }

    writeln!(writer)?;
    writeln!(writer, "Transcript")?;
    for (index, message) in loaded.transcript.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "{}", message_to_text(message))?;
    }
    Ok(())
}

fn write_loaded_agent_session_details(
    writer: &mut impl Write,
    loaded: &LoadedAgentSession,
) -> io::Result<()> {
    let summary = &loaded.summary;
    writeln!(writer, "Agent Session")?;
    writeln!(writer, "  ref: {}", summary.agent_session_ref)?;
    writeln!(writer, "  session: {}", summary.session_ref)?;
    writeln!(writer, "  label: {}", summary.label)?;
    writeln!(writer, "  resume: {}", summary.resume_support.label())?;
    if let Some(session_title) = summary.session_title.as_deref() {
        writeln!(writer, "  session title: {session_title}")?;
    }
    if let Some(prompt) = summary.last_user_prompt.as_deref() {
        writeln!(writer, "  last prompt: {prompt}")?;
    }
    writeln!(
        writer,
        "  counts: {} messages · {} events",
        format_token_count(summary.transcript_message_count as u64),
        format_token_count(summary.event_count as u64),
    )?;
    if let Some(token_usage) = &loaded.token_usage {
        if let Some(window) = token_usage.ledger.context_window {
            writeln!(
                writer,
                "  context: {} / {}",
                format_token_count(window.used_tokens as u64),
                format_token_count(window.max_tokens as u64),
            )?;
        }
        writeln!(
            writer,
            "  tokens: in={} out={} cache={}{}",
            format_token_count(token_usage.ledger.cumulative_usage.input_tokens),
            format_token_count(token_usage.ledger.cumulative_usage.output_tokens),
            format_token_count(token_usage.ledger.cumulative_usage.cache_read_tokens),
            format_reasoning_field_suffix(token_usage.ledger.cumulative_usage.reasoning_tokens),
        )?;
    }
    if !loaded.subagents.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Spawned Subagents")?;
        for subagent in &loaded.subagents {
            writeln!(
                writer,
                "  {} role={} status={} summary={}",
                subagent.handle.agent_session_id,
                subagent.task.role,
                subagent.status,
                subagent.summary,
            )?;
        }
    }
    if loaded.transcript.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Transcript")?;
        writeln!(writer, "  <empty>")?;
        return Ok(());
    }

    writeln!(writer)?;
    writeln!(writer, "Transcript")?;
    for (index, message) in loaded.transcript.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "{}", message_to_text(message))?;
    }
    Ok(())
}

fn write_loaded_task_details(writer: &mut impl Write, loaded: &LoadedTask) -> io::Result<()> {
    let summary = &loaded.summary;
    writeln!(writer, "Task")?;
    writeln!(writer, "  id: {}", summary.task_id)?;
    writeln!(writer, "  session: {}", summary.session_ref)?;
    writeln!(
        writer,
        "  parent agent session: {}",
        summary.parent_agent_session_ref
    )?;
    writeln!(writer, "  role: {}", summary.role)?;
    writeln!(writer, "  origin: {}", summary.origin)?;
    writeln!(writer, "  status: {}", summary.status)?;
    writeln!(writer, "  summary: {}", summary.summary)?;
    writeln!(writer, "  prompt: {}", loaded.spec.prompt)?;
    if let Some(steer) = loaded.spec.steer.as_deref() {
        writeln!(writer, "  steer: {steer}")?;
    }
    if let Some(child_session_ref) = summary.child_session_ref.as_deref() {
        writeln!(writer, "  child session: {child_session_ref}")?;
    }
    if let Some(child_agent_session_ref) = summary.child_agent_session_ref.as_deref() {
        writeln!(writer, "  child agent session: {child_agent_session_ref}")?;
    }
    if let Some(token_usage) = &loaded.token_usage {
        if let Some(window) = token_usage.ledger.context_window {
            writeln!(
                writer,
                "  context: {} / {}",
                format_token_count(window.used_tokens as u64),
                format_token_count(window.max_tokens as u64),
            )?;
        }
        writeln!(
            writer,
            "  tokens: in={} out={} cache={}{}",
            format_token_count(token_usage.ledger.cumulative_usage.input_tokens),
            format_token_count(token_usage.ledger.cumulative_usage.output_tokens),
            format_token_count(token_usage.ledger.cumulative_usage.cache_read_tokens),
            format_reasoning_field_suffix(token_usage.ledger.cumulative_usage.reasoning_tokens),
        )?;
    }
    if let Some(result) = &loaded.result {
        writeln!(writer)?;
        writeln!(writer, "Result")?;
        writeln!(writer, "  status: {}", result.status)?;
        writeln!(writer, "  summary: {}", result.summary)?;
        if !result.claimed_files.is_empty() {
            writeln!(
                writer,
                "  claimed files: {}",
                result.claimed_files.join(", ")
            )?;
        }
    }
    if let Some(error) = loaded.error.as_deref() {
        writeln!(writer)?;
        writeln!(writer, "Error")?;
        writeln!(writer, "  {error}")?;
    }
    if !loaded.artifacts.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Artifacts")?;
        for artifact in &loaded.artifacts {
            writeln!(writer, "  {} {}", artifact.kind, artifact.uri)?;
        }
    }
    if !loaded.messages.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Agent Messages")?;
        for message in &loaded.messages {
            writeln!(writer, "{}", message_to_text(&message.message))?;
        }
    }
    if !loaded.child_transcript.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Child Transcript")?;
        for (index, message) in loaded.child_transcript.iter().enumerate() {
            if index > 0 {
                writeln!(writer)?;
            }
            writeln!(writer, "{}", message_to_text(message))?;
        }
    }
    Ok(())
}

fn write_export_artifact(
    writer: &mut impl Write,
    artifact: &SessionExportArtifact,
) -> io::Result<()> {
    let kind = match artifact.kind {
        SessionExportKind::EventsJsonl => "events",
        SessionExportKind::TranscriptText => "transcript",
    };
    writeln!(
        writer,
        "Exported {kind} for {} to {} ({} items).",
        artifact.session_id,
        artifact.output_path.display(),
        artifact.item_count,
    )
}

fn write_mcp_management_artifact(
    writer: &mut impl Write,
    verb: &str,
    name: &str,
    config_path: &Path,
    enabled: Option<bool>,
) -> io::Result<()> {
    match enabled {
        Some(enabled) => writeln!(
            writer,
            "{verb} MCP server `{name}` in {} (enabled: {}).",
            config_path.display(),
            enabled,
        ),
        None => writeln!(
            writer,
            "{verb} MCP server `{name}` from {}.",
            config_path.display(),
        ),
    }
}

fn write_skill_management_artifact(
    writer: &mut impl Write,
    verb: &str,
    artifact: &ManagedSkillArtifact,
) -> io::Result<()> {
    writeln!(
        writer,
        "{verb} managed skill `{}` at {} (enabled: {}).",
        artifact.skill_name,
        artifact.skill_path.display(),
        artifact.enabled,
    )
}

fn write_plugin_copy_artifact(
    writer: &mut impl Write,
    verb: &str,
    artifact: &ManagedPluginArtifact,
) -> io::Result<()> {
    writeln!(
        writer,
        "{verb} managed plugin `{}` at {} (enabled: {}).",
        artifact.plugin_id,
        artifact.plugin_path.display(),
        artifact.enabled,
    )
}

fn write_plugin_config_artifact(
    writer: &mut impl Write,
    verb: &str,
    plugin_id: &str,
    config_path: &Path,
    enabled: bool,
) -> io::Result<()> {
    writeln!(
        writer,
        "{verb} plugin `{plugin_id}` in {} (enabled: {}).",
        config_path.display(),
        enabled,
    )
}

fn write_archive_artifact(
    writer: &mut impl Write,
    artifact: &SessionArchiveArtifact,
) -> io::Result<()> {
    writeln!(
        writer,
        "Exported archive for {} to {} ({} sessions, {} events, {} notes).",
        artifact.root_session_id,
        artifact.output_path.display(),
        artifact.session_count,
        artifact.event_count,
        artifact.session_note_count,
    )
}

fn write_import_artifact(
    writer: &mut impl Write,
    artifact: &SessionImportArtifact,
) -> io::Result<()> {
    writeln!(
        writer,
        "Imported archive {} for {} ({} sessions, {} events, {} notes).",
        artifact.input_path.display(),
        artifact.root_session_id,
        artifact.session_count,
        artifact.event_count,
        artifact.session_note_count,
    )
}

fn write_startup_diagnostics(
    writer: &mut impl Write,
    snapshot: &StartupDiagnosticsSnapshot,
) -> io::Result<()> {
    writeln!(writer, "Startup Diagnostics")?;
    writeln!(
        writer,
        "  tools: {} local · {} mcp",
        snapshot.local_tool_count, snapshot.mcp_tool_count,
    )?;
    writeln!(
        writer,
        "  plugins: {} enabled / {} total",
        snapshot.enabled_plugin_count, snapshot.total_plugin_count,
    )?;
    writeln!(writer, "  mcp servers: {}", snapshot.mcp_servers.len())?;

    if !snapshot.mcp_servers.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "MCP Servers")?;
        for server in &snapshot.mcp_servers {
            write_mcp_server_summary_line(writer, server)?;
        }
    }
    if !snapshot.plugin_details.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Plugin Details")?;
        for detail in &snapshot.plugin_details {
            writeln!(writer, "  {detail}")?;
        }
    }
    if !snapshot.warnings.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Warnings")?;
        for warning in &snapshot.warnings {
            writeln!(writer, "  {warning}")?;
        }
    }
    if !snapshot.diagnostics.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "Diagnostics")?;
        for diagnostic in &snapshot.diagnostics {
            writeln!(writer, "  {diagnostic}")?;
        }
    }
    Ok(())
}

fn write_mcp_server_summary_line(
    writer: &mut impl Write,
    summary: &McpServerSummary,
) -> io::Result<()> {
    writeln!(
        writer,
        "{}  tools={} · prompts={} · resources={}",
        summary.server_name, summary.tool_count, summary.prompt_count, summary.resource_count,
    )
}

fn write_mcp_prompt_summaries(
    writer: &mut impl Write,
    prompts: &[McpPromptSummary],
) -> io::Result<()> {
    if prompts.is_empty() {
        writeln!(writer, "No MCP prompts available.")?;
        return Ok(());
    }
    for (index, summary) in prompts.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "{}/{}", summary.server_name, summary.prompt_name)?;
        let arguments = if summary.argument_names.is_empty() {
            "none".to_string()
        } else {
            summary.argument_names.join(", ")
        };
        writeln!(writer, "  args={}", arguments)?;
        if !summary.description.trim().is_empty() {
            writeln!(writer, "  {}", summary.description.trim())?;
        }
    }
    Ok(())
}

fn write_mcp_resource_summaries(
    writer: &mut impl Write,
    resources: &[McpResourceSummary],
) -> io::Result<()> {
    if resources.is_empty() {
        writeln!(writer, "No MCP resources available.")?;
        return Ok(());
    }
    for (index, summary) in resources.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "{}  {}", summary.server_name, summary.uri)?;
        let mime = summary.mime_type.as_deref().unwrap_or("unknown");
        writeln!(writer, "  mime={mime}")?;
        if !summary.description.trim().is_empty() {
            writeln!(writer, "  {}", summary.description.trim())?;
        }
    }
    Ok(())
}

fn format_agent_session_heading(summary: &PersistedAgentSessionSummary) -> String {
    match (
        summary.session_title.as_deref(),
        summary.last_user_prompt.as_deref(),
    ) {
        (Some(title), Some(_prompt)) => format!("{} · {}", summary.label, title),
        (Some(title), None) => format!("{} · {}", summary.label, title),
        (None, Some(prompt)) => format!("{} · {}", summary.label, prompt),
        (None, None) => summary.label.clone(),
    }
}

fn session_title_or_prompt(summary: &PersistedSessionSummary) -> &str {
    summary
        .session_title
        .as_deref()
        .or(summary.last_user_prompt.as_deref())
        .unwrap_or("no prompt yet")
}

fn format_session_counts(summary: &PersistedSessionSummary) -> String {
    let mut rendered = format!(
        "{} messages · {} events · {} agent sessions",
        format_token_count(summary.transcript_message_count as u64),
        format_token_count(summary.event_count as u64),
        format_token_count(summary.worker_session_count as u64),
    );
    if let Some(token_usage) = summary
        .token_usage
        .as_ref()
        .filter(|token_usage| !token_usage.is_zero())
    {
        rendered.push_str(&format!(
            " · tokens in={} out={} cache={}{}",
            format_token_count(token_usage.cumulative_usage.input_tokens),
            format_token_count(token_usage.cumulative_usage.output_tokens),
            format_token_count(token_usage.cumulative_usage.cache_read_tokens),
            format_reasoning_field_suffix(token_usage.cumulative_usage.reasoning_tokens),
        ));
    }
    rendered
}

async fn emit_ui_exit_summary(session: &CodeAgentUiSession) {
    let startup: SessionStartupSnapshot = session.query(UIQuery::StartupSnapshot);
    match session
        .run::<LoadedSession>(UIAsyncCommand::LoadSession {
            session_ref: startup.active_session_ref.clone(),
        })
        .await
    {
        Ok(loaded) => emit_loaded_session_exit_summary(&startup.active_session_ref, &loaded),
        Err(error) => warn!(error = %error, "failed to load UI exit summary"),
    }
}

async fn emit_exit_summary(session: &code_agent_backend::CodeAgentSession) {
    let startup = session.startup_snapshot();
    match session.load_session(&startup.active_session_ref).await {
        Ok(loaded) => emit_loaded_session_exit_summary(&startup.active_session_ref, &loaded),
        Err(error) => warn!(error = %error, "failed to load exit summary"),
    }
}

fn emit_loaded_session_exit_summary(session_ref: &str, loaded: &LoadedSession) {
    let mut stderr = io::stderr().lock();
    if let Err(error) = write_exit_summary(
        &mut stderr,
        current_program_name(),
        session_ref,
        &loaded.token_usage,
    ) {
        warn!(error = %error, "failed to print exit summary");
    }
}

fn write_exit_summary(
    writer: &mut impl Write,
    program_name: String,
    session_ref: &str,
    token_usage: &SessionTokenUsageReport,
) -> io::Result<()> {
    let usage = token_usage.aggregate_usage;
    let total_tokens = usage.visible_total_tokens();
    writeln!(
        writer,
        "Token usage: total={} input={}{} output={}{}",
        format_token_count(total_tokens),
        format_token_count(usage.uncached_input_tokens()),
        format_cached_suffix(usage.cache_read_tokens),
        format_token_count(usage.output_tokens),
        format_reasoning_suffix(usage.reasoning_tokens),
    )?;
    writeln!(
        writer,
        "To continue this session, run {} resume {}",
        program_name, session_ref
    )?;
    writeln!(
        writer,
        "To fork from this session, run {} fork {}",
        program_name, session_ref
    )?;
    writer.flush()
}

fn current_program_name() -> String {
    env::args()
        .next()
        .and_then(|value| {
            Path::new(&value)
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "code-agent".to_string())
}

fn format_cached_suffix(cache_read_tokens: u64) -> String {
    if cache_read_tokens == 0 {
        String::new()
    } else {
        format!(" (+ {} cached)", format_token_count(cache_read_tokens))
    }
}

fn format_reasoning_suffix(reasoning_tokens: u64) -> String {
    if reasoning_tokens == 0 {
        String::new()
    } else {
        format!(" (reasoning {})", format_token_count(reasoning_tokens))
    }
}

fn format_reasoning_field_suffix(reasoning_tokens: u64) -> String {
    if reasoning_tokens == 0 {
        String::new()
    } else {
        format!(" reasoning={}", format_token_count(reasoning_tokens))
    }
}

fn format_token_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SandboxFallbackAction {
    Continue,
    Prompt,
    Abort,
}

fn confirm_unsandboxed_startup_if_needed(
    workspace_root: &Path,
    options: &mut AppOptions,
    interactive_terminal: bool,
) -> Result<()> {
    let preflight = inspect_sandbox_preflight(workspace_root, options);
    let Some(notice) = build_sandbox_fallback_notice(&preflight) else {
        return Ok(());
    };
    match choose_sandbox_fallback_action(options, interactive_terminal) {
        SandboxFallbackAction::Continue => {
            options.sandbox_fail_if_unavailable = false;
            print_sandbox_fallback_notice(
                &notice,
                "Continuing because --allow-no-sandbox was set.\n",
            )?;
            Ok(())
        }
        SandboxFallbackAction::Prompt => {
            if operator_confirms_unsandboxed_startup(&notice)? {
                options.sandbox_fail_if_unavailable = false;
                Ok(())
            } else {
                bail!("aborted because sandbox enforcement is unavailable on this host")
            }
        }
        SandboxFallbackAction::Abort => bail!(format_sandbox_abort_message(&notice)),
    }
}

fn choose_sandbox_fallback_action(
    options: &AppOptions,
    interactive_terminal: bool,
) -> SandboxFallbackAction {
    if options.allow_no_sandbox {
        SandboxFallbackAction::Continue
    } else if interactive_terminal {
        SandboxFallbackAction::Prompt
    } else {
        // Headless invocations cannot answer a startup risk prompt, so require
        // an explicit CLI override instead of silently inheriting host fallback.
        SandboxFallbackAction::Abort
    }
}

fn print_sandbox_fallback_notice(notice: &SandboxFallbackNotice, trailer: &str) -> Result<()> {
    let mut stderr = io::stderr().lock();
    writeln!(
        stderr,
        "warning: sandbox backend unavailable for the configured runtime policy"
    )?;
    writeln!(stderr, "  policy: {}", notice.policy_summary)?;
    writeln!(stderr, "  reason: {}", notice.reason)?;
    writeln!(stderr, "  risk: {}", notice.risk_summary)?;
    writeln!(stderr, "  setup:")?;
    for (index, step) in notice.setup_steps.iter().enumerate() {
        writeln!(stderr, "    {}. {}", index + 1, step)?;
    }
    write!(stderr, "{trailer}")?;
    stderr.flush()?;
    Ok(())
}

fn operator_confirms_unsandboxed_startup(notice: &SandboxFallbackNotice) -> Result<bool> {
    confirm_unsandboxed_startup_screen(notice)
        .context("failed to render sandbox confirmation screen")
}

fn format_sandbox_abort_message(notice: &SandboxFallbackNotice) -> String {
    let mut lines = vec![
        "sandbox backend unavailable for the configured runtime policy".to_string(),
        format!("policy: {}", notice.policy_summary),
        format!("reason: {}", notice.reason),
        format!("risk: {}", notice.risk_summary),
        "setup:".to_string(),
    ];
    lines.extend(
        notice
            .setup_steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("  {}. {}", index + 1, step)),
    );
    lines.push(
        "rerun in a terminal to confirm explicitly, or pass --allow-no-sandbox to accept the risk for this invocation".to_string(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod diagnostic_tests {
    use super::should_render_diagnostic_details;
    use anyhow::anyhow;

    #[test]
    fn internal_runtime_errors_request_diagnostic_output() {
        let error = anyhow::Error::from(agent::runtime::RuntimeError::model_backend(
            "provider transport failed",
        ));
        assert!(should_render_diagnostic_details(&error));
    }

    #[test]
    fn plain_operator_errors_stay_concise() {
        let error = anyhow!("missing prompt");
        assert!(!should_render_diagnostic_details(&error));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, ExplicitExecCommand, LaunchMode, LiveInspectCommand, ManagementCommand,
        McpManagementCommand, PluginManagementCommand, ReadOnlyCommand, SandboxFallbackAction,
        SessionSelector, SkillManagementCommand, choose_sandbox_fallback_action,
        format_sandbox_abort_message, format_session_counts, format_token_count,
        launch_headless_one_shot, write_exit_summary, write_mcp_prompt_summaries,
        write_mcp_resource_summaries, write_session_search_results, write_session_summaries,
        write_startup_diagnostics,
    };
    use agent::DriverActivationOutcome;
    use agent::ToolExecutionContext;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent::runtime::SubagentProfileResolver;
    use agent::tools::{NetworkPolicy, SandboxMode, SubagentLaunchSpec};
    use agent::types::{AgentTaskSpec, HookEvent, HookHandler, HookRegistration, HttpHookHandler};
    use agent::types::{SessionSummaryTokenUsage, TokenUsage};
    use agent_env::EnvMap;
    use clap::Parser;
    use code_agent_backend::{
        AppOptions, CodeAgentSubagentProfileResolver, McpPromptSummary, McpResourceSummary,
        McpServerSummary, PersistedSessionSearchMatch, PersistedSessionSummary, ResumeSupport,
        SandboxFallbackNotice, StartupDiagnosticsSnapshot, build_sandbox_policy, dedup_mcp_servers,
        driver_host_output_lines, merge_driver_host_inputs, parse_bool_flag, resolve_mcp_servers,
        tool_context_for_profile,
    };
    use nanoclaw_config::{
        AgentProfileConfig, AgentSandboxMode, CoreConfig, ModelCapabilitiesConfig, ModelConfig,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    use store::SessionTokenUsageReport;
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn parses_boolean_flag_values() {
        assert!(parse_bool_flag("true").unwrap());
        assert!(!parse_bool_flag("off").unwrap());
        assert!(parse_bool_flag("1").unwrap());
        assert!(parse_bool_flag("maybe").is_err());
    }

    fn persisted_summary(session_ref: &str) -> PersistedSessionSummary {
        PersistedSessionSummary {
            session_ref: session_ref.to_string(),
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            event_count: 12,
            worker_session_count: 3,
            transcript_message_count: 7,
            session_title: Some("Session title".to_string()),
            last_user_prompt: Some("inspect repository".to_string()),
            token_usage: Some(SessionSummaryTokenUsage {
                context_window: None,
                cumulative_usage: TokenUsage {
                    input_tokens: 1_234,
                    output_tokens: 56,
                    prefill_tokens: 445,
                    decode_tokens: 56,
                    cache_read_tokens: 789,
                    reasoning_tokens: 12,
                },
            }),
            resume_support: ResumeSupport::Reattachable,
        }
    }

    #[test]
    fn clap_parses_resume_last() {
        let cli = Cli::parse_from(["code-agent", "--allow-no-sandbox", "resume", "--last"]);

        assert!(cli.allow_no_sandbox);
        assert_eq!(
            cli.launch_mode().unwrap(),
            LaunchMode::Resume(SessionSelector::Last)
        );
        assert_eq!(
            cli.app_option_args(),
            vec!["--allow-no-sandbox".to_string()]
        );
    }

    #[test]
    fn clap_parses_exec_prompt() {
        let cli = Cli::parse_from(["code-agent", "exec", "inspect", "repository"]);

        assert_eq!(
            cli.explicit_exec_command().unwrap(),
            Some(ExplicitExecCommand {
                launch_mode: LaunchMode::Default,
                prompt: "inspect repository".to_string(),
            })
        );
    }

    #[test]
    fn clap_parses_mcp_add_stdio_with_trailing_command() {
        let cli = Cli::parse_from([
            "code-agent",
            "mcp",
            "add",
            "docs",
            "--type",
            "stdio",
            "--env",
            "TOKEN=secret",
            "--",
            "npx",
            "-y",
            "remote-mcp",
        ]);

        let Some(ManagementCommand::Mcp(McpManagementCommand::Add { server })) =
            cli.management_command().unwrap()
        else {
            panic!("expected MCP management command");
        };

        assert_eq!(server.name.as_str(), "docs");
        assert!(server.enabled);
        assert_eq!(
            server.transport,
            McpTransportConfig::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "remote-mcp".to_string()],
                env: BTreeMap::from([("TOKEN".to_string(), "secret".to_string())]),
                cwd: None,
            }
        );
    }

    #[test]
    fn clap_parses_mcp_disable() {
        let cli = Cli::parse_from(["code-agent", "mcp", "disable", "docs"]);

        assert_eq!(
            cli.management_command().unwrap(),
            Some(ManagementCommand::Mcp(McpManagementCommand::SetEnabled {
                name: "docs".to_string(),
                enabled: false,
            }))
        );
    }

    #[test]
    fn clap_parses_skill_add() {
        let cli = Cli::parse_from(["code-agent", "skill", "add", "./skills/review"]);

        assert_eq!(
            cli.management_command().unwrap(),
            Some(ManagementCommand::Skill(SkillManagementCommand::Add {
                path: "./skills/review".to_string(),
            }))
        );
    }

    #[test]
    fn clap_parses_skill_disable() {
        let cli = Cli::parse_from(["code-agent", "skill", "disable", "review"]);

        assert_eq!(
            cli.management_command().unwrap(),
            Some(ManagementCommand::Skill(
                SkillManagementCommand::SetEnabled {
                    name: "review".to_string(),
                    enabled: false,
                }
            ))
        );
    }

    #[test]
    fn clap_parses_plugin_add() {
        let cli = Cli::parse_from(["code-agent", "plugin", "add", "./plugins/review-policy"]);

        assert_eq!(
            cli.management_command().unwrap(),
            Some(ManagementCommand::Plugin(PluginManagementCommand::Add {
                path: "./plugins/review-policy".to_string(),
            }))
        );
    }

    #[test]
    fn clap_parses_plugin_enable() {
        let cli = Cli::parse_from(["code-agent", "plugin", "enable", "review-policy"]);

        assert_eq!(
            cli.management_command().unwrap(),
            Some(ManagementCommand::Plugin(
                PluginManagementCommand::SetEnabled {
                    id: "review-policy".to_string(),
                    enabled: true,
                }
            ))
        );
    }

    #[test]
    fn clap_parses_exec_resume_last() {
        let cli = Cli::parse_from(["code-agent", "exec", "resume", "--last", "continue"]);

        assert_eq!(
            cli.explicit_exec_command().unwrap(),
            Some(ExplicitExecCommand {
                launch_mode: LaunchMode::Resume(SessionSelector::Last),
                prompt: "continue".to_string(),
            })
        );
    }

    #[test]
    fn clap_routes_resume_last_prompt_tail() {
        let cli = Cli::parse_from(["code-agent", "resume", "--last", "continue", "from", "here"]);

        assert_eq!(
            cli.launch_mode().unwrap(),
            LaunchMode::Resume(SessionSelector::Last)
        );
        assert_eq!(
            cli.app_option_args(),
            vec![
                "continue".to_string(),
                "from".to_string(),
                "here".to_string(),
            ]
        );
    }

    #[test]
    fn clap_parses_fork_session_reference() {
        let cli = Cli::parse_from(["code-agent", "fork", "019d8aae-c699-75c3-b9de-6890b6f4d21a"]);

        assert_eq!(
            cli.launch_mode().unwrap(),
            LaunchMode::Fork(SessionSelector::Reference(
                "019d8aae-c699-75c3-b9de-6890b6f4d21a".to_string()
            ))
        );
    }

    #[test]
    fn clap_parses_exec_fork_reference() {
        let cli = Cli::parse_from([
            "code-agent",
            "exec",
            "fork",
            "019d8aae-c699-75c3-b9de-6890b6f4d21a",
            "summarize",
            "changes",
        ]);

        assert_eq!(
            cli.explicit_exec_command().unwrap(),
            Some(ExplicitExecCommand {
                launch_mode: LaunchMode::Fork(SessionSelector::Reference(
                    "019d8aae-c699-75c3-b9de-6890b6f4d21a".to_string()
                )),
                prompt: "summarize changes".to_string(),
            })
        );
    }

    #[test]
    fn clap_routes_resume_prompt_tail_after_explicit_session_id() {
        let cli = Cli::parse_from([
            "code-agent",
            "resume",
            "019d8aae-c699-75c3-b9de-6890b6f4d21a",
            "continue",
            "from",
            "here",
        ]);

        assert_eq!(
            cli.launch_mode().unwrap(),
            LaunchMode::Resume(SessionSelector::Reference(
                "019d8aae-c699-75c3-b9de-6890b6f4d21a".to_string()
            ))
        );
        assert_eq!(
            cli.app_option_args(),
            vec![
                "continue".to_string(),
                "from".to_string(),
                "here".to_string(),
            ]
        );
    }

    #[test]
    fn clap_parses_sessions_search_query() {
        let cli = Cli::parse_from(["code-agent", "sessions", "recent", "planner", "work"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::Sessions {
                query: Some("recent planner work".to_string())
            })
        );
    }

    #[test]
    fn clap_parses_session_last_lookup() {
        let cli = Cli::parse_from(["code-agent", "session", "--last"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::Session {
                selector: SessionSelector::Last
            })
        );
    }

    #[test]
    fn clap_parses_agent_sessions_filter() {
        let cli = Cli::parse_from(["code-agent", "agent-sessions", "session-a"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::AgentSessions {
                session_ref: Some("session-a".to_string()),
            })
        );
    }

    #[test]
    fn clap_parses_agent_session_lookup() {
        let cli = Cli::parse_from(["code-agent", "agent-session", "agent-session-a"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::AgentSession {
                agent_session_ref: "agent-session-a".to_string(),
            })
        );
    }

    #[test]
    fn clap_parses_tasks_filter() {
        let cli = Cli::parse_from(["code-agent", "tasks", "session-a"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::Tasks {
                session_ref: Some("session-a".to_string()),
            })
        );
    }

    #[test]
    fn clap_parses_task_lookup() {
        let cli = Cli::parse_from(["code-agent", "task", "task-a"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::Task {
                task_ref: "task-a".to_string(),
            })
        );
    }

    #[test]
    fn clap_parses_export_archive_with_relative_path() {
        let cli = Cli::parse_from([
            "code-agent",
            "export",
            "019d8aae-c699-75c3-b9de-6890b6f4d21a",
            "tmp/session-archive.json",
        ]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::ExportArchive {
                selector: SessionSelector::Reference(
                    "019d8aae-c699-75c3-b9de-6890b6f4d21a".to_string()
                ),
                output_path: "tmp/session-archive.json".to_string(),
            })
        );
    }

    #[test]
    fn clap_parses_import_archive_path() {
        let cli = Cli::parse_from(["code-agent", "import", "tmp/session-archive.json"]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::ImportArchive {
                input_path: "tmp/session-archive.json".to_string(),
            })
        );
    }

    #[test]
    fn clap_parses_export_transcript_with_relative_path() {
        let cli = Cli::parse_from([
            "code-agent",
            "export-transcript",
            "019d8aae-c699-75c3-b9de-6890b6f4d21a",
            "tmp/session.txt",
        ]);

        assert_eq!(
            cli.read_only_command().unwrap(),
            Some(ReadOnlyCommand::ExportTranscript {
                selector: SessionSelector::Reference(
                    "019d8aae-c699-75c3-b9de-6890b6f4d21a".to_string()
                ),
                output_path: "tmp/session.txt".to_string(),
            })
        );
    }

    #[test]
    fn clap_parses_diagnostics_live_command() {
        let cli = Cli::parse_from(["code-agent", "diagnostics"]);

        assert_eq!(
            cli.live_inspect_command(),
            Some(LiveInspectCommand::Diagnostics)
        );
    }

    #[test]
    fn clap_parses_resources_live_command() {
        let cli = Cli::parse_from(["code-agent", "resources"]);

        assert_eq!(
            cli.live_inspect_command(),
            Some(LiveInspectCommand::Resources)
        );
    }

    #[test]
    fn token_counts_render_with_grouping() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(965650), "965,650");
        assert_eq!(format_token_count(2672768), "2,672,768");
    }

    #[test]
    fn session_count_summary_includes_grouped_token_usage() {
        assert_eq!(
            format_session_counts(&persisted_summary("session-a")),
            "7 messages · 12 events · 3 agent sessions · tokens in=1,234 out=56 cache=789 reasoning=12"
        );
    }

    #[test]
    fn session_summary_writer_renders_full_ids() {
        let mut output = Vec::new();
        write_session_summaries(&mut output, &[persisted_summary("session-a")]).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert_eq!(
            rendered,
            concat!(
                "session-a  Session title\n",
                "  7 messages · 12 events · 3 agent sessions · tokens in=1,234 out=56 cache=789 reasoning=12\n",
            )
        );
    }

    #[test]
    fn session_search_writer_includes_match_previews() {
        let mut output = Vec::new();
        write_session_search_results(
            &mut output,
            &[PersistedSessionSearchMatch {
                summary: persisted_summary("session-a"),
                matched_event_count: 2,
                preview_matches: vec![
                    "session title: Session title".to_string(),
                    "user> inspect repository".to_string(),
                ],
            }],
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert_eq!(
            rendered,
            concat!(
                "session-a  Session title\n",
                "  7 messages · 12 events · 3 agent sessions · tokens in=1,234 out=56 cache=789 reasoning=12 · 2 matched events\n",
                "  match: session title: Session title\n",
                "  match: user> inspect repository\n",
            )
        );
    }

    #[test]
    fn exit_summary_prints_resume_and_fork_hints() {
        let mut output = Vec::new();
        write_exit_summary(
            &mut output,
            "nanoclaw".to_string(),
            "019d8aae-c699-75c3-b9de-6890b6f4d21a",
            &SessionTokenUsageReport {
                aggregate_usage: TokenUsage {
                    input_tokens: 3_586_997,
                    output_tokens: 51_421,
                    prefill_tokens: 914_229,
                    decode_tokens: 51_421,
                    cache_read_tokens: 2_672_768,
                    reasoning_tokens: 24_457,
                },
                ..Default::default()
            },
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert_eq!(
            rendered,
            concat!(
                "Token usage: total=965,650 input=914,229 (+ 2,672,768 cached) output=51,421 (reasoning 24,457)\n",
                "To continue this session, run nanoclaw resume 019d8aae-c699-75c3-b9de-6890b6f4d21a\n",
                "To fork from this session, run nanoclaw fork 019d8aae-c699-75c3-b9de-6890b6f4d21a\n",
            )
        );
    }

    #[test]
    fn startup_diagnostics_writer_lists_sections() {
        let mut output = Vec::new();
        write_startup_diagnostics(
            &mut output,
            &StartupDiagnosticsSnapshot {
                local_tool_count: 12,
                mcp_tool_count: 3,
                enabled_plugin_count: 1,
                total_plugin_count: 2,
                mcp_servers: vec![McpServerSummary {
                    server_name: "memory".to_string(),
                    tool_count: 2,
                    prompt_count: 1,
                    resource_count: 4,
                }],
                plugin_details: vec!["memory slot: workspace".to_string()],
                warnings: vec!["stdio server skipped".to_string()],
                diagnostics: vec!["managed code intel disabled".to_string()],
            },
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Startup Diagnostics"));
        assert!(rendered.contains("tools: 12 local · 3 mcp"));
        assert!(rendered.contains("MCP Servers"));
        assert!(rendered.contains("memory  tools=2 · prompts=1 · resources=4"));
        assert!(rendered.contains("Warnings"));
        assert!(rendered.contains("Diagnostics"));
    }

    #[test]
    fn mcp_prompt_writer_renders_arguments_and_description() {
        let mut output = Vec::new();
        write_mcp_prompt_summaries(
            &mut output,
            &[McpPromptSummary {
                server_name: "memory".to_string(),
                prompt_name: "summarize".to_string(),
                description: "Summarize the active note".to_string(),
                argument_names: vec!["path*".to_string(), "limit".to_string()],
            }],
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert_eq!(
            rendered,
            concat!(
                "memory/summarize\n",
                "  args=path*, limit\n",
                "  Summarize the active note\n",
            )
        );
    }

    #[test]
    fn mcp_resource_writer_renders_mime_and_description() {
        let mut output = Vec::new();
        write_mcp_resource_summaries(
            &mut output,
            &[McpResourceSummary {
                server_name: "memory".to_string(),
                uri: "memory://session/note".to_string(),
                mime_type: Some("text/markdown".to_string()),
                description: "Active session memory note".to_string(),
            }],
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert_eq!(
            rendered,
            concat!(
                "memory  memory://session/note\n",
                "  mime=text/markdown\n",
                "  Active session memory note\n",
            )
        );
    }

    #[test]
    fn headless_one_shot_activates_only_for_non_tty_prompt_runs() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let prompted =
            AppOptions::from_env_and_args_iter(dir.path(), &env_map, vec!["inspect".to_string()])
                .unwrap();
        let interactive =
            AppOptions::from_env_and_args_iter(dir.path(), &env_map, std::iter::empty::<String>())
                .unwrap();

        assert!(launch_headless_one_shot(&prompted, false, true));
        assert!(launch_headless_one_shot(&prompted, true, false));
        assert!(!launch_headless_one_shot(&prompted, true, true));
        assert!(!launch_headless_one_shot(&interactive, false, false));
        assert!(!launch_headless_one_shot(&interactive, true, true));
    }

    #[test]
    fn sandbox_fallback_requires_explicit_override_in_non_interactive_runs() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let default =
            AppOptions::from_env_and_args_iter(dir.path(), &env_map, std::iter::empty::<String>())
                .unwrap();
        let override_allowed = AppOptions::from_env_and_args_iter(
            dir.path(),
            &env_map,
            vec!["--allow-no-sandbox".to_string()],
        )
        .unwrap();

        assert_eq!(
            choose_sandbox_fallback_action(&default, true),
            SandboxFallbackAction::Prompt
        );
        assert_eq!(
            choose_sandbox_fallback_action(&default, false),
            SandboxFallbackAction::Abort
        );
        assert_eq!(
            choose_sandbox_fallback_action(&override_allowed, false),
            SandboxFallbackAction::Continue
        );
    }

    #[test]
    fn sandbox_abort_message_includes_setup_guidance_and_override_hint() {
        let message = format_sandbox_abort_message(&SandboxFallbackNotice {
            policy_summary: "workspace-write, network off, best effort host fallback".to_string(),
            reason: "bwrap probe failed".to_string(),
            risk_summary: "local subprocesses may run on the host".to_string(),
            setup_steps: vec![
                "install bubblewrap".to_string(),
                "enable user namespaces".to_string(),
            ],
        });

        assert!(message.contains("policy: workspace-write"));
        assert!(message.contains("1. install bubblewrap"));
        assert!(message.contains("--allow-no-sandbox"));
    }

    #[test]
    fn driver_outcome_extends_code_agent_runtime_inputs() {
        let merged = merge_driver_host_inputs(
            vec![HookRegistration {
                name: "existing-hook".into(),
                event: HookEvent::Stop,
                matcher: None,
                handler: HookHandler::Http(HttpHookHandler {
                    url: "https://example.test/existing".to_string(),
                    method: "POST".to_string(),
                    headers: BTreeMap::new(),
                }),
                timeout_ms: None,
                execution: None,
            }],
            vec![McpServerConfig {
                name: "existing-mcp".into(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "stdio-server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            vec!["existing instruction".to_string()],
            &DriverActivationOutcome {
                warnings: Vec::new(),
                hooks: vec![HookRegistration {
                    name: "driver-hook".into(),
                    event: HookEvent::SessionStart,
                    matcher: None,
                    handler: HookHandler::Http(HttpHookHandler {
                        url: "https://example.test/hook".to_string(),
                        method: "POST".to_string(),
                        headers: BTreeMap::new(),
                    }),
                    timeout_ms: Some(500),
                    execution: None,
                }],
                mcp_servers: vec![McpServerConfig {
                    name: "driver-mcp".into(),
                    enabled: true,
                    transport: McpTransportConfig::StreamableHttp {
                        url: "https://example.test/mcp".to_string(),
                        headers: BTreeMap::new(),
                    },
                }],
                instructions: vec!["driver instruction".to_string()],
                diagnostics: vec!["prepared runtime".to_string()],
                primary_memory_backend: None,
                tool_names: Vec::new(),
            },
        );

        assert_eq!(
            merged
                .runtime_hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook", "driver-hook"]
        );
        assert_eq!(
            merged
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp", "driver-mcp"]
        );
        assert_eq!(
            merged.instructions,
            vec![
                "existing instruction".to_string(),
                "driver instruction".to_string()
            ]
        );
    }

    #[test]
    fn tool_context_for_read_only_profile_promotes_accessible_roots_and_disables_full_network() {
        let profile = CoreConfig::default()
            .with_override(|config| {
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            })
            .resolve_subagent_profile(Some("reviewer"))
            .unwrap();
        let context = tool_context_for_profile(
            &ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/worktree")),
                additional_roots: vec![PathBuf::from("/refs")],
                writable_roots: vec![PathBuf::from("/workspace/tmp")],
                exec_roots: vec![PathBuf::from("/workspace/bin")],
                network_policy: Some(NetworkPolicy::Full),
                workspace_only: false,
                ..Default::default()
            },
            &profile,
        );

        assert!(context.workspace_only);
        assert!(context.writable_roots.is_empty());
        assert_eq!(context.network_policy, Some(NetworkPolicy::Off));
        assert_eq!(
            context.read_only_roots,
            vec![
                PathBuf::from("/refs"),
                PathBuf::from("/workspace"),
                PathBuf::from("/workspace/bin"),
                PathBuf::from("/workspace/tmp"),
                PathBuf::from("/worktree"),
            ]
        );
    }

    #[test]
    fn subagent_profile_resolver_routes_role_profiles_and_honors_tool_capability() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let resolver = CodeAgentSubagentProfileResolver {
            core: CoreConfig::default().with_override(|config| {
                let base_model = config.models["gpt_5_4_default"].clone();
                config.models.insert(
                    "reviewer_no_tools".to_string(),
                    ModelConfig {
                        capabilities: ModelCapabilitiesConfig {
                            tool_calls: false,
                            ..base_model.capabilities.clone()
                        },
                        ..base_model
                    },
                );
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        model: Some("reviewer_no_tools".to_string()),
                        system_prompt: Some("Review only".to_string()),
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            }),
            env_map: EnvMap::from_workspace_dir(dir.path()).unwrap(),
            base_tool_context: Arc::new(std::sync::RwLock::new(ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/workspace")),
                workspace_only: true,
                ..Default::default()
            })),
            skill_catalog: agent::SkillCatalog::default(),
            plugin_instructions: Arc::new(std::sync::RwLock::new(vec![
                "Plugin instruction".to_string(),
            ])),
        };

        let profile = resolver
            .resolve_profile(&SubagentLaunchSpec::from_task(AgentTaskSpec {
                task_id: "review".into(),
                role: "reviewer".to_string(),
                prompt: "review".to_string(),
                origin: agent::types::TaskOrigin::AgentCreated,
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            }))
            .unwrap();

        assert_eq!(profile.profile_name, "roles.reviewer");
        assert!(!profile.supports_tool_calls);
        assert!(profile.instructions.join("\n").contains("Review only"));
        assert_eq!(
            profile.tool_context.model_context_window_tokens,
            Some(400_000)
        );
        assert_eq!(
            profile.tool_context.network_policy,
            Some(NetworkPolicy::Off)
        );
    }

    #[test]
    fn subagent_profile_resolver_applies_launch_model_and_reasoning_overrides() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let resolver = CodeAgentSubagentProfileResolver {
            core: CoreConfig::default().with_override(|config| {
                let base_model = config.models["gpt_5_4_default"].clone();
                config.models.insert(
                    "reviewer_no_tools".to_string(),
                    ModelConfig {
                        capabilities: ModelCapabilitiesConfig {
                            tool_calls: false,
                            ..base_model.capabilities.clone()
                        },
                        ..base_model
                    },
                );
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        model: Some("reviewer_no_tools".to_string()),
                        system_prompt: Some("Review only".to_string()),
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            }),
            env_map: EnvMap::from_workspace_dir(dir.path()).unwrap(),
            base_tool_context: Arc::new(std::sync::RwLock::new(ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/workspace")),
                workspace_only: true,
                ..Default::default()
            })),
            skill_catalog: agent::SkillCatalog::default(),
            plugin_instructions: Arc::new(std::sync::RwLock::new(vec![
                "Plugin instruction".to_string(),
            ])),
        };

        let launch = SubagentLaunchSpec {
            task: AgentTaskSpec {
                task_id: "review".into(),
                role: "reviewer".to_string(),
                prompt: "review".to_string(),
                origin: agent::types::TaskOrigin::AgentCreated,
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            },
            initial_input: agent::types::Message::user("review"),
            fork_context: false,
            worktree_mode: agent::tools::ChildWorktreeMode::Inherit,
            model: Some("gpt_5_4_default".to_string()),
            reasoning_effort: Some("high".to_string()),
        };

        let resolved = resolver.resolve_agent_profile(&launch).unwrap();
        assert_eq!(resolved.model.alias, "gpt_5_4_default");
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));

        let profile = resolver.resolve_profile(&launch).unwrap();
        assert!(profile.supports_tool_calls);
        assert!(profile.instructions.join("\n").contains("Review only"));
    }

    #[test]
    fn empty_driver_outcome_keeps_code_agent_runtime_inputs_stable() {
        let merged = merge_driver_host_inputs(
            vec![HookRegistration {
                name: "existing-hook".into(),
                event: HookEvent::Stop,
                matcher: None,
                handler: HookHandler::Http(HttpHookHandler {
                    url: "https://example.test/existing".to_string(),
                    method: "POST".to_string(),
                    headers: BTreeMap::new(),
                }),
                timeout_ms: None,
                execution: None,
            }],
            vec![McpServerConfig {
                name: "existing-mcp".into(),
                enabled: true,
                transport: McpTransportConfig::Stdio {
                    command: "stdio-server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            vec!["existing instruction".to_string()],
            &DriverActivationOutcome::default(),
        );

        assert_eq!(
            merged
                .runtime_hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook"]
        );
        assert_eq!(
            merged
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp"]
        );
        assert_eq!(
            merged.instructions,
            vec!["existing instruction".to_string()]
        );
    }

    #[test]
    fn driver_diagnostics_are_rendered_for_host_output() {
        let lines = driver_host_output_lines(&DriverActivationOutcome {
            warnings: vec!["slow startup".to_string()],
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
            instructions: Vec::new(),
            diagnostics: vec!["validated wasm hook module".to_string()],
            primary_memory_backend: None,
            tool_names: Vec::new(),
        });

        assert_eq!(
            lines,
            vec![
                "warning: plugin driver warning: slow startup".to_string(),
                "info: plugin driver diagnostic: validated wasm hook module".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_and_dedup_plugin_mcp_servers_matches_host_boot_expectations() {
        let dir = tempdir().unwrap();
        let resolved = dedup_mcp_servers(resolve_mcp_servers(
            &[
                McpServerConfig {
                    name: "dup".into(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "first".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: Some("relative".to_string()),
                    },
                },
                McpServerConfig {
                    name: "dup".into(),
                    enabled: true,
                    transport: McpTransportConfig::Stdio {
                        command: "second".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: Some("ignored".to_string()),
                    },
                },
            ],
            dir.path(),
        ));

        assert_eq!(resolved.len(), 1);
        match &resolved[0].transport {
            McpTransportConfig::Stdio { command, cwd, .. } => {
                let expected_cwd = dir.path().join("relative");
                assert_eq!(command, "first");
                assert_eq!(
                    cwd.as_deref(),
                    Some(expected_cwd.to_string_lossy().as_ref())
                );
            }
            McpTransportConfig::StreamableHttp { .. } => {
                panic!("expected stdio transport");
            }
        }
    }

    #[test]
    fn resolve_mcp_servers_skips_disabled_entries() {
        let dir = tempdir().unwrap();
        let resolved = resolve_mcp_servers(
            &[McpServerConfig {
                name: "disabled".into(),
                enabled: false,
                transport: McpTransportConfig::StreamableHttp {
                    url: "https://example.test/mcp".to_string(),
                    headers: BTreeMap::new(),
                },
            }],
            dir.path(),
        );

        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn loads_sandbox_fail_closed_from_env_and_cli() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OPENAI_API_KEY=test-key\nNANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=false\n",
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let options = AppOptions::from_env_and_args_iter(
            dir.path(),
            &env_map,
            vec![
                "--sandbox-fail-if-unavailable".to_string(),
                "true".to_string(),
            ],
        )
        .unwrap();
        let tool_context = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            worktree_root: Some(dir.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };

        let policy = build_sandbox_policy(&options, &tool_context);

        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(policy.network, NetworkPolicy::Off);
        assert!(policy.fail_if_unavailable);
    }
}
