use agent::runtime::{RuntimeObserver, RuntimeProgressEvent};
use anyhow::{Context, Result, bail};
use std::io::{self, Write};

use crate::bootstrap::BuiltRuntime;
use crate::daemon_protocol::SchedExtDaemonRequest;
use crate::display::{
    OutputStyle, render_daemon_response, render_skill_detail, render_skill_list,
    render_tool_detail, render_tool_list,
};

pub async fn run_repl(host: &mut BuiltRuntime, mut output_style: OutputStyle) -> Result<()> {
    println!("sched-claw repl");
    println!(
        "Commands: :help, :format <table|plain>, :tools, :tool <name>, :skills, :skill <name>, :daemon status, :daemon logs [N], :quit"
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
                println!(":tools                 show the startup tool surface");
                println!(":tool <name>           inspect one tool from the startup catalog");
                println!(":skills                show available skills");
                println!(":skill <name>          inspect one skill from the startup catalog");
                println!(":daemon status         inspect the privileged daemon snapshot");
                println!(":daemon logs [N]       inspect daemon logs with an optional tail size");
                println!(":quit                  exit the repl");
            }
            ReplCommand::SetFormat(style) => {
                output_style = style;
                println!("output format: {}", output_style.as_str());
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

#[derive(Debug, PartialEq, Eq)]
enum ReplCommand {
    Help,
    Quit,
    SetFormat(OutputStyle),
    Tools,
    ToolShow(String),
    Skills,
    SkillShow(String),
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
        ":tools" => Ok(ReplCommand::Tools),
        ":skills" => Ok(ReplCommand::Skills),
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
    fn parses_daemon_logs_tail() {
        assert_eq!(
            parse_repl_command(":daemon logs 12").unwrap(),
            ReplCommand::DaemonLogs {
                tail_lines: Some(12)
            }
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
