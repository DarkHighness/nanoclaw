#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TuiCommand {
    Quit,
    Clear,
    Status,
    Compact {
        instructions: Option<String>,
    },
    Runs {
        query: Option<String>,
    },
    Run {
        run_ref: String,
    },
    ExportRun {
        run_ref: String,
        path: String,
    },
    ExportTranscript {
        run_ref: String,
        path: String,
    },
    Skills {
        query: Option<String>,
    },
    Skill {
        skill_name: String,
    },
    Tools,
    Hooks,
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
}

pub fn parse_command(input: &str, prefix: &str) -> Option<TuiCommand> {
    let input = input.trim();
    if !input.starts_with(prefix) {
        return None;
    }
    let body = input.trim_start_matches(prefix).trim();
    let mut parts = body.split_whitespace();
    let command = parts.next()?;
    match command {
        "quit" | "q" => Some(TuiCommand::Quit),
        "clear" => Some(TuiCommand::Clear),
        "status" => Some(TuiCommand::Status),
        "compact" => {
            let instructions = parts.collect::<Vec<_>>().join(" ");
            Some(TuiCommand::Compact {
                instructions: if instructions.is_empty() {
                    None
                } else {
                    Some(instructions)
                },
            })
        }
        "runs" => {
            let query = parts.collect::<Vec<_>>().join(" ");
            Some(TuiCommand::Runs {
                query: if query.is_empty() { None } else { Some(query) },
            })
        }
        "run" => Some(TuiCommand::Run {
            run_ref: parts.next()?.to_string(),
        }),
        "export_run" => Some(TuiCommand::ExportRun {
            run_ref: parts.next()?.to_string(),
            path: parts.next()?.to_string(),
        }),
        "export_transcript" => Some(TuiCommand::ExportTranscript {
            run_ref: parts.next()?.to_string(),
            path: parts.next()?.to_string(),
        }),
        "skills" => {
            let query = parts.collect::<Vec<_>>().join(" ");
            Some(TuiCommand::Skills {
                query: if query.is_empty() { None } else { Some(query) },
            })
        }
        "skill" => Some(TuiCommand::Skill {
            skill_name: parts.next()?.to_string(),
        }),
        "tools" => Some(TuiCommand::Tools),
        "hooks" => Some(TuiCommand::Hooks),
        "mcp" => Some(TuiCommand::Mcp),
        "prompts" => Some(TuiCommand::Prompts),
        "resources" => Some(TuiCommand::Resources),
        "prompt" => Some(TuiCommand::Prompt {
            server_name: parts.next()?.to_string(),
            prompt_name: parts.next()?.to_string(),
        }),
        "resource" => Some(TuiCommand::Resource {
            server_name: parts.next()?.to_string(),
            uri: parts.next()?.to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{TuiCommand, parse_command};

    #[test]
    fn parses_basic_commands() {
        assert_eq!(parse_command("/quit", "/"), Some(TuiCommand::Quit));
        assert_eq!(parse_command("/status", "/"), Some(TuiCommand::Status));
        assert_eq!(
            parse_command("/compact preserve unresolved errors", "/"),
            Some(TuiCommand::Compact {
                instructions: Some("preserve unresolved errors".to_string()),
            })
        );
        assert_eq!(
            parse_command("/runs", "/"),
            Some(TuiCommand::Runs { query: None })
        );
        assert_eq!(
            parse_command("/runs release notes", "/"),
            Some(TuiCommand::Runs {
                query: Some("release notes".to_string()),
            })
        );
        assert_eq!(
            parse_command("/run abc123", "/"),
            Some(TuiCommand::Run {
                run_ref: "abc123".to_string(),
            })
        );
        assert_eq!(
            parse_command("/export_run abc123 out/run.jsonl", "/"),
            Some(TuiCommand::ExportRun {
                run_ref: "abc123".to_string(),
                path: "out/run.jsonl".to_string(),
            })
        );
        assert_eq!(
            parse_command("/skills pdf", "/"),
            Some(TuiCommand::Skills {
                query: Some("pdf".to_string()),
            })
        );
        assert_eq!(
            parse_command("/skill pdf", "/"),
            Some(TuiCommand::Skill {
                skill_name: "pdf".to_string(),
            })
        );
        assert_eq!(parse_command("/tools", "/"), Some(TuiCommand::Tools));
        assert_eq!(
            parse_command("/prompt fs code_review", "/"),
            Some(TuiCommand::Prompt {
                server_name: "fs".to_string(),
                prompt_name: "code_review".to_string(),
            })
        );
        assert_eq!(parse_command("hello", "/"), None);
    }
}
