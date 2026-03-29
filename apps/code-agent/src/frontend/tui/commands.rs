use clap::{CommandFactory, Parser, Subcommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandSpec {
    pub(crate) section: &'static str,
    pub(crate) name: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) summary: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandHint {
    pub(crate) selected: SlashCommandSpec,
    pub(crate) matches: Vec<SlashCommandSpec>,
    pub(crate) arguments: Option<SlashCommandArgumentHint>,
    pub(crate) exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentHint {
    pub(crate) provided: Vec<SlashCommandArgumentValue>,
    pub(crate) next: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentValue {
    pub(crate) placeholder: &'static str,
    pub(crate) value: String,
}

const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        section: "Session",
        name: "help",
        usage: "help",
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
        name: "steer",
        usage: "steer <notes>",
        summary: "inject guidance",
    },
    SlashCommandSpec {
        section: "Session",
        name: "quit",
        usage: "quit",
        summary: "exit",
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
    Help,
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
    Compact {
        notes: Option<String>,
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
    Help,
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
    Compact {
        #[arg(
            value_name = "NOTES",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        notes: Vec<String>,
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
    #[command(alias = "exit")]
    Quit,
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

pub(crate) fn command_palette_lines() -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_section = None;
    for spec in SLASH_COMMAND_SPECS {
        if current_section != Some(spec.section) {
            current_section = Some(spec.section);
            lines.push(format!("## {}", spec.section));
        }
        lines.push(format!("/{}  {}", spec.usage, spec.summary));
    }
    lines
}

pub(crate) fn slash_command_hint(input: &str, selected_index: usize) -> Option<SlashCommandHint> {
    let (command_token, tail) = split_slash_input(input)?;
    let matches = matching_specs(command_token);
    if let Some(selected) = selected_spec(command_token, tail, selected_index, &matches) {
        return Some(SlashCommandHint {
            exact: command_token == selected.name,
            arguments: (command_token == selected.name)
                .then(|| build_argument_hint(selected, tail))
                .flatten(),
            selected,
            matches: matches.into_iter().take(5).collect(),
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

impl From<SlashSubcommand> for SlashCommand {
    fn from(value: SlashSubcommand) -> Self {
        match value {
            SlashSubcommand::Status => Self::Status,
            SlashSubcommand::Help => Self::Help,
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
            SlashSubcommand::Compact { notes } => Self::Compact {
                notes: join_optional_tail(notes),
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
    SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .filter(|spec| prefix.is_empty() || spec.name.starts_with(prefix))
        .collect()
}

fn selected_spec(
    command_token: &str,
    tail: Option<&str>,
    selected_index: usize,
    matches: &[SlashCommandSpec],
) -> Option<SlashCommandSpec> {
    if let Some(exact) = SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .find(|spec| spec.name == command_token)
    {
        return Some(exact);
    }
    if tail.is_some() {
        return None;
    }
    matches
        .get(selected_index.min(matches.len().saturating_sub(1)))
        .copied()
}

fn build_argument_hint(
    spec: SlashCommandSpec,
    tail: Option<&str>,
) -> Option<SlashCommandArgumentHint> {
    let placeholders = spec.usage.split_whitespace().skip(1).collect::<Vec<_>>();
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
            placeholder: *placeholder,
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
        SlashCommand, command_palette_lines, cycle_slash_command, parse_slash_command,
        slash_command_hint,
    };

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
    fn command_palette_includes_help_and_clear_alias() {
        let lines = command_palette_lines();

        assert!(lines.iter().any(|line| line == "## Session"));
        assert!(lines.iter().any(|line| line == "/help  browse commands"));
        assert!(lines.iter().any(|line| line == "/clear  alias of /new"));
    }

    #[test]
    fn slash_command_hint_matches_prefix() {
        let hint = slash_command_hint("/sess", 0).expect("hint");

        assert_eq!(hint.selected.name, "sessions");
        assert!(hint.matches.iter().any(|spec| spec.name == "sessions"));
        assert!(hint.matches.iter().any(|spec| spec.name == "session"));
        assert!(hint.arguments.is_none());
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
    fn slash_command_hint_surfaces_next_required_argument() {
        let hint = slash_command_hint("/session ", 0).expect("hint");

        let arguments = hint.arguments.expect("arguments");
        assert_eq!(arguments.next, Some("<session-ref>"));
        assert!(arguments.provided.is_empty());
    }

    #[test]
    fn slash_command_hint_tracks_argument_progress() {
        let hint = slash_command_hint("/spawn_task reviewer", 0).expect("hint");

        let arguments = hint.arguments.expect("arguments");
        assert_eq!(arguments.provided.len(), 1);
        assert_eq!(arguments.provided[0].placeholder, "<role>");
        assert_eq!(arguments.provided[0].value, "reviewer");
        assert_eq!(arguments.next, Some("<prompt>"));
    }
}
