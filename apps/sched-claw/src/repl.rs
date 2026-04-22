use agent::runtime::{RuntimeObserver, RuntimeProgressEvent};
use anyhow::Result;
use std::io::{self, Write};

use crate::bootstrap::BuiltRuntime;

pub async fn run_repl(host: &mut BuiltRuntime) -> Result<()> {
    println!("sched-claw repl");
    println!("Commands: :help, :tools, :quit");
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
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        match prompt {
            ":quit" | ":exit" => break,
            ":help" => {
                println!("Type a normal prompt to run a turn.");
                println!(":tools  show the model-visible tool surface");
                println!(":quit   exit the repl");
                continue;
            }
            ":tools" => {
                for tool_name in &host.tool_names {
                    println!("{tool_name}");
                }
                continue;
            }
            _ => {}
        }

        let mut observer = StreamingObserver::default();
        host.runtime
            .run_user_prompt_with_observer(prompt.to_string(), &mut observer)
            .await?;
        observer.finish()?;
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
