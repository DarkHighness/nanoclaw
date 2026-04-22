use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sched_claw::app_config::{CliOverrides, SchedClawConfig};
use sched_claw::bootstrap::load_bootstrap;
use sched_claw::daemon_client::SchedExtDaemonClient;
use sched_claw::daemon_protocol::{SchedExtDaemonRequest, SchedExtDaemonResponse};
use sched_claw::display::{
    OutputStyle, render_daemon_response, render_experiment_artifact, render_experiment_detail,
    render_experiment_list, render_experiment_score, render_session_detail,
    render_session_export_artifact, render_session_list, render_session_search_results,
    render_skill_detail, render_skill_list, render_tool_detail, render_tool_list,
};
use sched_claw::experiment::{
    CandidateSpec, ExperimentCatalog, ExperimentInitSpec, RecordedRun, SchedulerKind,
};
use sched_claw::history::SessionHistory;
use sched_claw::metrics::{MetricGoal, MetricTarget, parse_guardrail, parse_metric_assignment};
use sched_claw::repl::{run_exec, run_repl};
use sched_claw::workload::WorkloadContract;
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
    Sessions(SessionsArgs),
    Session(SessionArgs),
    Resume(ResumeArgs),
    ExportTranscript(ExportArgs),
    ExportEvents(ExportArgs),
    Experiment(ExperimentArgs),
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

#[derive(Debug, Args)]
struct SessionsArgs {
    #[command(flatten)]
    output: OutputArgs,
    #[arg(value_name = "QUERY")]
    query: Option<String>,
}

#[derive(Debug, Args)]
struct SessionArgs {
    #[arg(value_name = "SESSION")]
    session_ref: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct ResumeArgs {
    #[arg(value_name = "SESSION")]
    session_ref: String,
    #[command(flatten)]
    output: OutputArgs,
    #[arg(
        value_name = "PROMPT",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    prompt: Vec<String>,
}

#[derive(Debug, Args)]
struct ExportArgs {
    #[arg(value_name = "SESSION")]
    session_ref: String,
    #[arg(value_name = "PATH")]
    path: String,
}

#[derive(Debug, Args)]
struct ExperimentArgs {
    #[command(subcommand)]
    command: ExperimentCommand,
}

#[derive(Debug, Subcommand)]
enum ExperimentCommand {
    List(OutputArgs),
    Init(ExperimentInitArgs),
    Show(ExperimentShowArgs),
    AddCandidate(ExperimentAddCandidateArgs),
    RecordBaseline(ExperimentRecordBaselineArgs),
    RecordCandidate(ExperimentRecordCandidateArgs),
    Score(ExperimentShowArgs),
}

#[derive(Debug, Args)]
struct ExperimentShowArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct ExperimentInitArgs {
    #[arg(long, value_name = "ID")]
    id: String,
    #[arg(long, value_name = "NAME")]
    workload_name: String,
    #[arg(long, value_name = "TEXT")]
    workload_description: Option<String>,
    #[arg(long, value_name = "PATH")]
    workload_cwd: Option<String>,
    #[arg(long = "workload-arg", value_name = "ARG", allow_hyphen_values = true)]
    workload_args: Vec<String>,
    #[arg(long = "workload-env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    workload_env: Vec<(String, String)>,
    #[arg(long, value_name = "TEXT")]
    workload_scope: Option<String>,
    #[arg(long, value_name = "TEXT")]
    workload_phase: Option<String>,
    #[arg(long, value_name = "TEXT")]
    success_criteria: Option<String>,
    #[arg(long, value_name = "NAME")]
    primary_metric: String,
    #[arg(long, value_name = "GOAL", value_parser = parse_metric_goal_arg)]
    primary_goal: MetricGoal,
    #[arg(long, value_name = "UNIT")]
    primary_unit: Option<String>,
    #[arg(long = "guardrail", value_name = "NAME:GOAL:MAX_REGRESSION_PCT", value_parser = parse_guardrail_arg)]
    guardrails: Vec<sched_claw::metrics::Guardrail>,
}

#[derive(Debug, Args)]
struct ExperimentAddCandidateArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "ID")]
    candidate_id: String,
    #[arg(long, value_name = "NAME")]
    template: String,
    #[arg(long, value_name = "PATH")]
    source_path: Option<String>,
    #[arg(long, value_name = "TEXT")]
    build_command: Option<String>,
    #[arg(long = "daemon-arg", value_name = "ARG", allow_hyphen_values = true)]
    daemon_args: Vec<String>,
    #[arg(long = "knob", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    knobs: Vec<(String, String)>,
    #[arg(long, value_name = "TEXT")]
    notes: Option<String>,
}

#[derive(Debug, Args)]
struct ExperimentRecordBaselineArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "LABEL")]
    label: String,
    #[arg(long, value_name = "PATH")]
    artifact_dir: String,
    #[arg(long = "metric", value_name = "NAME=VALUE", value_parser = parse_metric_assignment_arg)]
    metrics: Vec<(String, f64)>,
    #[arg(long, value_name = "TEXT")]
    notes: Option<String>,
}

#[derive(Debug, Args)]
struct ExperimentRecordCandidateArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "ID")]
    candidate_id: String,
    #[arg(long, value_name = "LABEL")]
    label: String,
    #[arg(long, value_name = "PATH")]
    artifact_dir: String,
    #[arg(long = "metric", value_name = "NAME=VALUE", value_parser = parse_metric_assignment_arg)]
    metrics: Vec<(String, f64)>,
    #[arg(long, value_name = "TEXT")]
    notes: Option<String>,
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
        Some(Command::Sessions(args)) => {
            run_sessions_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::Session(args)) => {
            run_session_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::Resume(args)) => {
            run_resume_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::ExportTranscript(args)) => {
            run_export_transcript_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::ExportEvents(args)) => {
            run_export_events_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::Experiment(args)) => run_experiment_command(&workspace_root, args).await?,
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

async fn run_experiment_command(workspace_root: &Path, args: ExperimentArgs) -> Result<()> {
    let catalog = ExperimentCatalog::open(workspace_root)?;
    match args.command {
        ExperimentCommand::List(output) => {
            let summaries = catalog.list()?;
            println!("{}", render_experiment_list(&summaries, output.style));
        }
        ExperimentCommand::Init(args) => {
            let artifact = catalog.init(ExperimentInitSpec {
                experiment_id: args.id,
                workload: WorkloadContract {
                    name: args.workload_name,
                    description: args.workload_description,
                    cwd: args.workload_cwd,
                    argv: args.workload_args,
                    env: args.workload_env.into_iter().collect(),
                    scope: args.workload_scope,
                    phase: args.workload_phase,
                    success_criteria: args.success_criteria,
                },
                primary_metric: MetricTarget {
                    name: args.primary_metric,
                    goal: args.primary_goal,
                    unit: args.primary_unit,
                    notes: None,
                },
                guardrails: args.guardrails,
            })?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::Show(args) => {
            let experiment = catalog.load(&args.experiment_ref)?;
            println!(
                "{}",
                render_experiment_detail(&experiment, args.output.style)
            );
        }
        ExperimentCommand::AddCandidate(args) => {
            let artifact = catalog.add_candidate(
                &args.experiment_ref,
                CandidateSpec {
                    candidate_id: args.candidate_id,
                    template: args.template,
                    source_path: args.source_path,
                    build_command: args.build_command,
                    daemon_argv: args.daemon_args,
                    knobs: args.knobs.into_iter().collect(),
                    notes: args.notes,
                },
            )?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::RecordBaseline(args) => {
            let artifact = catalog.record_baseline(
                &args.experiment_ref,
                RecordedRun {
                    label: args.label,
                    recorded_at_unix_ms: sched_claw::experiment::now_unix_ms(),
                    scheduler: SchedulerKind::Cfs,
                    artifact_dir: args.artifact_dir,
                    metrics: args.metrics.into_iter().collect(),
                    notes: args.notes,
                },
            )?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::RecordCandidate(args) => {
            let artifact = catalog.record_candidate(
                &args.experiment_ref,
                &args.candidate_id,
                RecordedRun {
                    label: args.label,
                    recorded_at_unix_ms: sched_claw::experiment::now_unix_ms(),
                    scheduler: SchedulerKind::SchedExt,
                    artifact_dir: args.artifact_dir,
                    metrics: args.metrics.into_iter().collect(),
                    notes: args.notes,
                },
            )?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::Score(args) => {
            let report = catalog.score(&args.experiment_ref)?;
            println!("{}", render_experiment_score(&report, args.output.style));
        }
    }
    Ok(())
}

async fn run_sessions_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: SessionsArgs,
) -> Result<()> {
    let history = SessionHistory::open(workspace_root, overrides).await?;
    if let Some(query) = args.query {
        let results = history.search_sessions(&query).await?;
        println!(
            "{}",
            render_session_search_results(&results, args.output.style)
        );
    } else {
        let sessions = history.list_sessions().await?;
        println!("{}", render_session_list(&sessions, args.output.style));
    }
    Ok(())
}

async fn run_session_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: SessionArgs,
) -> Result<()> {
    let history = SessionHistory::open(workspace_root, overrides).await?;
    let detail = history.load_session(&args.session_ref).await?;
    println!("{}", render_session_detail(&detail, args.output.style));
    Ok(())
}

async fn run_resume_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: ResumeArgs,
) -> Result<()> {
    let history = SessionHistory::open(workspace_root, overrides).await?;
    let (summary, runtime_session) = history.load_resumable_session(&args.session_ref).await?;
    let bootstrap = load_bootstrap(workspace_root, overrides).await?;
    let mut host = bootstrap.build_runtime().await?;
    host.runtime.resume_session(runtime_session).await?;
    eprintln!("resumed session {}", summary.session_id);
    if args.prompt.is_empty() {
        run_repl(&mut host, args.output.style).await?;
    } else {
        run_exec(&mut host, join_prompt(args.prompt)?).await?;
    }
    Ok(())
}

async fn run_export_transcript_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: ExportArgs,
) -> Result<()> {
    let history = SessionHistory::open(workspace_root, overrides).await?;
    let artifact = history
        .export_transcript(workspace_root, &args.session_ref, &args.path)
        .await?;
    println!("{}", render_session_export_artifact(&artifact));
    Ok(())
}

async fn run_export_events_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: ExportArgs,
) -> Result<()> {
    let history = SessionHistory::open(workspace_root, overrides).await?;
    let artifact = history
        .export_events(workspace_root, &args.session_ref, &args.path)
        .await?;
    println!("{}", render_session_export_artifact(&artifact));
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

fn parse_metric_goal_arg(value: &str) -> Result<MetricGoal> {
    value.parse::<MetricGoal>()
}

fn parse_guardrail_arg(value: &str) -> Result<sched_claw::metrics::Guardrail> {
    parse_guardrail(value)
}

fn parse_metric_assignment_arg(value: &str) -> Result<(String, f64)> {
    parse_metric_assignment(value)
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

#[cfg(test)]
mod tests {
    use super::{Cli, Command, OutputStyle};
    use clap::Parser;

    #[test]
    fn parses_sessions_query_with_style_after_query() {
        let cli = Cli::try_parse_from(["sched-claw", "sessions", "agent-e2e", "--style", "plain"])
            .unwrap();

        match cli.command {
            Some(Command::Sessions(args)) => {
                assert_eq!(args.query.as_deref(), Some("agent-e2e"));
                assert_eq!(args.output.style, OutputStyle::Plain);
            }
            other => panic!("expected sessions command, got {other:?}"),
        }
    }

    #[test]
    fn parses_experiment_init_arguments() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "init",
            "--id",
            "demo",
            "--workload-name",
            "bench",
            "--primary-metric",
            "latency_ms",
            "--primary-goal",
            "minimize",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(_)) => {}
            other => panic!("expected experiment command, got {other:?}"),
        }
    }

    #[test]
    fn parses_candidate_daemon_arg_that_starts_with_dash() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "add-candidate",
            "demo",
            "--candidate-id",
            "cand-a",
            "--template",
            "locality",
            "--daemon-arg",
            "--sched-ext",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(_)) => {}
            other => panic!("expected experiment command, got {other:?}"),
        }
    }
}
