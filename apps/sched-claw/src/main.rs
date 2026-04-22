use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sched_claw::app_config::{CliOverrides, SchedClawConfig};
use sched_claw::bootstrap::load_bootstrap;
use sched_claw::daemon_client::{SchedExtDaemonClient, render_response_text};
use sched_claw::daemon_protocol::{SchedExtDaemonRequest, SchedExtDaemonResponse};
use sched_claw::repl::{run_exec, run_repl};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(disable_help_subcommand = true, subcommand_precedence_over_arg = true)]
struct Cli {
    #[arg(long, value_name = "TEXT")]
    system_prompt: Option<String>,
    #[arg(long = "skill-root", value_name = "PATH")]
    skill_roots: Vec<PathBuf>,
    #[arg(long = "daemon-socket", value_name = "PATH")]
    daemon_socket: Option<PathBuf>,
    #[arg(long = "sandbox-fail-if-unavailable", value_name = "BOOL")]
    sandbox_fail_if_unavailable: Option<bool>,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(
        value_name = "PROMPT",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    prompt: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Exec(PromptArgs),
    Repl,
    Tool(ToolArgs),
    Daemon(DaemonArgs),
}

#[derive(Debug, Args)]
struct PromptArgs {
    #[arg(
        value_name = "PROMPT",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    prompt: Vec<String>,
}

#[derive(Debug, Args)]
struct ToolArgs {
    #[command(subcommand)]
    command: ToolCommand,
}

#[derive(Debug, Subcommand)]
enum ToolCommand {
    List,
}

#[derive(Debug, Args)]
struct DaemonArgs {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Status,
    Activate(DaemonActivateArgs),
    Logs {
        #[arg(long, value_name = "LINES")]
        tail_lines: Option<usize>,
    },
    Stop {
        #[arg(long, value_name = "MS")]
        graceful_timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Args)]
struct DaemonActivateArgs {
    #[arg(long, value_name = "TEXT")]
    label: Option<String>,
    #[arg(long, value_name = "PATH")]
    cwd: Option<String>,
    #[arg(long)]
    replace_existing: bool,
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    env: Vec<(String, String)>,
    #[arg(
        value_name = "ARGV",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    argv: Vec<String>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let workspace_root = std::env::current_dir().context("failed to resolve current directory")?;
    let overrides = CliOverrides {
        system_prompt: cli.system_prompt,
        skill_roots: cli.skill_roots,
        daemon_socket: cli.daemon_socket,
        sandbox_fail_if_unavailable: cli.sandbox_fail_if_unavailable,
    };

    match cli.command {
        Some(Command::Tool(ToolArgs {
            command: ToolCommand::List,
        })) => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            for tool_name in bootstrap.tool_names() {
                println!("{tool_name}");
            }
        }
        Some(Command::Daemon(args)) => {
            let client = SchedExtDaemonClient::new(
                SchedClawConfig::load_from_dir(&workspace_root, &overrides)?.daemon,
            );
            let request = match args.command {
                DaemonCommand::Status => SchedExtDaemonRequest::Status {},
                DaemonCommand::Activate(args) => SchedExtDaemonRequest::Activate {
                    label: args.label,
                    argv: args.argv,
                    cwd: args.cwd,
                    env: args.env.into_iter().collect::<BTreeMap<_, _>>(),
                    replace_existing: args.replace_existing,
                },
                DaemonCommand::Logs { tail_lines } => SchedExtDaemonRequest::Logs { tail_lines },
                DaemonCommand::Stop {
                    graceful_timeout_ms,
                } => SchedExtDaemonRequest::Stop {
                    graceful_timeout_ms,
                },
            };
            let response = client.send(&request).await?;
            match response {
                SchedExtDaemonResponse::Error { message } => anyhow::bail!(message),
                other => println!("{}", render_response_text(&other)),
            }
        }
        Some(Command::Exec(args)) => {
            let prompt = join_prompt(args.prompt)?;
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_exec(&mut host, prompt).await?;
        }
        Some(Command::Repl) => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_repl(&mut host).await?;
        }
        None if !cli.prompt.is_empty() => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_exec(&mut host, join_prompt(cli.prompt)?).await?;
        }
        None => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_repl(&mut host).await?;
        }
    }

    Ok(())
}

fn join_prompt(parts: Vec<String>) -> Result<String> {
    let prompt = parts.join(" ");
    if prompt.trim().is_empty() {
        anyhow::bail!("prompt cannot be empty");
    }
    Ok(prompt)
}

fn parse_key_value_arg(value: &str) -> Result<(String, String)> {
    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("expected KEY=VALUE, got `{value}`"))?;
    let key = key.trim();
    if key.is_empty() {
        anyhow::bail!("expected non-empty key in KEY=VALUE");
    }
    Ok((key.to_string(), value.to_string()))
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_target(false)
        .without_time()
        .try_init();
}
