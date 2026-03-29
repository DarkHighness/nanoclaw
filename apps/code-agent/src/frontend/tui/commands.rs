use clap::{CommandFactory, Parser, Subcommand};

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

#[cfg(test)]
mod tests {
    use super::{SlashCommand, parse_slash_command};

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
}
