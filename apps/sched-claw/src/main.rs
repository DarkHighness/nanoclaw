use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sched_claw::app_config::{CliOverrides, SchedClawConfig};
use sched_claw::bootstrap::load_bootstrap;
use sched_claw::daemon_client::SchedClawDaemonClient;
use sched_claw::daemon_protocol::{
    DaemonCapabilityInvocation, PerfCallGraphMode, PerfCollectionMode, PerfTargetSelector,
    SchedClawDaemonRequest, SchedClawDaemonResponse,
};
use sched_claw::display::{
    OutputStyle, render_daemon_response, render_doctor_report, render_session_detail,
    render_session_export_artifact, render_session_list, render_session_search_results,
    render_skill_detail, render_skill_list, render_tool_detail, render_tool_list,
};
use sched_claw::doctor::collect_doctor_report;
use sched_claw::history::SessionHistory;
use sched_claw::repl::{run_exec, run_repl};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    Capabilities(OutputArgs),
    Status(OutputArgs),
    Activate(DaemonActivateArgs),
    CollectPerf(DaemonCollectPerfArgs),
    CollectSched(DaemonCollectSchedArgs),
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

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum PerfModeArg {
    Stat,
    Record,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum PerfCallGraphArg {
    FramePointer,
    Dwarf,
    Lbr,
}

#[derive(Debug, Args)]
struct DaemonCollectPerfArgs {
    #[arg(long, value_name = "TEXT")]
    label: Option<String>,
    #[arg(long, value_enum, default_value_t = PerfModeArg::Stat)]
    mode: PerfModeArg,
    #[arg(long, value_name = "DIR")]
    output_dir: String,
    #[arg(long, value_name = "MS")]
    duration_ms: u64,
    #[arg(long = "event", value_name = "EVENT")]
    events: Vec<String>,
    #[arg(long, value_name = "PID", value_delimiter = ',')]
    pid: Vec<u32>,
    #[arg(long, value_name = "UID")]
    uid: Option<u32>,
    #[arg(long, value_name = "GID")]
    gid: Option<u32>,
    #[arg(long, value_name = "PATH")]
    cgroup: Option<String>,
    #[arg(long, value_name = "HZ")]
    sample_frequency_hz: Option<u32>,
    #[arg(long, value_enum)]
    call_graph: Option<PerfCallGraphArg>,
    #[arg(long)]
    overwrite: bool,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Debug, Args)]
struct DaemonCollectSchedArgs {
    #[arg(long, value_name = "TEXT")]
    label: Option<String>,
    #[arg(long, value_name = "DIR")]
    output_dir: String,
    #[arg(long, value_name = "MS")]
    duration_ms: u64,
    #[arg(long, value_name = "PID", value_delimiter = ',')]
    pid: Vec<u32>,
    #[arg(long, value_name = "UID")]
    uid: Option<u32>,
    #[arg(long, value_name = "GID")]
    gid: Option<u32>,
    #[arg(long, value_name = "PATH")]
    cgroup: Option<String>,
    #[arg(long)]
    latency_by_pid: bool,
    #[arg(long)]
    overwrite: bool,
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

async fn run_daemon_command(
    workspace_root: &Path,
    overrides: &CliOverrides,
    args: DaemonArgs,
) -> Result<()> {
    let client = SchedClawDaemonClient::new(
        SchedClawConfig::load_from_dir(workspace_root, overrides)?.daemon,
    );
    let (request, style) = match args.command {
        DaemonCommand::Capabilities(output) => {
            (SchedClawDaemonRequest::Capabilities {}, output.style)
        }
        DaemonCommand::Status(output) => (SchedClawDaemonRequest::Status {}, output.style),
        DaemonCommand::Activate(args) => (
            SchedClawDaemonRequest::Invoke {
                invocation: DaemonCapabilityInvocation::RolloutActivate {
                    label: args.label,
                    argv: args.argv,
                    cwd: args.cwd,
                    env: args.env.into_iter().collect::<BTreeMap<_, _>>(),
                    lease_timeout_ms: lease_seconds_to_ms(args.lease_seconds),
                    replace_existing: args.replace_existing,
                },
            },
            args.output.style,
        ),
        DaemonCommand::CollectPerf(args) => {
            let selector = parse_perf_selector(
                !args.pid.is_empty(),
                args.pid.clone(),
                args.uid,
                args.gid,
                args.cgroup.clone(),
            )?;
            (
                SchedClawDaemonRequest::Invoke {
                    invocation: DaemonCapabilityInvocation::PerfCapture {
                        label: args.label,
                        mode: map_perf_mode(args.mode),
                        selector,
                        output_dir: args.output_dir,
                        duration_ms: args.duration_ms,
                        events: args.events,
                        sample_frequency_hz: args.sample_frequency_hz,
                        call_graph: args.call_graph.map(map_perf_call_graph),
                        overwrite: args.overwrite,
                    },
                },
                args.output.style,
            )
        }
        DaemonCommand::CollectSched(args) => {
            let selector = parse_perf_selector(
                !args.pid.is_empty(),
                args.pid.clone(),
                args.uid,
                args.gid,
                args.cgroup.clone(),
            )?;
            (
                SchedClawDaemonRequest::Invoke {
                    invocation: DaemonCapabilityInvocation::SchedulerTraceCapture {
                        label: args.label,
                        selector,
                        output_dir: args.output_dir,
                        duration_ms: args.duration_ms,
                        latency_by_pid: args.latency_by_pid,
                        overwrite: args.overwrite,
                    },
                },
                args.output.style,
            )
        }
        DaemonCommand::Logs(args) => (
            SchedClawDaemonRequest::Logs {
                tail_lines: args.tail_lines,
            },
            args.output.style,
        ),
        DaemonCommand::Stop(args) => (
            SchedClawDaemonRequest::Invoke {
                invocation: DaemonCapabilityInvocation::RolloutStop {
                    graceful_timeout_ms: args.graceful_timeout_ms,
                },
            },
            args.output.style,
        ),
    };
    let response = client.send(&request).await?;
    match response {
        SchedClawDaemonResponse::Error { message } => anyhow::bail!(message),
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

fn lease_seconds_to_ms(seconds: Option<u64>) -> Option<u64> {
    seconds.map(|value| value.max(1).saturating_mul(1_000))
}

fn map_perf_mode(value: PerfModeArg) -> PerfCollectionMode {
    match value {
        PerfModeArg::Stat => PerfCollectionMode::Stat,
        PerfModeArg::Record => PerfCollectionMode::Record,
    }
}

fn map_perf_call_graph(value: PerfCallGraphArg) -> PerfCallGraphMode {
    match value {
        PerfCallGraphArg::FramePointer => PerfCallGraphMode::FramePointer,
        PerfCallGraphArg::Dwarf => PerfCallGraphMode::Dwarf,
        PerfCallGraphArg::Lbr => PerfCallGraphMode::Lbr,
    }
}

fn parse_perf_selector(
    has_pids: bool,
    pids: Vec<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    cgroup: Option<String>,
) -> Result<PerfTargetSelector> {
    let mut seen = 0usize;
    if has_pids {
        seen += 1;
    }
    if uid.is_some() {
        seen += 1;
    }
    if gid.is_some() {
        seen += 1;
    }
    if cgroup.is_some() {
        seen += 1;
    }
    if seen != 1 {
        anyhow::bail!("exactly one of --pid, --uid, --gid, or --cgroup is required");
    }
    if has_pids {
        return Ok(PerfTargetSelector::Pid { pids });
    }
    if let Some(uid) = uid {
        return Ok(PerfTargetSelector::Uid { uid });
    }
    if let Some(gid) = gid {
        return Ok(PerfTargetSelector::Gid { gid });
    }
    Ok(PerfTargetSelector::Cgroup {
        path: cgroup.expect("cgroup selector is present when no other selector matches"),
    })
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
    use super::{Cli, Command, DaemonCommand};
    use clap::Parser;

    #[test]
    fn parses_sessions_query_with_style_after_query() {
        let cli =
            Cli::try_parse_from(["sched-claw", "sessions", "daemon logs", "--style", "plain"])
                .unwrap();
        match cli.command {
            Some(Command::Sessions(args)) => {
                assert_eq!(args.query.as_deref(), Some("daemon logs"));
                assert_eq!(args.output.style.as_str(), "plain");
            }
            other => panic!("expected sessions command, got {other:?}"),
        }
    }

    #[test]
    fn parses_doctor_command() {
        let cli = Cli::try_parse_from(["sched-claw", "doctor", "--style", "plain"]).unwrap();
        match cli.command {
            Some(Command::Doctor(args)) => assert_eq!(args.output.style.as_str(), "plain"),
            other => panic!("expected doctor command, got {other:?}"),
        }
    }

    #[test]
    fn parses_daemon_activate_lease() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "daemon",
            "activate",
            "--lease-seconds",
            "7",
            "--env",
            "A=B",
            "loader",
            "--flag",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Daemon(args)) => match args.command {
                DaemonCommand::Activate(args) => {
                    assert_eq!(args.lease_seconds, Some(7));
                    assert_eq!(args.env, vec![("A".to_string(), "B".to_string())]);
                    assert_eq!(args.argv, vec!["loader".to_string(), "--flag".to_string()]);
                }
                other => panic!("expected activate command, got {other:?}"),
            },
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn parses_daemon_collect_perf_pid_target() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "daemon",
            "collect-perf",
            "--mode",
            "record",
            "--output-dir",
            "artifacts/perf-a",
            "--duration-ms",
            "750",
            "--pid",
            "123,456",
            "--event",
            "cycles",
            "--call-graph",
            "dwarf",
            "--style",
            "plain",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Daemon(args)) => match args.command {
                DaemonCommand::CollectPerf(args) => {
                    assert_eq!(args.pid, vec![123, 456]);
                    assert_eq!(args.events, vec!["cycles"]);
                    assert_eq!(args.output.style.as_str(), "plain");
                }
                other => panic!("expected collect-perf command, got {other:?}"),
            },
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn parses_daemon_collect_sched_cgroup_target() {
        let cli = Cli::try_parse_from([
            "sched-claw",
            "daemon",
            "collect-sched",
            "--output-dir",
            "artifacts/sched-a",
            "--duration-ms",
            "900",
            "--cgroup",
            "work.slice",
            "--latency-by-pid",
            "--style",
            "plain",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Daemon(args)) => match args.command {
                DaemonCommand::CollectSched(args) => {
                    assert_eq!(args.cgroup.as_deref(), Some("work.slice"));
                    assert!(args.latency_by_pid);
                    assert_eq!(args.output.style.as_str(), "plain");
                }
                other => panic!("expected collect-sched command, got {other:?}"),
            },
            other => panic!("expected daemon command, got {other:?}"),
        }
    }

    #[test]
    fn parses_daemon_capabilities_command() {
        let cli = Cli::try_parse_from(["sched-claw", "daemon", "capabilities", "--style", "plain"])
            .unwrap();
        match cli.command {
            Some(Command::Daemon(args)) => match args.command {
                DaemonCommand::Capabilities(output) => {
                    assert_eq!(output.style.as_str(), "plain");
                }
                other => panic!("expected capabilities command, got {other:?}"),
            },
            other => panic!("expected daemon command, got {other:?}"),
        }
    }
}
