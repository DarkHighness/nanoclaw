use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sched_claw::app_config::{CliOverrides, SchedClawConfig};
use sched_claw::bootstrap::load_bootstrap;
use sched_claw::build_capture::{BuildCaptureOptions, CliVerifierBackend, capture_candidate_build};
use sched_claw::candidate_templates::{
    find_template, materialize_template, render_build_command, template_specs,
};
use sched_claw::daemon_client::SchedExtDaemonClient;
use sched_claw::daemon_protocol::{SchedExtDaemonRequest, SchedExtDaemonResponse};
use sched_claw::deployment::{DeployOverrides, build_activation_plan};
use sched_claw::display::{
    OutputStyle, render_candidate_build_capture, render_daemon_response, render_doctor_report,
    render_experiment_artifact, render_experiment_detail, render_experiment_list,
    render_experiment_score, render_session_detail, render_session_export_artifact,
    render_session_list, render_session_search_results, render_skill_detail, render_skill_list,
    render_template_detail, render_template_list, render_tool_detail, render_tool_list,
    render_workload_run_capture,
};
use sched_claw::doctor::collect_doctor_report;
use sched_claw::experiment::{
    CandidateRecord, CandidateSpec, CommandStatus, DeploymentRecord, EvaluationPolicy,
    ExperimentCatalog, ExperimentInitSpec, RecordedRun, SchedulerKind,
};
use sched_claw::history::SessionHistory;
use sched_claw::metrics::{
    MeasurementBasis, MetricGoal, MetricTarget, PerformancePolicy, PerformancePreference,
    infer_performance_policy, parse_guardrail, parse_metric_assignment, parse_metric_target,
};
use sched_claw::repl::{run_exec, run_repl};
use sched_claw::run_capture::{
    RunFailureMode, WorkloadRunOptions, attach_daemon_logs, capture_workload_run,
};
use sched_claw::workload::{WorkloadContract, WorkloadTarget};
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
    Template(TemplateArgs),
    Tool(ToolArgs),
    Skill(SkillArgs),
    Doctor(DoctorArgs),
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
    SetEvaluationPolicy(ExperimentSetEvaluationPolicyArgs),
    AddCandidate(ExperimentAddCandidateArgs),
    SetCandidate(ExperimentAddCandidateArgs),
    Materialize(ExperimentMaterializeArgs),
    Build(ExperimentBuildArgs),
    Run(ExperimentRunArgs),
    RecordBaseline(ExperimentRecordBaselineArgs),
    RecordCandidate(ExperimentRecordCandidateArgs),
    Score(ExperimentShowArgs),
    Deploy(ExperimentDeployArgs),
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
    #[arg(long, value_name = "PID")]
    target_pid: Option<u32>,
    #[arg(long, value_name = "UID")]
    target_uid: Option<u32>,
    #[arg(long, value_name = "GID")]
    target_gid: Option<u32>,
    #[arg(long, value_name = "PATH")]
    target_cgroup: Option<String>,
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
    #[arg(long, value_name = "PREFERENCE", value_parser = parse_performance_preference_arg)]
    performance_preference: Option<PerformancePreference>,
    #[arg(long, value_name = "BASIS", value_parser = parse_measurement_basis_arg)]
    performance_basis: Option<MeasurementBasis>,
    #[arg(long = "proxy-metric", value_name = "NAME:GOAL[:UNIT]", value_parser = parse_metric_target_arg)]
    proxy_metrics: Vec<MetricTarget>,
    #[arg(long, value_name = "TEXT")]
    performance_notes: Option<String>,
    #[arg(long = "guardrail", value_name = "NAME:GOAL:MAX_REGRESSION_PCT", value_parser = parse_guardrail_arg)]
    guardrails: Vec<sched_claw::metrics::Guardrail>,
    #[arg(long, value_name = "N", default_value_t = 1usize)]
    min_baseline_runs: usize,
    #[arg(long, value_name = "N", default_value_t = 1usize)]
    min_candidate_runs: usize,
    #[arg(long, value_name = "PCT")]
    min_primary_improvement_pct: Option<f64>,
    #[arg(long, value_name = "PCT")]
    max_primary_relative_spread_pct: Option<f64>,
}

#[derive(Debug, Args)]
struct ExperimentSetEvaluationPolicyArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "N")]
    min_baseline_runs: Option<usize>,
    #[arg(long, value_name = "N")]
    min_candidate_runs: Option<usize>,
    #[arg(long, value_name = "PCT")]
    min_primary_improvement_pct: Option<f64>,
    #[arg(long, value_name = "PCT")]
    max_primary_relative_spread_pct: Option<f64>,
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
    #[arg(long, value_name = "PATH")]
    object_path: Option<String>,
    #[arg(long, value_name = "TEXT")]
    build_command: Option<String>,
    #[arg(long = "daemon-arg", value_name = "ARG", allow_hyphen_values = true)]
    daemon_args: Vec<String>,
    #[arg(long = "daemon-cwd", value_name = "PATH")]
    daemon_cwd: Option<String>,
    #[arg(long = "daemon-env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    daemon_env: Vec<(String, String)>,
    #[arg(long = "knob", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    knobs: Vec<(String, String)>,
    #[arg(long, value_name = "TEXT")]
    notes: Option<String>,
}

#[derive(Debug, Args)]
struct ExperimentMaterializeArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "ID")]
    candidate_id: String,
    #[arg(long, value_name = "NAME")]
    template: Option<String>,
    #[arg(long = "knob", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    knobs: Vec<(String, String)>,
    #[arg(long, value_name = "PATH")]
    output: Option<String>,
    #[arg(long, value_name = "TEXT")]
    build_command: Option<String>,
    #[arg(long, value_name = "PATH")]
    loader: Option<String>,
    #[arg(long = "loader-arg", value_name = "ARG", allow_hyphen_values = true)]
    loader_args: Vec<String>,
    #[arg(long = "daemon-cwd", value_name = "PATH")]
    daemon_cwd: Option<String>,
    #[arg(long = "daemon-env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    daemon_env: Vec<(String, String)>,
    #[arg(long, value_name = "TEXT")]
    notes: Option<String>,
}

#[derive(Debug, Args)]
struct ExperimentBuildArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "ID")]
    candidate_id: String,
    #[arg(long)]
    skip_verifier: bool,
    #[arg(long, value_name = "PATH", default_value = "bpftool")]
    bpftool: String,
    #[arg(long, value_enum, default_value_t = CliVerifierBackend::BpftoolProgLoadall)]
    verifier_backend: CliVerifierBackend,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct ExperimentRunArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "ID")]
    candidate_id: Option<String>,
    #[arg(long, value_name = "LABEL")]
    label: Option<String>,
    #[arg(long, value_name = "PATH")]
    artifact_dir: Option<String>,
    #[arg(long, value_name = "NAME", default_value = "metrics.env")]
    metrics_file: String,
    #[arg(long, value_name = "SECONDS")]
    timeout_seconds: Option<u64>,
    #[arg(long, value_name = "SECONDS")]
    lease_seconds: Option<u64>,
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    env: Vec<(String, String)>,
    #[arg(long)]
    replace_existing: bool,
    #[arg(long)]
    allow_unverified_build: bool,
    #[arg(long, value_enum, default_value_t = RunFailureMode::Record)]
    on_failure: RunFailureMode,
    #[arg(long, value_name = "LINES", default_value_t = 500usize)]
    daemon_log_tail_lines: usize,
    #[command(flatten)]
    output: OutputArgs,
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

#[derive(Debug, Args)]
struct ExperimentDeployArgs {
    #[arg(value_name = "EXPERIMENT")]
    experiment_ref: String,
    #[arg(long, value_name = "ID")]
    candidate_id: String,
    #[arg(long, value_name = "TEXT")]
    label: Option<String>,
    #[arg(long, value_name = "PATH")]
    loader: Option<String>,
    #[arg(long = "loader-arg", value_name = "ARG", allow_hyphen_values = true)]
    loader_args: Vec<String>,
    #[arg(long, value_name = "PATH")]
    cwd: Option<String>,
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_key_value_arg)]
    env: Vec<(String, String)>,
    #[arg(long, value_name = "SECONDS")]
    lease_seconds: Option<u64>,
    #[arg(long)]
    replace_existing: bool,
    #[arg(long)]
    allow_unverified_build: bool,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct TemplateArgs {
    #[command(subcommand)]
    command: TemplateCommand,
}

#[derive(Debug, Subcommand)]
enum TemplateCommand {
    List(OutputArgs),
    Show(TemplateShowArgs),
}

#[derive(Debug, Args)]
struct TemplateShowArgs {
    #[arg(value_name = "NAME")]
    name: String,
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
struct DoctorArgs {
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
    #[arg(long, value_name = "SECONDS")]
    lease_seconds: Option<u64>,
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
        Some(Command::Experiment(args)) => {
            run_experiment_command(&workspace_root, &overrides, args).await?
        }
        Some(Command::Template(args)) => run_template_command(args).await?,
        Some(Command::Tool(args)) => run_tool_command(&workspace_root, &overrides, args).await?,
        Some(Command::Skill(args)) => run_skill_command(&workspace_root, &overrides, args).await?,
        Some(Command::Doctor(args)) => {
            run_doctor_command(&workspace_root, &overrides, args).await?
        }
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

async fn run_experiment_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: ExperimentArgs,
) -> Result<()> {
    let catalog = ExperimentCatalog::open(workspace_root)?;
    match args.command {
        ExperimentCommand::List(output) => {
            let summaries = catalog.list()?;
            println!("{}", render_experiment_list(&summaries, output.style));
        }
        ExperimentCommand::Init(args) => {
            let target = build_workload_target(&args)?;
            let primary_metric = MetricTarget {
                name: args.primary_metric,
                goal: args.primary_goal,
                unit: args.primary_unit,
                notes: None,
            };
            let performance_policy = resolve_performance_policy(
                args.performance_preference,
                args.performance_basis,
                args.performance_notes,
                &primary_metric,
                &args.guardrails,
                args.proxy_metrics,
            )?;
            let evaluation_policy = EvaluationPolicy {
                min_baseline_runs: args.min_baseline_runs,
                min_candidate_runs: args.min_candidate_runs,
                min_primary_improvement_pct: args.min_primary_improvement_pct,
                max_primary_relative_spread_pct: args.max_primary_relative_spread_pct,
            };
            let artifact = catalog.init(ExperimentInitSpec {
                experiment_id: args.id,
                workload: WorkloadContract {
                    name: args.workload_name,
                    description: args.workload_description,
                    target,
                    cwd: args.workload_cwd,
                    argv: args.workload_args,
                    env: args.workload_env.into_iter().collect(),
                    scope: args.workload_scope,
                    phase: args.workload_phase,
                    success_criteria: args.success_criteria,
                },
                primary_metric,
                performance_policy,
                evaluation_policy,
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
        ExperimentCommand::SetEvaluationPolicy(args) => {
            let experiment = catalog.load(&args.experiment_ref)?;
            let mut policy = experiment.manifest.evaluation_policy;
            if let Some(value) = args.min_baseline_runs {
                policy.min_baseline_runs = value;
            }
            if let Some(value) = args.min_candidate_runs {
                policy.min_candidate_runs = value;
            }
            if let Some(value) = args.min_primary_improvement_pct {
                policy.min_primary_improvement_pct = Some(value);
            }
            if let Some(value) = args.max_primary_relative_spread_pct {
                policy.max_primary_relative_spread_pct = Some(value);
            }
            let artifact = catalog.set_evaluation_policy(&args.experiment_ref, policy)?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::AddCandidate(args) => {
            let artifact = catalog.add_candidate(
                &args.experiment_ref,
                CandidateSpec {
                    candidate_id: args.candidate_id,
                    template: args.template,
                    source_path: args.source_path,
                    object_path: args.object_path,
                    build_command: args.build_command,
                    daemon_argv: args.daemon_args,
                    daemon_cwd: args.daemon_cwd,
                    daemon_env: args.daemon_env.into_iter().collect(),
                    knobs: args.knobs.into_iter().collect(),
                    notes: args.notes,
                },
            )?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::SetCandidate(args) => {
            let artifact = catalog.set_candidate(
                &args.experiment_ref,
                CandidateSpec {
                    candidate_id: args.candidate_id,
                    template: args.template,
                    source_path: args.source_path,
                    object_path: args.object_path,
                    build_command: args.build_command,
                    daemon_argv: args.daemon_args,
                    daemon_cwd: args.daemon_cwd,
                    daemon_env: args.daemon_env.into_iter().collect(),
                    knobs: args.knobs.into_iter().collect(),
                    notes: args.notes,
                },
            )?;
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::Materialize(args) => {
            if !args.loader_args.is_empty() && args.loader.is_none() {
                anyhow::bail!("--loader-arg requires --loader");
            }
            let existing = catalog
                .load(&args.experiment_ref)?
                .manifest
                .candidates
                .into_iter()
                .find(|candidate| candidate.spec.candidate_id == args.candidate_id)
                .map(|candidate| candidate.spec);
            let template_name = args
                .template
                .clone()
                .or_else(|| existing.as_ref().map(|candidate| candidate.template.clone()))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "candidate {} does not exist yet; pass --template to materialize a new candidate",
                        args.candidate_id
                    )
                })?;
            let template = find_template(&template_name)
                .with_context(|| format!("unknown template `{template_name}`"))?;
            let mut knobs = existing
                .as_ref()
                .map(|candidate| candidate.knobs.clone())
                .unwrap_or_default();
            knobs.extend(args.knobs.into_iter());
            let materialized = materialize_template(
                workspace_root,
                &catalog.load(&args.experiment_ref)?.manifest.experiment_id,
                &args.candidate_id,
                template,
                &knobs,
                args.output.as_deref(),
            )?;
            let mut candidate = existing.unwrap_or(CandidateSpec {
                candidate_id: args.candidate_id.clone(),
                template: template.name.to_string(),
                source_path: None,
                object_path: None,
                build_command: None,
                daemon_argv: Vec::new(),
                daemon_cwd: None,
                daemon_env: BTreeMap::new(),
                knobs: BTreeMap::new(),
                notes: None,
            });
            candidate.template = template.name.to_string();
            candidate.source_path = Some(materialized.relative_source_path.clone());
            candidate.object_path = Some(materialized.relative_object_path.clone());
            candidate.knobs = materialized.applied_knobs.clone();
            candidate.build_command = Some(args.build_command.unwrap_or_else(|| {
                render_build_command(
                    template,
                    &materialized.relative_source_path,
                    &materialized.relative_object_path,
                )
            }));
            if let Some(notes) = args.notes {
                candidate.notes = Some(notes);
            }
            if let Some(daemon_cwd) = args.daemon_cwd {
                candidate.daemon_cwd = Some(daemon_cwd);
            }
            if !args.daemon_env.is_empty() {
                candidate.daemon_env.extend(args.daemon_env.into_iter());
            }
            if args.loader.is_some() {
                let plan = build_activation_plan(
                    &catalog.load(&args.experiment_ref)?.manifest.experiment_id,
                    &candidate,
                    &DeployOverrides {
                        loader: args.loader,
                        loader_args: args.loader_args,
                        cwd: candidate.daemon_cwd.clone(),
                        env: candidate.daemon_env.clone(),
                        ..DeployOverrides::default()
                    },
                )?;
                candidate.daemon_argv = plan.argv;
                candidate.daemon_cwd = plan.cwd;
                candidate.daemon_env = plan.env;
            }
            let mut artifact = catalog.set_candidate(&args.experiment_ref, candidate)?;
            artifact
                .details
                .push(("source".to_string(), materialized.relative_source_path));
            artifact
                .details
                .push(("object".to_string(), materialized.relative_object_path));
            println!("{}", render_experiment_artifact(&artifact));
        }
        ExperimentCommand::Build(args) => {
            let experiment = catalog.load(&args.experiment_ref)?;
            let candidate = experiment
                .manifest
                .candidates
                .iter()
                .find(|candidate| candidate.spec.candidate_id == args.candidate_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown candidate {} in experiment {}",
                        args.candidate_id,
                        experiment.manifest.experiment_id
                    )
                })?;
            let record = capture_candidate_build(
                workspace_root,
                &experiment.manifest.experiment_id,
                &candidate.spec,
                &BuildCaptureOptions {
                    skip_verifier: args.skip_verifier,
                    bpftool_path: args.bpftool,
                    verifier_backend: args.verifier_backend,
                },
            )?;
            let artifact = catalog.record_candidate_build(
                &args.experiment_ref,
                &args.candidate_id,
                record.clone(),
            )?;
            println!(
                "{}",
                render_candidate_build_capture(
                    &experiment.manifest.experiment_id,
                    &args.candidate_id,
                    &artifact.manifest_path,
                    &record,
                    args.output.style,
                )
            );
        }
        ExperimentCommand::Run(args) => {
            let experiment = catalog.load(&args.experiment_ref)?;
            let label = args.label.clone().unwrap_or_else(|| {
                args.candidate_id
                    .as_deref()
                    .map(|candidate_id| {
                        format!("{candidate_id}-{}", sched_claw::experiment::now_unix_ms())
                    })
                    .unwrap_or_else(|| {
                        format!("baseline-{}", sched_claw::experiment::now_unix_ms())
                    })
            });

            let capture = if let Some(candidate_id) = &args.candidate_id {
                let candidate = experiment
                    .manifest
                    .candidates
                    .iter()
                    .find(|candidate| candidate.spec.candidate_id == *candidate_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "unknown candidate {} in experiment {}",
                            candidate_id,
                            experiment.manifest.experiment_id
                        )
                    })?;
                ensure_candidate_ready_for_rollout(candidate, args.allow_unverified_build)?;
                let plan = build_activation_plan(
                    &experiment.manifest.experiment_id,
                    &candidate.spec,
                    &DeployOverrides {
                        label: Some(format!("{}:{}", experiment.manifest.experiment_id, label)),
                        lease_timeout_ms: effective_run_lease_timeout_ms(
                            args.timeout_seconds,
                            args.lease_seconds,
                        ),
                        replace_existing: args.replace_existing,
                        ..DeployOverrides::default()
                    },
                )?;
                let client = SchedExtDaemonClient::new(
                    SchedClawConfig::load_from_dir(workspace_root, overrides)?.daemon,
                );
                let response = client
                    .send(&SchedExtDaemonRequest::Activate {
                        label: Some(plan.label.clone()),
                        argv: plan.argv.clone(),
                        cwd: plan.cwd.clone(),
                        env: plan.env.clone(),
                        lease_timeout_ms: plan.lease_timeout_ms,
                        replace_existing: plan.replace_existing,
                    })
                    .await?;
                if let SchedExtDaemonResponse::Error { message } = &response {
                    anyhow::bail!(message.clone());
                }
                let run_result = capture_workload_run(
                    workspace_root,
                    &experiment.manifest.experiment_id,
                    &experiment.manifest.workload,
                    &WorkloadRunOptions {
                        label: label.clone(),
                        scheduler: SchedulerKind::SchedExt,
                        candidate_id: Some(candidate_id.clone()),
                        artifact_dir: args.artifact_dir.clone(),
                        metrics_file_name: args.metrics_file.clone(),
                        timeout_seconds: args.timeout_seconds,
                        extra_env: args.env.clone().into_iter().collect(),
                    },
                )
                .await;
                let stop_response = client
                    .send(&SchedExtDaemonRequest::Stop {
                        graceful_timeout_ms: None,
                    })
                    .await;
                let mut capture = run_result?;
                match stop_response? {
                    SchedExtDaemonResponse::Error { message } => anyhow::bail!(message),
                    _ => {}
                }
                let logs = client.logs(Some(args.daemon_log_tail_lines)).await;
                if let Ok(snapshot) = logs {
                    attach_daemon_logs(workspace_root, &mut capture, &snapshot)?;
                }
                let artifact = catalog.record_candidate(
                    &args.experiment_ref,
                    candidate_id,
                    capture.run.clone(),
                )?;
                println!("{}", render_experiment_artifact(&artifact));
                capture
            } else {
                let capture = capture_workload_run(
                    workspace_root,
                    &experiment.manifest.experiment_id,
                    &experiment.manifest.workload,
                    &WorkloadRunOptions {
                        label: label.clone(),
                        scheduler: SchedulerKind::Cfs,
                        candidate_id: None,
                        artifact_dir: args.artifact_dir.clone(),
                        metrics_file_name: args.metrics_file.clone(),
                        timeout_seconds: args.timeout_seconds,
                        extra_env: args.env.clone().into_iter().collect(),
                    },
                )
                .await?;
                let artifact =
                    catalog.record_baseline(&args.experiment_ref, capture.run.clone())?;
                println!("{}", render_experiment_artifact(&artifact));
                capture
            };

            println!(
                "{}",
                render_workload_run_capture(
                    &experiment.manifest.experiment_id,
                    args.candidate_id.as_deref(),
                    &experiment.manifest_path,
                    &capture,
                    args.output.style,
                )
            );

            if matches!(args.on_failure, RunFailureMode::Strict)
                && capture
                    .run
                    .notes
                    .as_deref()
                    .unwrap_or_default()
                    .contains("status=failed")
            {
                anyhow::bail!("workload run completed with a failure status");
            }
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
        ExperimentCommand::Deploy(args) => {
            let experiment = catalog.load(&args.experiment_ref)?;
            let candidate = experiment
                .manifest
                .candidates
                .iter()
                .find(|candidate| candidate.spec.candidate_id == args.candidate_id)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown candidate {} in experiment {}",
                        args.candidate_id,
                        experiment.manifest.experiment_id
                    )
                })?;
            ensure_candidate_ready_for_rollout(candidate, args.allow_unverified_build)?;
            let plan = build_activation_plan(
                &experiment.manifest.experiment_id,
                &candidate.spec,
                &DeployOverrides {
                    label: args.label,
                    loader: args.loader,
                    loader_args: args.loader_args,
                    cwd: args.cwd,
                    env: args.env.into_iter().collect(),
                    lease_timeout_ms: lease_seconds_to_ms(args.lease_seconds),
                    replace_existing: args.replace_existing,
                },
            )?;
            let client = SchedExtDaemonClient::new(
                SchedClawConfig::load_from_dir(workspace_root, overrides)?.daemon,
            );
            let response = client
                .send(&SchedExtDaemonRequest::Activate {
                    label: Some(plan.label.clone()),
                    argv: plan.argv.clone(),
                    cwd: plan.cwd.clone(),
                    env: plan.env.clone(),
                    lease_timeout_ms: plan.lease_timeout_ms,
                    replace_existing: plan.replace_existing,
                })
                .await?;
            if let SchedExtDaemonResponse::Error { message } = &response {
                anyhow::bail!(message.clone());
            }
            match &response {
                SchedExtDaemonResponse::Ack { snapshot, .. } => {
                    let active = snapshot.active.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "daemon acknowledged activation without an active deployment snapshot"
                        )
                    })?;
                    let artifact = catalog.record_deployment(
                        &args.experiment_ref,
                        DeploymentRecord {
                            candidate_id: candidate.spec.candidate_id.clone(),
                            requested_at_unix_ms: sched_claw::experiment::now_unix_ms(),
                            label: active.label.clone(),
                            daemon_pid: active.pid,
                            argv: active.argv.clone(),
                            cwd: Some(active.cwd.clone()),
                            env: plan.env,
                            source_path: plan.source_path,
                            lease_timeout_ms: plan.lease_timeout_ms,
                            replace_existing: plan.replace_existing,
                        },
                    )?;
                    println!("{}", render_experiment_artifact(&artifact));
                    println!("{}", render_daemon_response(&response, args.output.style));
                }
                other => anyhow::bail!("daemon returned unexpected response for deploy: {other:?}"),
            }
        }
    }
    Ok(())
}

async fn run_template_command(args: TemplateArgs) -> Result<()> {
    match args.command {
        TemplateCommand::List(output) => {
            println!("{}", render_template_list(template_specs(), output.style));
        }
        TemplateCommand::Show(args) => {
            let template = find_template(&args.name)
                .with_context(|| format!("unknown template `{}`", args.name))?;
            println!("{}", render_template_detail(template, args.output.style));
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

async fn run_doctor_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: DoctorArgs,
) -> Result<()> {
    let config = SchedClawConfig::load_from_dir(workspace_root, overrides)?;
    let report = collect_doctor_report(workspace_root, &config).await?;
    println!("{}", render_doctor_report(&report, args.output.style));
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
                lease_timeout_ms: lease_seconds_to_ms(args.lease_seconds),
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

fn lease_seconds_to_ms(seconds: Option<u64>) -> Option<u64> {
    seconds.map(|value| value.max(1).saturating_mul(1_000))
}

fn effective_run_lease_timeout_ms(
    timeout_seconds: Option<u64>,
    lease_seconds: Option<u64>,
) -> Option<u64> {
    lease_seconds_to_ms(lease_seconds).or_else(|| lease_seconds_to_ms(timeout_seconds))
}

fn ensure_candidate_ready_for_rollout(
    candidate: &CandidateRecord,
    allow_unverified_build: bool,
) -> Result<()> {
    if allow_unverified_build {
        return Ok(());
    }
    let Some(latest_build) = candidate.builds.last() else {
        anyhow::bail!(
            "candidate {} has no recorded build; run `sched-claw experiment build ...` first or pass --allow-unverified-build",
            candidate.spec.candidate_id
        );
    };
    if latest_build.build.status != CommandStatus::Success {
        anyhow::bail!(
            "candidate {} latest build status is {}; rerun `sched-claw experiment build ...` or pass --allow-unverified-build",
            candidate.spec.candidate_id,
            latest_build.build.status.as_str()
        );
    }
    if latest_build.verifier.status != CommandStatus::Success {
        anyhow::bail!(
            "candidate {} latest verifier status is {}; inspect {} and rerun `sched-claw experiment build ...` or pass --allow-unverified-build",
            candidate.spec.candidate_id,
            latest_build.verifier.status.as_str(),
            latest_build.verifier.stderr_path
        );
    }
    Ok(())
}

fn build_workload_target(args: &ExperimentInitArgs) -> Result<Option<WorkloadTarget>> {
    let selectors = [
        args.target_pid.is_some(),
        args.target_uid.is_some(),
        args.target_gid.is_some(),
        args.target_cgroup.is_some(),
    ];
    if selectors.into_iter().filter(|selected| *selected).count() > 1 {
        anyhow::bail!(
            "only one of --target-pid, --target-uid, --target-gid, or --target-cgroup may be set"
        );
    }
    if args.target_pid.is_some()
        || args.target_uid.is_some()
        || args.target_gid.is_some()
        || args.target_cgroup.is_some()
    {
        if args.workload_cwd.is_some()
            || !args.workload_args.is_empty()
            || !args.workload_env.is_empty()
        {
            anyhow::bail!(
                "script launch fields (--workload-cwd/--workload-arg/--workload-env) cannot be mixed with pid/uid/gid/cgroup targets"
            );
        }
    }
    Ok(if let Some(pid) = args.target_pid {
        Some(WorkloadTarget::Pid { pid })
    } else if let Some(uid) = args.target_uid {
        Some(WorkloadTarget::Uid { uid })
    } else if let Some(gid) = args.target_gid {
        Some(WorkloadTarget::Gid { gid })
    } else if let Some(path) = args.target_cgroup.clone() {
        Some(WorkloadTarget::Cgroup { path })
    } else if args.workload_cwd.is_some()
        || !args.workload_args.is_empty()
        || !args.workload_env.is_empty()
    {
        Some(WorkloadTarget::Script {
            cwd: args.workload_cwd.clone(),
            argv: args.workload_args.clone(),
            env: args.workload_env.iter().cloned().collect(),
        })
    } else {
        None
    })
}

fn resolve_performance_policy(
    preference: Option<PerformancePreference>,
    basis: Option<MeasurementBasis>,
    notes: Option<String>,
    primary_metric: &MetricTarget,
    guardrails: &[sched_claw::metrics::Guardrail],
    proxy_metrics: Vec<MetricTarget>,
) -> Result<PerformancePolicy> {
    let mut policy = infer_performance_policy(primary_metric, guardrails, proxy_metrics);
    if let Some(preference) = preference {
        policy.preference = preference;
    }
    if let Some(basis) = basis {
        policy.basis = basis;
    }
    policy.notes = notes;
    policy.validate()?;
    Ok(policy)
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

fn parse_performance_preference_arg(value: &str) -> Result<PerformancePreference> {
    value.parse::<PerformancePreference>()
}

fn parse_measurement_basis_arg(value: &str) -> Result<MeasurementBasis> {
    value.parse::<MeasurementBasis>()
}

fn parse_guardrail_arg(value: &str) -> Result<sched_claw::metrics::Guardrail> {
    parse_guardrail(value)
}

fn parse_metric_target_arg(value: &str) -> Result<MetricTarget> {
    parse_metric_target(value)
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
    use super::{
        Cli, Command, DaemonCommand, ExperimentCommand, ExperimentInitArgs, OutputStyle,
        RunFailureMode, build_workload_target, ensure_candidate_ready_for_rollout,
        resolve_performance_policy,
    };
    use clap::Parser;
    use sched_claw::experiment::{
        CandidateBuildRecord, CandidateRecord, CandidateSpec, CommandStatus, StepCommandRecord,
        VerifierBackend, VerifierCommandRecord,
    };
    use sched_claw::metrics::{MeasurementBasis, MetricGoal, MetricTarget, PerformancePreference};
    use std::collections::BTreeMap;

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
    fn parses_doctor_command() {
        let cli = Cli::try_parse_from(["sched-claw", "doctor", "--style", "plain"]).unwrap();

        match cli.command {
            Some(Command::Doctor(args)) => {
                assert_eq!(args.output.style, OutputStyle::Plain);
            }
            other => panic!("expected doctor command, got {other:?}"),
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

    #[test]
    fn parses_materialize_loader_arg_that_starts_with_dash() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "materialize",
            "demo",
            "--candidate-id",
            "cand-a",
            "--template",
            "latency_guard",
            "--loader",
            "/tmp/mock-loader",
            "--loader-arg",
            "--sched-ext",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(_)) => {}
            other => panic!("expected experiment command, got {other:?}"),
        }
    }

    #[test]
    fn parses_experiment_build_command() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "build",
            "demo",
            "--candidate-id",
            "cand-a",
            "--bpftool",
            "/tmp/bpftool",
            "--skip-verifier",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(args)) => match args.command {
                ExperimentCommand::Build(args) => {
                    assert_eq!(args.candidate_id, "cand-a");
                    assert!(args.skip_verifier);
                    assert_eq!(args.bpftool, "/tmp/bpftool");
                }
                other => panic!("expected build command, got {other:?}"),
            },
            other => panic!("expected experiment command, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_evaluation_policy_command() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "set-evaluation-policy",
            "demo",
            "--min-baseline-runs",
            "3",
            "--min-candidate-runs",
            "4",
            "--min-primary-improvement-pct",
            "2.5",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(args)) => match args.command {
                ExperimentCommand::SetEvaluationPolicy(args) => {
                    assert_eq!(args.min_baseline_runs, Some(3));
                    assert_eq!(args.min_candidate_runs, Some(4));
                    assert_eq!(args.min_primary_improvement_pct, Some(2.5));
                }
                other => panic!("expected set-evaluation-policy command, got {other:?}"),
            },
            other => panic!("expected experiment command, got {other:?}"),
        }
    }

    #[test]
    fn parses_experiment_run_command() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "run",
            "demo",
            "--candidate-id",
            "cand-a",
            "--allow-unverified-build",
            "--lease-seconds",
            "30",
            "--on-failure",
            "strict",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(args)) => match args.command {
                ExperimentCommand::Run(args) => {
                    assert_eq!(args.candidate_id.as_deref(), Some("cand-a"));
                    assert!(args.allow_unverified_build);
                    assert_eq!(args.lease_seconds, Some(30));
                    assert_eq!(args.on_failure, RunFailureMode::Strict);
                }
                other => panic!("expected run command, got {other:?}"),
            },
            other => panic!("expected experiment command, got {other:?}"),
        }
    }

    #[test]
    fn parses_template_show_command() {
        let cli = Cli::try_parse_from(["sched-claw", "template", "show", "latency_guard"]).unwrap();

        match cli.command {
            Some(Command::Template(_)) => {}
            other => panic!("expected template command, got {other:?}"),
        }
    }

    #[test]
    fn parses_daemon_activate_lease() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "daemon",
            "activate",
            "--lease-seconds",
            "30",
            "/tmp/loader",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Daemon(args)) => match args.command {
                DaemonCommand::Activate(args) => {
                    assert_eq!(args.lease_seconds, Some(30));
                }
                other => panic!("expected activate command, got {other:?}"),
            },
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn parses_experiment_init_pid_target_and_proxy_metrics() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "experiment",
            "init",
            "--id",
            "demo",
            "--workload-name",
            "bench",
            "--target-pid",
            "4242",
            "--primary-metric",
            "ipc",
            "--primary-goal",
            "maximize",
            "--performance-basis",
            "proxy_estimate",
            "--proxy-metric",
            "ipc:maximize",
            "--proxy-metric",
            "cpi:minimize",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Experiment(args)) => match args.command {
                ExperimentCommand::Init(args) => {
                    assert_eq!(args.target_pid, Some(4242));
                    assert_eq!(args.proxy_metrics.len(), 2);
                }
                other => panic!("expected init command, got {other:?}"),
            },
            other => panic!("expected experiment command, got {other:?}"),
        }
    }

    #[test]
    fn workload_target_rejects_selector_and_script_mix() {
        let args = ExperimentInitArgs {
            id: "demo".to_string(),
            workload_name: "bench".to_string(),
            workload_description: None,
            target_pid: Some(1),
            target_uid: None,
            target_gid: None,
            target_cgroup: None,
            workload_cwd: None,
            workload_args: vec!["./run.sh".to_string()],
            workload_env: Vec::new(),
            workload_scope: None,
            workload_phase: None,
            success_criteria: None,
            primary_metric: "latency_ms".to_string(),
            primary_goal: MetricGoal::Minimize,
            primary_unit: None,
            performance_preference: None,
            performance_basis: None,
            proxy_metrics: Vec::new(),
            performance_notes: None,
            guardrails: Vec::new(),
            min_baseline_runs: 1,
            min_candidate_runs: 1,
            min_primary_improvement_pct: None,
            max_primary_relative_spread_pct: None,
        };
        assert!(build_workload_target(&args).is_err());
    }

    #[test]
    fn resolves_proxy_performance_policy() {
        let policy = resolve_performance_policy(
            Some(PerformancePreference::Custom),
            Some(MeasurementBasis::ProxyEstimate),
            Some("no direct throughput or latency metric".to_string()),
            &MetricTarget {
                name: "ipc".to_string(),
                goal: MetricGoal::Maximize,
                unit: None,
                notes: None,
            },
            &[],
            vec![
                MetricTarget {
                    name: "ipc".to_string(),
                    goal: MetricGoal::Maximize,
                    unit: None,
                    notes: None,
                },
                MetricTarget {
                    name: "cpi".to_string(),
                    goal: MetricGoal::Minimize,
                    unit: None,
                    notes: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(policy.basis, MeasurementBasis::ProxyEstimate);
        assert_eq!(policy.proxy_metrics.len(), 2);
    }

    #[test]
    fn rollout_gate_rejects_missing_verified_build() {
        let candidate = CandidateRecord {
            spec: CandidateSpec {
                candidate_id: "cand-a".to_string(),
                template: "latency_guard".to_string(),
                source_path: None,
                object_path: None,
                build_command: None,
                daemon_argv: Vec::new(),
                daemon_cwd: None,
                daemon_env: BTreeMap::new(),
                knobs: BTreeMap::new(),
                notes: None,
            },
            runs: Vec::new(),
            builds: vec![CandidateBuildRecord {
                requested_at_unix_ms: 1,
                artifact_dir: "artifacts/builds/cand-a/1".to_string(),
                source_path: None,
                object_path: None,
                build: StepCommandRecord {
                    status: CommandStatus::Success,
                    command: "clang".to_string(),
                    command_path: "build.command.txt".to_string(),
                    exit_code: Some(0),
                    duration_ms: 1,
                    stdout_path: "build.stdout.log".to_string(),
                    stderr_path: "build.stderr.log".to_string(),
                    summary: None,
                },
                verifier: VerifierCommandRecord {
                    backend: VerifierBackend::BpftoolProgLoadall,
                    status: CommandStatus::Failed,
                    command: "bpftool".to_string(),
                    command_path: "verify.command.txt".to_string(),
                    exit_code: Some(1),
                    duration_ms: 1,
                    stdout_path: "verify.stdout.log".to_string(),
                    stderr_path: "verify.stderr.log".to_string(),
                    summary: None,
                },
            }],
        };
        assert!(ensure_candidate_ready_for_rollout(&candidate, false).is_err());
        assert!(ensure_candidate_ready_for_rollout(&candidate, true).is_ok());
    }
}
