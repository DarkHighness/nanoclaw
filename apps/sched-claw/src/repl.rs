use agent::runtime::{RuntimeObserver, RuntimeProgressEvent};
use anyhow::{Context, Result, bail};
use std::io::{self, Write};

use crate::app_config::CliOverrides;
use crate::bootstrap::BuiltRuntime;
use crate::daemon_protocol::SchedExtDaemonRequest;
use crate::display::{
    OutputStyle, render_daemon_response, render_doctor_report, render_session_detail,
    render_session_list, render_session_search_results, render_skill_detail, render_skill_list,
    render_tool_detail, render_tool_list,
};
use crate::doctor::collect_doctor_report;
use crate::history::SessionHistory;

pub async fn run_repl(host: &mut BuiltRuntime, mut output_style: OutputStyle) -> Result<()> {
    println!("sched-claw repl");
    println!(
        "Commands: :help, :format <table|plain>, :doctor, :tools, :tool <name>, :skills, :skill <name>, :sessions [query], :session <id>, :resume <id>, :daemon status, :daemon logs [N], :quit"
    );
    let stdin = io::stdin();
    let mut line = String::new();
    loop {
        print!("sched> ");
        io::stdout().flush()?;
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            println!();
            break;
        }
        match parse_repl_command(&line)? {
            ReplCommand::Quit => break,
            ReplCommand::Help => {
                println!("Type a normal prompt to run a turn.");
                println!(":format <table|plain>  switch local inspection output style");
                println!(":doctor                inspect host readiness for sched-claw");
                println!(":tools                 show the startup tool surface");
                println!(":tool <name>           inspect one tool from the startup catalog");
                println!(":skills                show available skills");
                println!(":skill <name>          inspect one skill from the startup catalog");
                println!(":sessions [query]      list persisted sessions or search them");
                println!(":session <id>          inspect one persisted session");
                println!(":resume <id>           attach the repl to a persisted session");
                println!(":daemon status         inspect the privileged daemon snapshot");
                println!(":daemon logs [N]       inspect daemon logs with an optional tail size");
                println!(":quit                  exit the repl");
            }
            ReplCommand::SetFormat(style) => {
                output_style = style;
                println!("output format: {}", output_style.as_str());
            }
            ReplCommand::Doctor => {
                let report = collect_doctor_report(&host.workspace_root, &host.config).await?;
                println!("{}", render_doctor_report(&report, output_style));
            }
            ReplCommand::Tools => {
                println!(
                    "{}",
                    render_tool_list(host.startup_catalog.tool_specs(), output_style)
                );
            }
            ReplCommand::ToolShow(name) => {
                let spec = host
                    .startup_catalog
                    .resolve_tool(&name)
                    .ok_or_else(|| anyhow::anyhow!("unknown tool `{name}`"))?;
                println!("{}", render_tool_detail(spec, output_style));
            }
            ReplCommand::Skills => {
                println!(
                    "{}",
                    render_skill_list(host.startup_catalog.skills(), output_style)
                );
            }
            ReplCommand::SkillShow(name) => {
                let skill = host
                    .startup_catalog
                    .resolve_skill(&name)
                    .ok_or_else(|| anyhow::anyhow!("unknown skill `{name}`"))?;
                println!("{}", render_skill_detail(skill, output_style));
            }
            ReplCommand::Sessions { query } => {
                let history = open_history(host).await?;
                if let Some(query) = query {
                    let results = history.search_sessions(&query).await?;
                    println!("{}", render_session_search_results(&results, output_style));
                } else {
                    let sessions = history.list_sessions().await?;
                    println!("{}", render_session_list(&sessions, output_style));
                }
            }
            ReplCommand::SessionShow(session_ref) => {
                let history = open_history(host).await?;
                let detail = history.load_session(&session_ref).await?;
                println!("{}", render_session_detail(&detail, output_style));
            }
            ReplCommand::Resume(session_ref) => {
                let history = open_history(host).await?;
                let (summary, runtime_session) =
                    history.load_resumable_session(&session_ref).await?;
                host.runtime.resume_session(runtime_session).await?;
                println!("resumed session {}", summary.session_id);
            }
            ReplCommand::DaemonStatus => {
                let response = host
                    .daemon_client
                    .send(&SchedExtDaemonRequest::Status {})
                    .await?;
                println!("{}", render_daemon_response(&response, output_style));
            }
            ReplCommand::DaemonLogs { tail_lines } => {
                let response = host
                    .daemon_client
                    .send(&SchedExtDaemonRequest::Logs { tail_lines })
                    .await?;
                println!("{}", render_daemon_response(&response, output_style));
            }
            ReplCommand::Prompt(prompt) => {
                let mut observer = StreamingObserver::default();
                host.runtime
                    .run_user_prompt_with_observer(prompt, &mut observer)
                    .await?;
                observer.finish()?;
            }
        }
    }
    Ok(())
}

pub async fn run_exec(host: &mut BuiltRuntime, prompt: String) -> Result<()> {
    let mut observer = StreamingObserver::default();
    host.runtime
        .run_user_prompt_with_observer(prompt, &mut observer)
        .await?;
    observer.finish()
}

async fn open_history(host: &BuiltRuntime) -> Result<SessionHistory> {
    SessionHistory::open(&host.workspace_root, &CliOverrides::default()).await
}

#[derive(Debug, PartialEq, Eq)]
enum ReplCommand {
    Help,
    Quit,
    SetFormat(OutputStyle),
    Doctor,
    Tools,
    ToolShow(String),
    Skills,
    SkillShow(String),
    Sessions { query: Option<String> },
    SessionShow(String),
    Resume(String),
    DaemonStatus,
    DaemonLogs { tail_lines: Option<usize> },
    Prompt(String),
}

fn parse_repl_command(input: &str) -> Result<ReplCommand> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("prompt cannot be empty");
    }
    if !trimmed.starts_with(':') {
        return Ok(ReplCommand::Prompt(trimmed.to_string()));
    }

    let mut parts = trimmed.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        ":quit" | ":exit" => Ok(ReplCommand::Quit),
        ":help" => Ok(ReplCommand::Help),
        ":doctor" => Ok(ReplCommand::Doctor),
        ":tools" => Ok(ReplCommand::Tools),
        ":skills" => Ok(ReplCommand::Skills),
        ":sessions" => Ok(ReplCommand::Sessions {
            query: {
                let query = parts.collect::<Vec<_>>().join(" ");
                let trimmed = query.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            },
        }),
        ":tool" => {
            let name = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: :tool <name>"))?;
            Ok(ReplCommand::ToolShow(name.to_string()))
        }
        ":skill" => {
            let name = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: :skill <name>"))?;
            Ok(ReplCommand::SkillShow(name.to_string()))
        }
        ":format" => {
            let value = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: :format <table|plain>"))?;
            match value {
                "table" => Ok(ReplCommand::SetFormat(OutputStyle::Table)),
                "plain" => Ok(ReplCommand::SetFormat(OutputStyle::Plain)),
                other => bail!("unsupported format `{other}`; expected `table` or `plain`"),
            }
        }
        ":session" => {
            let session_ref = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: :session <id|last>"))?;
            Ok(ReplCommand::SessionShow(session_ref.to_string()))
        }
        ":resume" => {
            let session_ref = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("usage: :resume <id|last>"))?;
            Ok(ReplCommand::Resume(session_ref.to_string()))
        }
        ":daemon" => match parts.next() {
            Some("status") => Ok(ReplCommand::DaemonStatus),
            Some("logs") => {
                let tail_lines = match parts.next() {
                    Some(value) => Some(
                        value
                            .parse::<usize>()
                            .with_context(|| format!("invalid log count `{value}`"))?,
                    ),
                    None => None,
                };
                Ok(ReplCommand::DaemonLogs { tail_lines })
            }
            Some(other) => bail!("unsupported daemon command `{other}`"),
            None => bail!("usage: :daemon <status|logs [N]>"),
        },
        other => bail!("unknown repl command `{other}`; use :help"),
    }
}

#[derive(Default)]
struct StreamingObserver {
    saw_text_delta: bool,
    needs_trailing_newline: bool,
}

impl StreamingObserver {
    fn finish(&mut self) -> Result<()> {
        if self.needs_trailing_newline {
            println!();
            self.needs_trailing_newline = false;
        }
        io::stdout().flush()?;
        Ok(())
    }
}

impl RuntimeObserver for StreamingObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> agent::runtime::Result<()> {
        match event {
            RuntimeProgressEvent::AssistantTextDelta { delta } => {
                self.saw_text_delta = true;
                self.needs_trailing_newline = true;
                print!("{delta}");
                io::stdout().flush()?;
            }
            RuntimeProgressEvent::ToolCallRequested { call } => {
                eprintln!("[tool] {}", call.tool_name);
            }
            RuntimeProgressEvent::Notification { source, message } => {
                eprintln!("[{source}] {message}");
            }
            RuntimeProgressEvent::ProviderRetryScheduled {
                retry_count,
                max_retries,
                remaining_retries,
                ..
            } => {
                eprintln!(
                    "[retry] attempt {} of {} scheduled (remaining {})",
                    retry_count, max_retries, remaining_retries
                );
            }
            RuntimeProgressEvent::TurnCompleted { assistant_text, .. } => {
                if !self.saw_text_delta && !assistant_text.is_empty() {
                    println!("{assistant_text}");
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputStyle, ReplCommand, parse_repl_command};

    #[test]
    fn parses_style_switch() {
        assert_eq!(
            parse_repl_command(":format plain").unwrap(),
            ReplCommand::SetFormat(OutputStyle::Plain)
        );
    }

    #[test]
    fn parses_doctor_command() {
        assert_eq!(parse_repl_command(":doctor").unwrap(), ReplCommand::Doctor);
    }

    #[test]
    fn parses_daemon_logs_tail() {
        assert_eq!(
            parse_repl_command(":daemon logs 12").unwrap(),
            ReplCommand::DaemonLogs {
                tail_lines: Some(12)
            }
        );
    }

    #[test]
    fn parses_session_search() {
        assert_eq!(
            parse_repl_command(":sessions wakeup latency").unwrap(),
            ReplCommand::Sessions {
                query: Some("wakeup latency".to_string())
            }
        );
    }

    #[test]
    fn parses_resume_command() {
        assert_eq!(
            parse_repl_command(":resume last").unwrap(),
            ReplCommand::Resume("last".to_string())
        );
    }

    #[test]
    fn parses_prompt_passthrough() {
        assert_eq!(
            parse_repl_command("inspect the scheduler").unwrap(),
            ReplCommand::Prompt("inspect the scheduler".to_string())
        );
    }
}
