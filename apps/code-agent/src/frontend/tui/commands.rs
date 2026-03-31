use super::state::InspectorEntry;
use crate::backend::SessionPermissionMode;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

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

pub(crate) fn parse_slash_command(input: &str) -> SlashCommand {
    let trimmed = input.trim();
    let body = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let Some(args) = shlex::split(body) else {
        return SlashCommand::InvalidUsage("Unable to parse command line".to_string());
    };

    match SlashCli::try_parse_from(args) {
        Ok(parsed) => parsed.command.into(),
        Err(error) => SlashCommand::InvalidUsage(render_usage_error(error)),
    }
}

pub(crate) fn command_palette_lines() -> Vec<InspectorEntry> {
    command_palette_lines_for(None)
}

pub(crate) fn command_palette_lines_for(query: Option<&str>) -> Vec<InspectorEntry> {
    let trimmed = query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| query.trim_start_matches('/').to_ascii_lowercase());
    let specs = trimmed
        .as_deref()
        .map(palette_matching_specs)
        .unwrap_or_else(|| SLASH_COMMAND_SPECS.to_vec());
    if specs.is_empty() {
        return vec![
            InspectorEntry::section("Command Palette"),
            InspectorEntry::Muted("No commands match this query.".to_string()),
        ];
    }
    let mut lines = Vec::new();
    let mut current_section = None;
    for spec in specs {
        if current_section != Some(spec.section) {
            current_section = Some(spec.section);
            lines.push(InspectorEntry::section(spec.section));
        }
        let alias_suffix = if spec.aliases().is_empty() {
            String::new()
        } else {
            format!(
                " · aliases: {}",
                spec.aliases()
                    .iter()
                    .map(|alias| format!("/{alias}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        lines.push(InspectorEntry::collection(
            format!("/{}", spec.usage),
            Some(format!("{}{}", spec.summary, alias_suffix)),
        ));
    }
    lines
}

pub(crate) fn slash_command_hint(input: &str, selected_index: usize) -> Option<SlashCommandHint> {
    let (command_token, tail) = split_slash_input(input)?;
    let matches = matching_specs(command_token);
    if let Some(selected) = selected_spec(command_token, tail, selected_index, &matches) {
        return Some(SlashCommandHint {
            exact: selected.matches_token(command_token),
            arguments: selected
                .matches_token(command_token)
                .then(|| build_argument_hint(selected, tail))
                .flatten(),
            selected_match_index: matches
                .iter()
                .position(|spec| spec.name == selected.name)
                .unwrap_or(0),
            selected,
            matches,
        });
    }
    None
}

pub(crate) fn cycle_slash_command(
    input: &str,
    selected_index: usize,
    backwards: bool,
) -> Option<(String, usize)> {
    let (command_token, tail) = split_slash_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_specs(command_token);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    let exact_at_current = matches
        .get(current)
        .is_some_and(|spec| spec.name == command_token);
    let next = if backwards {
        if exact_at_current {
            current.checked_sub(1).unwrap_or(matches.len() - 1)
        } else {
            matches.len() - 1
        }
    } else if exact_at_current {
        (current + 1) % matches.len()
    } else {
        current
    };
    Some((format!("/{} ", matches[next].name), next))
}

pub(crate) fn move_slash_command_selection(
    input: &str,
    selected_index: usize,
    backwards: bool,
) -> Option<usize> {
    let (command_token, tail) = split_slash_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_specs(command_token);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    Some(if backwards {
        current.checked_sub(1).unwrap_or(matches.len() - 1)
    } else {
        (current + 1) % matches.len()
    })
}

pub(crate) fn resolve_slash_enter_action(
    input: &str,
    selected_index: usize,
) -> Option<SlashCommandEnterAction> {
    let hint = slash_command_hint(input, selected_index)?;
    if hint.exact {
        if hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some_and(|argument| argument.required)
        {
            return Some(SlashCommandEnterAction::Complete {
                input: format!("/{} ", hint.selected.name),
                index: hint.selected_match_index,
            });
        }
        return None;
    }
    if hint.matches.len() == 1 && !hint.selected.requires_arguments() {
        return Some(SlashCommandEnterAction::Execute(format!(
            "/{}",
            hint.selected.name
        )));
    }
    Some(SlashCommandEnterAction::Complete {
        input: format!("/{} ", hint.selected.name),
        index: hint.selected_match_index,
    })
}

impl From<SlashSubcommand> for SlashCommand {
    fn from(value: SlashSubcommand) -> Self {
        match value {
            SlashSubcommand::Status => Self::Status,
            SlashSubcommand::Details => Self::Details,
            SlashSubcommand::Statusline => Self::StatusLine,
            SlashSubcommand::Thinking { effort } => Self::Thinking { effort },
            SlashSubcommand::Theme { name } => Self::Theme { name },
            SlashSubcommand::Image { path } => Self::Image {
                path: join_required_tail(path),
            },
            SlashSubcommand::File { path } => Self::File {
                path: join_required_tail(path),
            },
            SlashSubcommand::Detach { index } => Self::Detach { index },
            SlashSubcommand::MoveAttachment { from, to } => Self::MoveAttachment { from, to },
            SlashSubcommand::Help { query } => Self::Help {
                query: join_optional_tail(query),
            },
            SlashSubcommand::Tools => Self::Tools,
            SlashSubcommand::Skills => Self::Skills,
            SlashSubcommand::Diagnostics => Self::Diagnostics,
            SlashSubcommand::Mcp => Self::Mcp,
            SlashSubcommand::Prompts => Self::Prompts,
            SlashSubcommand::Resources => Self::Resources,
            SlashSubcommand::Prompt {
                server_name,
                prompt_name,
            } => Self::Prompt {
                server_name,
                prompt_name,
            },
            SlashSubcommand::Resource { server_name, uri } => Self::Resource { server_name, uri },
            SlashSubcommand::Steer { message } => Self::Steer {
                message: join_optional_tail(message),
            },
            SlashSubcommand::Queue => Self::Queue,
            SlashSubcommand::Permissions { mode } => Self::Permissions {
                mode: mode.map(Into::into),
            },
            SlashSubcommand::Compact { notes } => Self::Compact {
                notes: join_optional_tail(notes),
            },
            SlashSubcommand::Btw { question } => Self::Btw {
                question: join_optional_tail(question),
            },
            SlashSubcommand::New => Self::New,
            SlashSubcommand::AgentSessions { session_ref } => Self::AgentSessions { session_ref },
            SlashSubcommand::AgentSession { agent_session_ref } => {
                Self::AgentSession { agent_session_ref }
            }
            SlashSubcommand::LiveTasks => Self::LiveTasks,
            SlashSubcommand::SpawnTask { role, prompt } => Self::SpawnTask {
                role,
                prompt: join_required_tail(prompt),
            },
            SlashSubcommand::SendTask {
                task_or_agent_ref,
                message,
            } => Self::SendTask {
                task_or_agent_ref,
                message: join_optional_tail(message),
            },
            SlashSubcommand::WaitTask { task_or_agent_ref } => Self::WaitTask { task_or_agent_ref },
            SlashSubcommand::CancelTask {
                task_or_agent_ref,
                reason,
            } => Self::CancelTask {
                task_or_agent_ref,
                reason: join_optional_tail(reason),
            },
            SlashSubcommand::Tasks { session_ref } => Self::Tasks { session_ref },
            SlashSubcommand::Task { task_ref } => Self::Task { task_ref },
            SlashSubcommand::Sessions { query } => Self::Sessions {
                query: join_optional_tail(query),
            },
            SlashSubcommand::Session { session_ref } => Self::Session { session_ref },
            SlashSubcommand::Resume { agent_session_ref } => Self::Resume { agent_session_ref },
            SlashSubcommand::ExportSession { session_ref, path } => Self::ExportSession {
                session_ref,
                path: join_required_tail(path),
            },
            SlashSubcommand::ExportTranscript { session_ref, path } => Self::ExportTranscript {
                session_ref,
                path: join_required_tail(path),
            },
            SlashSubcommand::Quit => Self::Quit,
        }
    }
}

fn join_optional_tail(parts: Vec<String>) -> Option<String> {
    let joined = parts.join(" ").trim().to_string();
    (!joined.is_empty()).then_some(joined)
}

fn join_required_tail(parts: Vec<String>) -> String {
    parts.join(" ").trim().to_string()
}

fn render_usage_error(error: clap::Error) -> String {
    let rendered = error.to_string().trim().to_string();
    if rendered.is_empty() {
        let mut command = SlashCli::command().styles(clap::builder::Styles::plain());
        return command.render_help().to_string().trim().to_string();
    }
    rendered
}

fn split_slash_input(input: &str) -> Option<(&str, Option<&str>)> {
    let body = input.strip_prefix('/')?;
    Some(
        body.split_once(' ')
            .map_or((body, None), |(command_token, tail)| {
                (command_token, Some(tail))
            }),
    )
}

fn matching_specs(prefix: &str) -> Vec<SlashCommandSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    let mut matches = SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .filter(|spec| spec.matches_prefix(&prefix))
        .collect::<Vec<_>>();
    if let Some(exact_index) = matches.iter().position(|spec| spec.matches_token(&prefix)) {
        matches.swap(0, exact_index);
    }
    matches
}

fn palette_matching_specs(prefix: &str) -> Vec<SlashCommandSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .filter(|spec| {
            spec.matches_prefix(&prefix) || spec.section.to_ascii_lowercase().starts_with(&prefix)
        })
        .collect()
}

fn selected_spec(
    command_token: &str,
    tail: Option<&str>,
    selected_index: usize,
    matches: &[SlashCommandSpec],
) -> Option<SlashCommandSpec> {
    if tail.is_some() {
        return SLASH_COMMAND_SPECS
            .iter()
            .copied()
            .find(|spec| spec.matches_token(command_token));
    }
    matches
        .get(selected_index.min(matches.len().saturating_sub(1)))
        .copied()
}

fn build_argument_hint(
    spec: SlashCommandSpec,
    tail: Option<&str>,
) -> Option<SlashCommandArgumentHint> {
    let placeholders = spec.argument_specs();
    if placeholders.is_empty() {
        return None;
    }

    let tail = tail.unwrap_or("").trim();
    let raw_values = if tail.is_empty() {
        Vec::new()
    } else {
        tail.split_whitespace().collect::<Vec<_>>()
    };
    let provided_count = raw_values.len().min(placeholders.len());
    let mut provided = Vec::new();
    for (index, placeholder) in placeholders.iter().take(provided_count).enumerate() {
        // The last positional is treated as a greedy tail because several host
        // commands intentionally accept spaces after the final placeholder
        // (`spawn_task <prompt>`, export paths, free-form notes).
        let value = if index + 1 == placeholders.len() {
            raw_values[index..].join(" ")
        } else {
            raw_values[index].to_string()
        };
        provided.push(SlashCommandArgumentValue {
            placeholder: placeholder.placeholder,
            value,
        });
        if index + 1 == placeholders.len() {
            break;
        }
    }

    Some(SlashCommandArgumentHint {
        provided,
        next: placeholders.get(provided_count).copied(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        SlashCommand, SlashCommandArgumentSpec, SlashCommandEnterAction, command_palette_lines,
        command_palette_lines_for, cycle_slash_command, move_slash_command_selection,
        parse_slash_command, resolve_slash_enter_action, slash_command_hint,
    };
    use crate::frontend::tui::state::InspectorEntry;

    #[test]
    fn parses_session_query_with_spaces() {
        match parse_slash_command("/sessions fix failing test") {
            SlashCommand::Sessions { query } => {
                assert_eq!(query, Some("fix failing test".to_string()));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_btw_question_with_spaces() {
        match parse_slash_command("/btw what changed in the deploy flow") {
            SlashCommand::Btw { question } => {
                assert_eq!(
                    question,
                    Some("what changed in the deploy flow".to_string())
                );
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn export_transcript_keeps_path_tail_intact() {
        match parse_slash_command("/export_transcript abc123 reports/run log.txt") {
            SlashCommand::ExportTranscript { session_ref, path } => {
                assert_eq!(session_ref, "abc123");
                assert_eq!(path, "reports/run log.txt");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_agent_session_listing_with_optional_session_ref() {
        match parse_slash_command("/agent_sessions abc123") {
            SlashCommand::AgentSessions { session_ref } => {
                assert_eq!(session_ref, Some("abc123".to_string()));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_agent_session_lookup() {
        match parse_slash_command("/agent_session agent123") {
            SlashCommand::AgentSession { agent_session_ref } => {
                assert_eq!(agent_session_ref, "agent123");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_task_listing_with_optional_session_ref() {
        match parse_slash_command("/tasks abc123") {
            SlashCommand::Tasks { session_ref } => {
                assert_eq!(session_ref, Some("abc123".to_string()));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_task_lookup() {
        match parse_slash_command("/task review-task") {
            SlashCommand::Task { task_ref } => {
                assert_eq!(task_ref, "review-task");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_live_task_listing() {
        assert!(matches!(
            parse_slash_command("/live_tasks"),
            SlashCommand::LiveTasks
        ));
    }

    #[test]
    fn parses_permissions_mode_switch() {
        match parse_slash_command("/permissions danger-full-access") {
            SlashCommand::Permissions { mode } => {
                assert_eq!(
                    mode,
                    Some(crate::backend::SessionPermissionMode::DangerFullAccess)
                );
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_spawn_task_with_prompt_tail() {
        match parse_slash_command("/spawn_task reviewer inspect failing tests") {
            SlashCommand::SpawnTask { role, prompt } => {
                assert_eq!(role, "reviewer");
                assert_eq!(prompt, "inspect failing tests");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn spawn_task_requires_prompt() {
        match parse_slash_command("/spawn_task reviewer") {
            SlashCommand::InvalidUsage(message) => {
                assert!(message.contains("Usage:"));
                assert!(message.contains("spawn_task <ROLE> <PROMPT>"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_send_task_with_message_tail() {
        match parse_slash_command("/send_task review-task focus on tests") {
            SlashCommand::SendTask {
                task_or_agent_ref,
                message,
            } => {
                assert_eq!(task_or_agent_ref, "review-task");
                assert_eq!(message, Some("focus on tests".to_string()));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_wait_task_lookup() {
        match parse_slash_command("/wait_task review-task") {
            SlashCommand::WaitTask { task_or_agent_ref } => {
                assert_eq!(task_or_agent_ref, "review-task");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_cancel_task_with_optional_reason_tail() {
        match parse_slash_command("/cancel_task review-task fix it now") {
            SlashCommand::CancelTask {
                task_or_agent_ref,
                reason,
            } => {
                assert_eq!(task_or_agent_ref, "review-task");
                assert_eq!(reason, Some("fix it now".to_string()));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn missing_session_ref_returns_usage_error() {
        match parse_slash_command("/session") {
            SlashCommand::InvalidUsage(message) => {
                assert!(message.contains("Usage:"));
                assert!(message.contains("session <SESSION_REF>"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_mcp_prompt_lookup() {
        match parse_slash_command("/prompt fs code_review") {
            SlashCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                assert_eq!(server_name, "fs");
                assert_eq!(prompt_name, "code_review");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_new_and_clear_as_same_session_operation() {
        assert!(matches!(parse_slash_command("/new"), SlashCommand::New));
        assert!(matches!(parse_slash_command("/clear"), SlashCommand::New));
    }

    #[test]
    fn parses_details_toggle() {
        assert!(matches!(
            parse_slash_command("/details"),
            SlashCommand::Details
        ));
    }

    #[test]
    fn parses_statusline_picker_command() {
        assert!(matches!(
            parse_slash_command("/statusline"),
            SlashCommand::StatusLine
        ));
    }

    #[test]
    fn parses_queue_command() {
        assert!(matches!(parse_slash_command("/queue"), SlashCommand::Queue));
    }

    #[test]
    fn parses_thinking_effort_command() {
        assert_eq!(
            parse_slash_command("/thinking high"),
            SlashCommand::Thinking {
                effort: Some("high".to_string())
            }
        );
        assert_eq!(
            parse_slash_command("/thinking"),
            SlashCommand::Thinking { effort: None }
        );
    }

    #[test]
    fn parses_theme_command() {
        assert_eq!(
            parse_slash_command("/theme fjord"),
            SlashCommand::Theme {
                name: Some("fjord".to_string())
            }
        );
        assert_eq!(
            parse_slash_command("/theme"),
            SlashCommand::Theme { name: None }
        );
    }

    #[test]
    fn parses_image_and_file_attachment_commands() {
        assert_eq!(
            parse_slash_command("/image artifacts/failure.png"),
            SlashCommand::Image {
                path: "artifacts/failure.png".to_string()
            }
        );
        assert_eq!(
            parse_slash_command("/file reports/run log.pdf"),
            SlashCommand::File {
                path: "reports/run log.pdf".to_string()
            }
        );
    }

    #[test]
    fn parses_detach_with_optional_index() {
        assert_eq!(
            parse_slash_command("/detach 2"),
            SlashCommand::Detach { index: Some(2) }
        );
        assert_eq!(
            parse_slash_command("/detach"),
            SlashCommand::Detach { index: None }
        );
    }

    #[test]
    fn parses_move_attachment_command() {
        assert_eq!(
            parse_slash_command("/move_attachment 2 1"),
            SlashCommand::MoveAttachment { from: 2, to: 1 }
        );
    }

    #[test]
    fn command_palette_includes_help_and_clear_alias() {
        let lines = inspector_line_texts(&command_palette_lines());

        assert!(lines.iter().any(|line| line == "## Session"));
        assert!(
            lines
                .iter()
                .any(|line| line == "/help [query]  browse commands")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "/details  toggle tool details")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "/statusline  toggle footer items")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "/thinking [level]  pick or set thinking effort")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "/theme [name]  pick or set the tui theme")
        );
        assert!(lines.iter().any(|line| line == "/clear  alias of /new"));
        assert!(
            lines
                .iter()
                .any(|line| { line == "/exit  leave tui · aliases: /quit /q" })
        );
    }

    #[test]
    fn command_palette_can_filter_by_query() {
        let lines = inspector_line_texts(&command_palette_lines_for(Some("agent")));

        assert!(lines.iter().any(|line| line == "## Agents"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("/agent_sessions [session-ref]"))
        );
        assert!(!lines.iter().any(|line| line.contains("/export_transcript")));
    }

    fn inspector_line_texts(lines: &[InspectorEntry]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match line {
                InspectorEntry::Section(text)
                | InspectorEntry::Plain(text)
                | InspectorEntry::Muted(text)
                | InspectorEntry::Command(text) => {
                    if matches!(line, InspectorEntry::Section(_)) {
                        format!("## {text}")
                    } else {
                        text.clone()
                    }
                }
                InspectorEntry::Field { key, value } => format!("{key}: {value}"),
                InspectorEntry::Transcript(entry) => entry.serialized(),
                InspectorEntry::CollectionItem { primary, secondary } => secondary
                    .as_ref()
                    .map(|secondary| format!("{primary}  {secondary}"))
                    .unwrap_or_else(|| primary.clone()),
                InspectorEntry::Empty => String::new(),
            })
            .collect()
    }

    #[test]
    fn parses_help_query_tail() {
        match parse_slash_command("/help agent") {
            SlashCommand::Help { query } => {
                assert_eq!(query, Some("agent".to_string()));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn slash_command_hint_matches_prefix() {
        let hint = slash_command_hint("/sess", 0).expect("hint");

        assert_eq!(hint.selected.name, "sessions");
        assert_eq!(hint.selected_match_index, 0);
        assert!(hint.matches.iter().any(|spec| spec.name == "sessions"));
        assert!(hint.matches.iter().any(|spec| spec.name == "session"));
        assert!(hint.arguments.is_none());
    }

    #[test]
    fn slash_command_hint_matches_exit_alias_prefix() {
        let hint = slash_command_hint("/q", 0).expect("hint");

        assert_eq!(hint.selected.name, "exit");
        assert!(hint.exact);
        assert!(hint.matches.iter().any(|spec| spec.name == "exit"));
    }

    #[test]
    fn cycle_slash_command_completes_partial_input() {
        let (input, index) = cycle_slash_command("/sess", 0, false).expect("completion");

        assert_eq!(input, "/sessions ");
        assert_eq!(index, 0);
    }

    #[test]
    fn cycle_slash_command_cycles_backward() {
        let (input, index) = cycle_slash_command("/sess", 0, true).expect("completion");

        assert_eq!(input, "/session ");
        assert_eq!(index, 1);
    }

    #[test]
    fn cycle_slash_command_stops_after_args_begin() {
        assert!(cycle_slash_command("/session abc123", 0, false).is_none());
    }

    #[test]
    fn move_slash_command_selection_keeps_partial_input_in_picker() {
        let next = move_slash_command_selection("/sess", 0, false).expect("selection");

        assert_eq!(next, 1);
    }

    #[test]
    fn slash_command_hint_surfaces_next_required_argument() {
        let hint = slash_command_hint("/session ", 0).expect("hint");

        let arguments = hint.arguments.expect("arguments");
        assert_eq!(
            arguments.next,
            Some(SlashCommandArgumentSpec {
                placeholder: "<session-ref>",
                required: true,
            })
        );
        assert!(arguments.provided.is_empty());
        assert_eq!(hint.selected_match_index, 0);
    }

    #[test]
    fn slash_command_hint_tracks_argument_progress() {
        let hint = slash_command_hint("/spawn_task reviewer", 0).expect("hint");

        let arguments = hint.arguments.expect("arguments");
        assert_eq!(arguments.provided.len(), 1);
        assert_eq!(arguments.provided[0].placeholder, "<role>");
        assert_eq!(arguments.provided[0].value, "reviewer");
        assert_eq!(
            arguments.next,
            Some(SlashCommandArgumentSpec {
                placeholder: "<prompt>",
                required: true,
            })
        );
    }

    #[test]
    fn slash_command_hint_browses_all_commands_from_empty_slash() {
        let hint = slash_command_hint("/", 0).expect("hint");

        assert_eq!(hint.selected.name, "help");
        assert_eq!(hint.selected_match_index, 0);
        assert!(hint.matches.len() > 10);
        assert!(hint.matches.iter().any(|spec| spec.name == "spawn_task"));
    }

    #[test]
    fn slash_enter_action_completes_ambiguous_partial_command() {
        let action = resolve_slash_enter_action("/sess", 0).expect("action");

        assert_eq!(
            action,
            SlashCommandEnterAction::Complete {
                input: "/sessions ".to_string(),
                index: 0,
            }
        );
    }

    #[test]
    fn slash_enter_action_executes_unique_no_arg_command() {
        let action = resolve_slash_enter_action("/he", 0).expect("action");

        assert_eq!(
            action,
            SlashCommandEnterAction::Execute("/help".to_string())
        );
    }

    #[test]
    fn exact_required_command_is_prioritized_in_hint() {
        let hint = slash_command_hint("/session", 0).expect("hint");

        assert_eq!(hint.selected.name, "session");
        assert!(hint.exact);
    }

    #[test]
    fn slash_enter_action_accepts_required_argument_command_before_running() {
        let action = resolve_slash_enter_action("/session", 0).expect("action");

        assert_eq!(
            action,
            SlashCommandEnterAction::Complete {
                input: "/session ".to_string(),
                index: 0,
            }
        );
    }
}
