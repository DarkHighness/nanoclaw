use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sched_claw::app_config::{CliOverrides, SchedClawConfig};
use sched_claw::bootstrap::load_bootstrap;
use sched_claw::daemon_client::SchedExtDaemonClient;
use sched_claw::daemon_protocol::{SchedExtDaemonRequest, SchedExtDaemonResponse};
use sched_claw::display::{
    OutputStyle, render_daemon_response, render_skill_detail, render_skill_list,
    render_tool_detail, render_tool_list,
};
use sched_claw::repl::{run_exec, run_repl};
use std::collections::BTreeMap;
use std::path::Path;
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
    Repl(ReplArgs),
    Tool(ToolArgs),
    Skill(SkillArgs),
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
struct ReplArgs {
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Clone, Args)]
struct OutputArgs {
    #[arg(long, value_enum, default_value_t = OutputStyle::Table)]
    style: OutputStyle,
}

#[derive(Debug, Args)]
struct ToolArgs {
    #[command(subcommand)]
    command: ToolCommand,
}

#[derive(Debug, Subcommand)]
enum ToolCommand {
    List(OutputArgs),
    Show(ToolShowArgs),
}

#[derive(Debug, Args)]
struct ToolShowArgs {
    #[arg(value_name = "NAME")]
    name: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct SkillArgs {
    #[command(subcommand)]
    command: SkillCommand,
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    List(OutputArgs),
    Show(SkillShowArgs),
}

#[derive(Debug, Args)]
struct SkillShowArgs {
    #[arg(value_name = "NAME")]
    name: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct DaemonArgs {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Status(OutputArgs),
    Activate(DaemonActivateArgs),
    Logs(DaemonLogsArgs),
    Stop(DaemonStopArgs),
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
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct DaemonLogsArgs {
    #[arg(long, value_name = "LINES")]
    tail_lines: Option<usize>,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct DaemonStopArgs {
    #[arg(long, value_name = "MS")]
    graceful_timeout_ms: Option<u64>,
    #[command(flatten)]
    output: OutputArgs,
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
        Some(Command::Tool(args)) => run_tool_command(&workspace_root, &overrides, args).await?,
        Some(Command::Skill(args)) => run_skill_command(&workspace_root, &overrides, args).await?,
        Some(Command::Daemon(args)) => {
            run_daemon_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::Exec(args)) => {
            let prompt = join_prompt(args.prompt)?;
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_exec(&mut host, prompt).await?;
        }
        Some(Command::Repl(args)) => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_repl(&mut host, args.output.style).await?;
        }
        None if !cli.prompt.is_empty() => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_exec(&mut host, join_prompt(cli.prompt)?).await?;
        }
        None => {
            let bootstrap = load_bootstrap(&workspace_root, &overrides).await?;
            let mut host = bootstrap.build_runtime().await?;
            run_repl(&mut host, OutputStyle::Table).await?;
        }
    }

    Ok(())
}

async fn run_tool_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: ToolArgs,
) -> Result<()> {
    let bootstrap = load_bootstrap(workspace_root, overrides).await?;
    let catalog = bootstrap.startup_catalog();
    match args.command {
        ToolCommand::List(output) => {
            println!("{}", render_tool_list(catalog.tool_specs(), output.style));
        }
        ToolCommand::Show(args) => {
            let spec = catalog
                .resolve_tool(&args.name)
                .with_context(|| format!("unknown tool `{}`", args.name))?;
            println!("{}", render_tool_detail(spec, args.output.style));
        }
    }
    Ok(())
}

async fn run_skill_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: SkillArgs,
) -> Result<()> {
    let bootstrap = load_bootstrap(workspace_root, overrides).await?;
    let catalog = bootstrap.startup_catalog();
    match args.command {
        SkillCommand::List(output) => {
            println!("{}", render_skill_list(catalog.skills(), output.style));
        }
        SkillCommand::Show(args) => {
            let skill = catalog
                .resolve_skill(&args.name)
                .with_context(|| format!("unknown skill `{}`", args.name))?;
            println!("{}", render_skill_detail(skill, args.output.style));
        }
    }
    Ok(())
}

async fn run_daemon_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: DaemonArgs,
) -> Result<()> {
    let client = SchedExtDaemonClient::new(
        SchedClawConfig::load_from_dir(workspace_root, overrides)?.daemon,
    );
    let (request, style) = match args.command {
        DaemonCommand::Status(output) => (SchedExtDaemonRequest::Status {}, output.style),
        DaemonCommand::Activate(args) => (
            SchedExtDaemonRequest::Activate {
                label: args.label,
                argv: args.argv,
                cwd: args.cwd,
                env: args.env.into_iter().collect::<BTreeMap<_, _>>(),
                replace_existing: args.replace_existing,
            },
            args.output.style,
        ),
        DaemonCommand::Logs(args) => (
            SchedExtDaemonRequest::Logs {
                tail_lines: args.tail_lines,
            },
            args.output.style,
        ),
        DaemonCommand::Stop(args) => (
            SchedExtDaemonRequest::Stop {
                graceful_timeout_ms: args.graceful_timeout_ms,
            },
            args.output.style,
        ),
    };
    let response = client.send(&request).await?;
    match response {
        SchedExtDaemonResponse::Error { message } => anyhow::bail!(message),
        other => println!("{}", render_daemon_response(&other, style)),
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
