pub(crate) enum SlashCommand<'a> {
    Status,
    Help,
    Tools,
    Skills,
    Steer { message: Option<&'a str> },
    Compact { notes: Option<&'a str> },
    Runs { query: Option<&'a str> },
    Run { run_ref: &'a str },
    ExportRun { run_ref: &'a str, path: &'a str },
    ExportTranscript { run_ref: &'a str, path: &'a str },
    Clear,
    Quit,
    InvalidUsage(&'static str),
    Unknown(&'a str),
}

pub(crate) fn parse_slash_command(input: &str) -> SlashCommand<'_> {
    let trimmed = input.trim();
    let body = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default();
    let remainder = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match name {
        "status" => SlashCommand::Status,
        "help" => SlashCommand::Help,
        "tools" => SlashCommand::Tools,
        "skills" => SlashCommand::Skills,
        "steer" => SlashCommand::Steer { message: remainder },
        "compact" => SlashCommand::Compact { notes: remainder },
        "runs" => SlashCommand::Runs { query: remainder },
        "run" => remainder
            .map(|run_ref| SlashCommand::Run { run_ref })
            .unwrap_or(SlashCommand::InvalidUsage("Usage: /run <id-prefix>")),
        "export_run" => parse_export_args(remainder)
            .map(|(run_ref, path)| SlashCommand::ExportRun { run_ref, path })
            .unwrap_or(SlashCommand::InvalidUsage(
                "Usage: /export_run <id-prefix> <path>",
            )),
        "export_transcript" => parse_export_args(remainder)
            .map(|(run_ref, path)| SlashCommand::ExportTranscript { run_ref, path })
            .unwrap_or(SlashCommand::InvalidUsage(
                "Usage: /export_transcript <id-prefix> <path>",
            )),
        "clear" => SlashCommand::Clear,
        "quit" | "exit" => SlashCommand::Quit,
        _ => SlashCommand::Unknown(trimmed),
    }
}

fn parse_export_args(input: Option<&str>) -> Option<(&str, &str)> {
    let input = input?;
    let mut parts = input.splitn(2, char::is_whitespace);
    let run_ref = parts.next()?.trim();
    let path = parts.next()?.trim();
    if run_ref.is_empty() || path.is_empty() {
        return None;
    }
    Some((run_ref, path))
}

#[cfg(test)]
mod tests {
    use super::{SlashCommand, parse_slash_command};

    #[test]
    fn parses_runs_query_with_spaces() {
        match parse_slash_command("/runs fix failing test") {
            SlashCommand::Runs { query } => {
                assert_eq!(query, Some("fix failing test"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn export_transcript_keeps_path_tail_intact() {
        match parse_slash_command("/export_transcript abc123 reports/run log.txt") {
            SlashCommand::ExportTranscript { run_ref, path } => {
                assert_eq!(run_ref, "abc123");
                assert_eq!(path, "reports/run log.txt");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn missing_run_ref_returns_usage_error() {
        match parse_slash_command("/run") {
            SlashCommand::InvalidUsage(message) => {
                assert_eq!(message, "Usage: /run <id-prefix>");
            }
            _ => panic!("unexpected command"),
        }
    }
}
