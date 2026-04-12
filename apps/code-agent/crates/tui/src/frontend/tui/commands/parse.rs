use super::*;

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
            SlashSubcommand::Monitors { include_closed } => Self::Monitors {
                include_closed: include_closed
                    .into_iter()
                    .any(|value| value.eq_ignore_ascii_case("all")),
            },
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
            SlashSubcommand::StopMonitor {
                monitor_ref,
                reason,
            } => Self::StopMonitor {
                monitor_ref,
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
