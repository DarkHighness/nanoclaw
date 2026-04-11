use super::state::{InspectorAction, InspectorEntry};
use crate::interaction::SessionPermissionMode;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

mod completion;
mod parse;

#[cfg(test)]
pub(crate) use completion::command_palette_lines;
pub(crate) use completion::{
    command_palette_lines_for, cycle_slash_command, inspector_action_for_slash_name,
    move_slash_command_selection, resolve_slash_enter_action, slash_command_hint,
};
pub(crate) use parse::parse_slash_command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandSpec {
    pub(crate) section: &'static str,
    pub(crate) name: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) summary: &'static str,
}

impl SlashCommandSpec {
    pub(crate) fn requires_arguments(self) -> bool {
        self.argument_specs()
            .iter()
            .any(|argument| argument.required)
    }

    pub(crate) fn aliases(self) -> &'static [&'static str] {
        match self.name {
            "exit" => &["quit", "q"],
            _ => &[],
        }
    }

    pub(crate) fn matches_prefix(self, prefix: &str) -> bool {
        prefix.is_empty()
            || self.name.starts_with(prefix)
            || self.aliases().iter().any(|alias| alias.starts_with(prefix))
    }

    pub(crate) fn matches_token(self, token: &str) -> bool {
        self.name == token || self.aliases().contains(&token)
    }

    pub(crate) fn argument_specs(self) -> Vec<SlashCommandArgumentSpec> {
        self.usage
            .split_whitespace()
            .skip(1)
            .map(|placeholder| SlashCommandArgumentSpec {
                placeholder,
                required: placeholder.starts_with('<'),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandHint {
    pub(crate) selected: SlashCommandSpec,
    pub(crate) matches: Vec<SlashCommandSpec>,
    pub(crate) selected_match_index: usize,
    pub(crate) arguments: Option<SlashCommandArgumentHint>,
    pub(crate) exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentHint {
    pub(crate) provided: Vec<SlashCommandArgumentValue>,
    pub(crate) next: Option<SlashCommandArgumentSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentValue {
    pub(crate) placeholder: &'static str,
    pub(crate) value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentSpec {
    pub(crate) placeholder: &'static str,
    pub(crate) required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommandEnterAction {
    Complete { input: String, index: usize },
    Execute(String),
}

const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        section: "Session",
        name: "help",
        usage: "help [query]",
        summary: "browse commands",
    },
    SlashCommandSpec {
        section: "Session",
        name: "status",
        usage: "status",
        summary: "session overview",
    },
    SlashCommandSpec {
        section: "Session",
        name: "details",
        usage: "details",
        summary: "toggle tool details",
    },
    SlashCommandSpec {
        section: "Session",
        name: "statusline",
        usage: "statusline",
        summary: "toggle footer items",
    },
    SlashCommandSpec {
        section: "Session",
        name: "thinking",
        usage: "thinking [level]",
        summary: "pick or set thinking effort",
    },
    SlashCommandSpec {
        section: "Session",
        name: "theme",
        usage: "theme [name]",
        summary: "pick or set the tui theme",
    },
    SlashCommandSpec {
        section: "Session",
        name: "image",
        usage: "image <path-or-url>",
        summary: "attach image to composer",
    },
    SlashCommandSpec {
        section: "Session",
        name: "file",
        usage: "file <path-or-url>",
        summary: "attach file to composer",
    },
    SlashCommandSpec {
        section: "Session",
        name: "detach",
        usage: "detach [index]",
        summary: "remove composer attachment",
    },
    SlashCommandSpec {
        section: "Session",
        name: "move_attachment",
        usage: "move_attachment <from> <to>",
        summary: "reorder composer attachments",
    },
    SlashCommandSpec {
        section: "Session",
        name: "new",
        usage: "new",
        summary: "fresh top-level session",
    },
    SlashCommandSpec {
        section: "Session",
        name: "clear",
        usage: "clear",
        summary: "alias of /new",
    },
    SlashCommandSpec {
        section: "Session",
        name: "compact",
        usage: "compact [notes]",
        summary: "compact active history",
    },
    SlashCommandSpec {
        section: "Session",
        name: "btw",
        usage: "btw <question>",
        summary: "ask a side question without interrupting work",
    },
    SlashCommandSpec {
        section: "Session",
        name: "steer",
        usage: "steer <notes>",
        summary: "schedule safe-point guidance",
    },
    SlashCommandSpec {
        section: "Session",
        name: "queue",
        usage: "queue",
        summary: "browse pending prompts and steers",
    },
    SlashCommandSpec {
        section: "Session",
        name: "permissions",
        usage: "permissions [default|danger-full-access]",
        summary: "inspect or switch the session sandbox mode",
    },
    SlashCommandSpec {
        section: "Session",
        name: "exit",
        usage: "exit",
        summary: "leave tui",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "live_tasks",
        usage: "live_tasks",
        summary: "list live child agents",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "spawn_task",
        usage: "spawn_task <role> <prompt>",
        summary: "launch child agent",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "send_task",
        usage: "send_task <task-or-agent-ref> <message>",
        summary: "steer child agent",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "wait_task",
        usage: "wait_task <task-or-agent-ref>",
        summary: "wait for child agent",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "cancel_task",
        usage: "cancel_task <task-or-agent-ref> [reason]",
        summary: "stop child agent",
    },
    SlashCommandSpec {
        section: "History",
        name: "sessions",
        usage: "sessions [query]",
        summary: "browse persisted sessions",
    },
    SlashCommandSpec {
        section: "History",
        name: "session",
        usage: "session <session-ref>",
        summary: "open persisted session",
    },
    SlashCommandSpec {
        section: "History",
        name: "agent_sessions",
        usage: "agent_sessions [session-ref]",
        summary: "list agent sessions",
    },
    SlashCommandSpec {
        section: "History",
        name: "agent_session",
        usage: "agent_session <agent-session-ref>",
        summary: "inspect agent session",
    },
    SlashCommandSpec {
        section: "History",
        name: "resume",
        usage: "resume <agent-session-ref>",
        summary: "reattach agent session",
    },
    SlashCommandSpec {
        section: "History",
        name: "tasks",
        usage: "tasks [session-ref]",
        summary: "list persisted child tasks",
    },
    SlashCommandSpec {
        section: "History",
        name: "task",
        usage: "task <task-id>",
        summary: "inspect persisted task",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "tools",
        usage: "tools",
        summary: "list tools",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "skills",
        usage: "skills",
        summary: "list discovered skills",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "diagnostics",
        usage: "diagnostics",
        summary: "startup diagnostics",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "mcp",
        usage: "mcp",
        summary: "list MCP servers",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "prompts",
        usage: "prompts",
        summary: "list MCP prompts",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "resources",
        usage: "resources",
        summary: "list MCP resources",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "prompt",
        usage: "prompt <server> <name>",
        summary: "load MCP prompt",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "resource",
        usage: "resource <server> <uri>",
        summary: "load MCP resource",
    },
    SlashCommandSpec {
        section: "Export",
        name: "export_session",
        usage: "export_session <session-ref> <path>",
        summary: "write session export",
    },
    SlashCommandSpec {
        section: "Export",
        name: "export_transcript",
        usage: "export_transcript <session-ref> <path>",
        summary: "write transcript export",
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Status,
    Details,
    StatusLine,
    Thinking {
        effort: Option<String>,
    },
    Theme {
        name: Option<String>,
    },
    Image {
        path: String,
    },
    File {
        path: String,
    },
    Detach {
        index: Option<usize>,
    },
    MoveAttachment {
        from: usize,
        to: usize,
    },
    Help {
        query: Option<String>,
    },
    Tools,
    Skills,
    Diagnostics,
    Mcp,
    Prompts,
    Resources,
    Prompt {
        server_name: String,
        prompt_name: String,
    },
    Resource {
        server_name: String,
        uri: String,
    },
    Steer {
        message: Option<String>,
    },
    Queue,
    Permissions {
        mode: Option<SessionPermissionMode>,
    },
    Compact {
        notes: Option<String>,
    },
    Btw {
        question: Option<String>,
    },
    New,
    AgentSessions {
        session_ref: Option<String>,
    },
    AgentSession {
        agent_session_ref: String,
    },
    LiveTasks,
    SpawnTask {
        role: String,
        prompt: String,
    },
    SendTask {
        task_or_agent_ref: String,
        message: Option<String>,
    },
    WaitTask {
        task_or_agent_ref: String,
    },
    CancelTask {
        task_or_agent_ref: String,
        reason: Option<String>,
    },
    Tasks {
        session_ref: Option<String>,
    },
    Task {
        task_ref: String,
    },
    Sessions {
        query: Option<String>,
    },
    Session {
        session_ref: String,
    },
    Resume {
        agent_session_ref: String,
    },
    ExportSession {
        session_ref: String,
        path: String,
    },
    ExportTranscript {
        session_ref: String,
        path: String,
    },
    Quit,
    InvalidUsage(String),
}

#[derive(Parser, Debug)]
#[command(
    no_binary_name = true,
    disable_version_flag = true,
    disable_help_flag = true,
    disable_help_subcommand = true
)]
struct SlashCli {
    #[command(subcommand)]
    command: SlashSubcommand,
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "snake_case")]
enum SlashSubcommand {
    Status,
    Details,
    Statusline,
    Thinking {
        effort: Option<String>,
    },
    Theme {
        name: Option<String>,
    },
    Image {
        #[arg(value_name = "PATH_OR_URL", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    File {
        #[arg(value_name = "PATH_OR_URL", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    Detach {
        index: Option<usize>,
    },
    MoveAttachment {
        from: usize,
        to: usize,
    },
    Help {
        #[arg(
            value_name = "QUERY",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        query: Vec<String>,
    },
    Tools,
    Skills,
    Diagnostics,
    Mcp,
    Prompts,
    Resources,
    Prompt {
        server_name: String,
        prompt_name: String,
    },
    Resource {
        server_name: String,
        uri: String,
    },
    Steer {
        #[arg(
            value_name = "NOTES",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        message: Vec<String>,
    },
    Queue,
    Permissions {
        #[arg(value_enum)]
        mode: Option<PermissionModeArg>,
    },
    Compact {
        #[arg(
            value_name = "NOTES",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        notes: Vec<String>,
    },
    Btw {
        #[arg(
            value_name = "QUESTION",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        question: Vec<String>,
    },
    #[command(alias = "clear")]
    New,
    AgentSessions {
        session_ref: Option<String>,
    },
    AgentSession {
        agent_session_ref: String,
    },
    LiveTasks,
    SpawnTask {
        role: String,
        #[arg(
            value_name = "PROMPT",
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        prompt: Vec<String>,
    },
    SendTask {
        task_or_agent_ref: String,
        #[arg(
            value_name = "MESSAGE",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        message: Vec<String>,
    },
    WaitTask {
        task_or_agent_ref: String,
    },
    CancelTask {
        task_or_agent_ref: String,
        #[arg(
            value_name = "REASON",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        reason: Vec<String>,
    },
    Tasks {
        session_ref: Option<String>,
    },
    Task {
        task_ref: String,
    },
    Sessions {
        #[arg(
            value_name = "QUERY",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        query: Vec<String>,
    },
    Session {
        session_ref: String,
    },
    Resume {
        agent_session_ref: String,
    },
    ExportSession {
        session_ref: String,
        #[arg(value_name = "PATH", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    ExportTranscript {
        session_ref: String,
        #[arg(value_name = "PATH", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    #[command(name = "exit", alias = "quit", alias = "q")]
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PermissionModeArg {
    Default,
    #[value(
        name = "danger-full-access",
        alias = "dangerous-full-access",
        alias = "danger"
    )]
    DangerFullAccess,
}

impl From<PermissionModeArg> for SessionPermissionMode {
    fn from(value: PermissionModeArg) -> Self {
        match value {
            PermissionModeArg::Default => SessionPermissionMode::Default,
            PermissionModeArg::DangerFullAccess => SessionPermissionMode::DangerFullAccess,
        }
    }
}

#[cfg(test)]
mod tests;
