use clap::{CommandFactory, Parser, Subcommand};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Status,
    Help,
    Tools,
    Skills,
    Steer { message: Option<String> },
    Compact { notes: Option<String> },
    Sessions { query: Option<String> },
    Session { session_ref: String },
    ExportSession { session_ref: String, path: String },
    ExportTranscript { session_ref: String, path: String },
    Clear,
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
    Clear,
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
            SlashSubcommand::Steer { message } => Self::Steer {
                message: join_optional_tail(message),
            },
            SlashSubcommand::Compact { notes } => Self::Compact {
                notes: join_optional_tail(notes),
            },
            SlashSubcommand::Sessions { query } => Self::Sessions {
                query: join_optional_tail(query),
            },
            SlashSubcommand::Session { session_ref } => Self::Session { session_ref },
            SlashSubcommand::ExportSession { session_ref, path } => Self::ExportSession {
                session_ref,
                path: join_required_tail(path),
            },
            SlashSubcommand::ExportTranscript { session_ref, path } => Self::ExportTranscript {
                session_ref,
                path: join_required_tail(path),
            },
            SlashSubcommand::Clear => Self::Clear,
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
    fn missing_session_ref_returns_usage_error() {
        match parse_slash_command("/session") {
            SlashCommand::InvalidUsage(message) => {
                assert!(message.contains("Usage:"));
                assert!(message.contains("session <SESSION_REF>"));
            }
            _ => panic!("unexpected command"),
        }
    }
}
