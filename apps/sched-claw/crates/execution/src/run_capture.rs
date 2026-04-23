use crate::daemon_protocol::DaemonLogsSnapshot;
use crate::experiment::{
    CommandStatus, PerfStatPolicy, PerfStatProfile, RecordedRun, SchedulerKind, experiments_dir,
    now_unix_ms,
};
use crate::metrics::MetricMap;
use crate::workload::{WorkloadContract, WorkloadTarget};
use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

#[derive(Clone, Debug)]
pub struct WorkloadRunOptions {
    pub label: String,
    pub scheduler: SchedulerKind,
    pub candidate_id: Option<String>,
    pub artifact_dir: Option<String>,
    pub metrics_file_name: String,
    pub timeout_seconds: Option<u64>,
    pub extra_env: BTreeMap<String, String>,
    pub perf_stat: Option<PerfStatRunOptions>,
}

#[derive(Clone, Debug)]
pub struct WorkloadRunCapture {
    pub run: RecordedRun,
    pub manifest_artifact_dir: String,
    pub command_path: String,
    pub stdout_path: String,
    pub stderr_path: String,
    pub metrics_path: String,
    pub perf_stat: Option<PerfStatCapture>,
    pub daemon_logs_path: Option<String>,
    pub summary: String,
}

#[derive(Clone, Debug)]
pub struct PerfStatRunOptions {
    pub perf_bin: String,
    pub policy: PerfStatPolicy,
}

#[derive(Clone, Debug)]
pub struct PerfStatCapture {
    pub artifact_path: String,
    pub collector: String,
    pub metrics: MetricMap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum RunFailureMode {
    Record,
    Strict,
}

pub async fn capture_workload_run(
    workspace_root: &Path,
    experiment_id: &str,
    workload: &WorkloadContract,
    options: &WorkloadRunOptions,
) -> Result<WorkloadRunCapture> {
    let target = workload.effective_target();
    let requested_at_unix_ms = now_unix_ms();
    let artifact_dir_abs = options
        .artifact_dir
        .as_deref()
        .map(|value| resolve_path(workspace_root, value))
        .unwrap_or_else(|| {
            default_run_artifact_dir(
                workspace_root,
                experiment_id,
                options.scheduler,
                options.candidate_id.as_deref(),
                &options.label,
                requested_at_unix_ms,
            )
        });
    std::fs::create_dir_all(&artifact_dir_abs).with_context(|| {
        format!(
            "failed to create run artifact dir {}",
            artifact_dir_abs.display()
        )
    })?;

    let run_cwd = match &target {
        WorkloadTarget::Script { cwd, .. } => cwd
            .as_deref()
            .map(|value| resolve_path(workspace_root, value))
            .unwrap_or_else(|| workspace_root.to_path_buf()),
        _ => workspace_root.to_path_buf(),
    };
    std::fs::create_dir_all(&run_cwd)
        .with_context(|| format!("failed to prepare workload cwd {}", run_cwd.display()))?;

    let command_path_abs = artifact_dir_abs.join("workload.command.txt");
    let stdout_path_abs = artifact_dir_abs.join("workload.stdout.log");
    let stderr_path_abs = artifact_dir_abs.join("workload.stderr.log");
    let metrics_path_abs = artifact_dir_abs.join(&options.metrics_file_name);
    let perf_stat_path_abs = artifact_dir_abs.join("perf.stat.csv");
    let mut run_env = match &target {
        WorkloadTarget::Script { env, .. } => env.clone(),
        _ => BTreeMap::new(),
    };
    run_env.extend(options.extra_env.clone());
    run_env.insert(
        "SCHED_CLAW_EXPERIMENT_ID".to_string(),
        experiment_id.to_string(),
    );
    run_env.insert("SCHED_CLAW_RUN_LABEL".to_string(), options.label.clone());
    run_env.insert(
        "SCHED_CLAW_SCHEDULER_KIND".to_string(),
        options.scheduler.as_str().to_string(),
    );
    run_env.insert(
        "SCHED_CLAW_ARTIFACT_DIR".to_string(),
        artifact_dir_abs.display().to_string(),
    );
    run_env.insert(
        "SCHED_CLAW_METRICS_FILE".to_string(),
        metrics_path_abs.display().to_string(),
    );
    if let Some(candidate_id) = &options.candidate_id {
        run_env.insert("SCHED_CLAW_CANDIDATE_ID".to_string(), candidate_id.clone());
    }

    let argv = build_run_command(
        workspace_root,
        &target,
        options,
        &artifact_dir_abs,
        &perf_stat_path_abs,
    )?;
    write_command_capture(&command_path_abs, &run_cwd, &run_env, &argv)?;

    let start = Instant::now();
    let stdout = File::create(&stdout_path_abs)
        .with_context(|| format!("failed to create {}", stdout_path_abs.display()))?;
    let stderr = File::create(&stderr_path_abs)
        .with_context(|| format!("failed to create {}", stderr_path_abs.display()))?;
    let mut child = Command::new(&argv[0]);
    child
        .args(argv.iter().skip(1))
        .current_dir(&run_cwd)
        .envs(&run_env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let mut child = child
        .spawn()
        .with_context(|| format!("failed to spawn workload command {}", argv[0]))?;
    let output_status = if let Some(timeout_seconds) = options.timeout_seconds {
        match timeout(Duration::from_secs(timeout_seconds.max(1)), child.wait()).await {
            Ok(result) => result.context("failed while waiting for workload process")?,
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                append_log_line(
                    &stderr_path_abs,
                    &format!(
                        "sched-claw: workload timed out after {} second(s)",
                        timeout_seconds.max(1)
                    ),
                )?;
                return finalize_capture(
                    workspace_root,
                    &artifact_dir_abs,
                    requested_at_unix_ms,
                    options,
                    CommandStatus::Failed,
                    None,
                    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    &command_path_abs,
                    &stdout_path_abs,
                    &stderr_path_abs,
                    &metrics_path_abs,
                    &perf_stat_path_abs,
                    perf_stat_collector(options),
                    None,
                );
            }
        }
    } else {
        child
            .wait()
            .await
            .context("failed while waiting for workload process")?
    };
    let duration_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let status_kind = if output_status.success() {
        CommandStatus::Success
    } else {
        CommandStatus::Failed
    };
    finalize_capture(
        workspace_root,
        &artifact_dir_abs,
        requested_at_unix_ms,
        options,
        status_kind,
        output_status.code(),
        duration_ms,
        &command_path_abs,
        &stdout_path_abs,
        &stderr_path_abs,
        &metrics_path_abs,
        &perf_stat_path_abs,
        perf_stat_collector(options),
        None,
    )
}

pub fn attach_daemon_logs(
    workspace_root: &Path,
    capture: &mut WorkloadRunCapture,
    snapshot: &DaemonLogsSnapshot,
) -> Result<()> {
    let artifact_dir_abs = resolve_path(workspace_root, &capture.manifest_artifact_dir);
    let daemon_logs_path_abs = artifact_dir_abs.join("daemon.logs.txt");
    let mut file = File::create(&daemon_logs_path_abs)
        .with_context(|| format!("failed to create {}", daemon_logs_path_abs.display()))?;
    writeln!(
        file,
        "# active_label={}",
        snapshot.active_label.as_deref().unwrap_or("<none>")
    )?;
    writeln!(
        file,
        "# truncated={}",
        if snapshot.truncated { "yes" } else { "no" }
    )?;
    for line in &snapshot.lines {
        writeln!(
            file,
            "[{}] {} {}",
            line.emitted_at_unix_ms, line.source, line.line
        )?;
    }
    let relative = relative_path(workspace_root, &daemon_logs_path_abs);
    capture.daemon_logs_path = Some(relative.clone());
    capture.summary = append_detail(&capture.summary, &format!("daemon_logs={relative}"));
    capture.run.notes = Some(capture.summary.clone());
    Ok(())
}

fn finalize_capture(
    workspace_root: &Path,
    artifact_dir_abs: &Path,
    requested_at_unix_ms: u64,
    options: &WorkloadRunOptions,
    status: CommandStatus,
    exit_code: Option<i32>,
    duration_ms: u64,
    command_path_abs: &Path,
    stdout_path_abs: &Path,
    stderr_path_abs: &Path,
    metrics_path_abs: &Path,
    perf_stat_path_abs: &Path,
    perf_stat_collector: Option<String>,
    daemon_logs_path: Option<String>,
) -> Result<WorkloadRunCapture> {
    let mut metrics = parse_metrics_file(metrics_path_abs)?;
    let perf_stat = capture_perf_stat_artifact(
        workspace_root,
        perf_stat_path_abs,
        perf_stat_collector,
        &mut metrics,
    )?;
    let mut summary = summarize_run(status, exit_code, duration_ms, &metrics, stderr_path_abs)?;
    if let Some(perf_stat) = &perf_stat {
        summary = append_detail(
            &summary,
            &format!(
                "perf_stat={} perf_metrics={}",
                perf_stat.artifact_path,
                perf_stat.metrics.len()
            ),
        );
    }
    let manifest_artifact_dir = relative_path(workspace_root, artifact_dir_abs);
    let metrics_path = relative_path(workspace_root, metrics_path_abs);
    Ok(WorkloadRunCapture {
        run: RecordedRun {
            label: options.label.clone(),
            recorded_at_unix_ms: requested_at_unix_ms,
            scheduler: options.scheduler,
            artifact_dir: manifest_artifact_dir.clone(),
            metrics,
            notes: Some(summary.clone()),
        },
        manifest_artifact_dir,
        command_path: relative_path(workspace_root, command_path_abs),
        stdout_path: relative_path(workspace_root, stdout_path_abs),
        stderr_path: relative_path(workspace_root, stderr_path_abs),
        metrics_path,
        perf_stat,
        daemon_logs_path,
        summary,
    })
}

fn default_run_artifact_dir(
    workspace_root: &Path,
    experiment_id: &str,
    scheduler: SchedulerKind,
    candidate_id: Option<&str>,
    label: &str,
    requested_at_unix_ms: u64,
) -> PathBuf {
    let base = experiments_dir(workspace_root)
        .join(experiment_id)
        .join("artifacts")
        .join("runs");
    match scheduler {
        SchedulerKind::Cfs => base.join("baseline"),
        SchedulerKind::SchedExt => base
            .join("candidates")
            .join(candidate_id.unwrap_or("unknown-candidate")),
    }
    .join(format!("{requested_at_unix_ms}-{label}"))
}

fn rewrite_artifact_dir_arg(argv: Vec<String>, artifact_dir_abs: &Path) -> Vec<String> {
    let mut rewritten = Vec::with_capacity(argv.len());
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--artifact-dir" {
            rewritten.push(arg);
            rewritten.push(artifact_dir_abs.display().to_string());
            let _ = iter.next();
            continue;
        }
        if let Some((flag, _)) = arg.split_once('=')
            && flag == "--artifact-dir"
        {
            rewritten.push(format!("--artifact-dir={}", artifact_dir_abs.display()));
            continue;
        }
        rewritten.push(arg);
    }
    rewritten
}

fn build_run_command(
    workspace_root: &Path,
    target: &WorkloadTarget,
    options: &WorkloadRunOptions,
    artifact_dir_abs: &Path,
    perf_stat_path_abs: &Path,
) -> Result<Vec<String>> {
    let perf_stat = options.perf_stat.as_ref();
    match target {
        WorkloadTarget::Script { argv, .. } => {
            if argv.is_empty() {
                bail!("script workload target must define a non-empty argv");
            }
            let rewritten = rewrite_artifact_dir_arg(argv.clone(), artifact_dir_abs);
            Ok(if let Some(perf_stat) = perf_stat {
                build_perf_stat_command(
                    target,
                    workspace_root,
                    Some(rewritten),
                    options,
                    perf_stat,
                    perf_stat_path_abs,
                )?
            } else {
                rewritten
            })
        }
        _ => {
            let Some(perf_stat) = perf_stat else {
                bail!(
                    "experiment run for target {} requires a collection policy such as perf_stat",
                    target.kind_label()
                );
            };
            build_perf_stat_command(
                target,
                workspace_root,
                None,
                options,
                perf_stat,
                perf_stat_path_abs,
            )
        }
    }
}

fn build_perf_stat_command(
    target: &WorkloadTarget,
    workspace_root: &Path,
    command_argv: Option<Vec<String>>,
    options: &WorkloadRunOptions,
    perf_stat: &PerfStatRunOptions,
    perf_stat_path_abs: &Path,
) -> Result<Vec<String>> {
    let events = perf_stat_events(&perf_stat.policy);
    let mut argv = vec![
        perf_stat.perf_bin.clone(),
        "stat".to_string(),
        "-x,".to_string(),
        "--no-big-num".to_string(),
        "-o".to_string(),
        perf_stat_path_abs.display().to_string(),
    ];
    for event in events {
        argv.push("-e".to_string());
        argv.push(event);
    }
    match target {
        WorkloadTarget::Script { .. } => {
            argv.push("--".to_string());
            argv.extend(command_argv.unwrap_or_default());
        }
        WorkloadTarget::Pid { pid } => {
            argv.push("-p".to_string());
            argv.push(pid.to_string());
            argv.push("--timeout".to_string());
            argv.push(required_timeout_ms(options.timeout_seconds)?.to_string());
        }
        WorkloadTarget::Uid { uid } => {
            argv.push("-p".to_string());
            argv.push(join_pids(resolve_target_pids(workspace_root, *uid, true)?));
            argv.push("--timeout".to_string());
            argv.push(required_timeout_ms(options.timeout_seconds)?.to_string());
        }
        WorkloadTarget::Gid { gid } => {
            argv.push("-p".to_string());
            argv.push(join_pids(resolve_target_pids(workspace_root, *gid, false)?));
            argv.push("--timeout".to_string());
            argv.push(required_timeout_ms(options.timeout_seconds)?.to_string());
        }
        WorkloadTarget::Cgroup { path } => {
            argv.push("-a".to_string());
            argv.push("-G".to_string());
            argv.push(path.clone());
            argv.push("--timeout".to_string());
            argv.push(required_timeout_ms(options.timeout_seconds)?.to_string());
        }
    }
    Ok(argv)
}

fn required_timeout_ms(timeout_seconds: Option<u64>) -> Result<u64> {
    let seconds = timeout_seconds
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow::anyhow!("existing workload targets require --timeout-seconds"))?;
    Ok(seconds.saturating_mul(1000))
}

fn perf_stat_collector(options: &WorkloadRunOptions) -> Option<String> {
    let perf = options.perf_stat.as_ref()?;
    let events = perf_stat_events(&perf.policy);
    let mut collector = format!("{} stat -x, --no-big-num", perf.perf_bin);
    if !events.is_empty() {
        collector.push_str(&format!(" -e {}", events.join(",")));
    }
    Some(collector)
}

fn perf_stat_events(policy: &PerfStatPolicy) -> Vec<String> {
    let mut events = match policy.profile {
        PerfStatProfile::ProxyBasic => vec![
            "task-clock",
            "cycles",
            "instructions",
            "context-switches",
            "cpu-migrations",
            "page-faults",
        ],
        PerfStatProfile::SchedulerBasic => vec![
            "task-clock",
            "cycles",
            "instructions",
            "context-switches",
            "cpu-migrations",
            "page-faults",
            "branches",
            "branch-misses",
            "cache-references",
            "cache-misses",
        ],
    }
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    for event in &policy.events {
        if !events.iter().any(|existing| existing == event) {
            events.push(event.clone());
        }
    }
    events
}

fn capture_perf_stat_artifact(
    workspace_root: &Path,
    perf_stat_path_abs: &Path,
    collector: Option<String>,
    metrics: &mut MetricMap,
) -> Result<Option<PerfStatCapture>> {
    if !perf_stat_path_abs.is_file() {
        return Ok(None);
    }
    let perf_metrics = parse_perf_stat_file(perf_stat_path_abs)?;
    for (name, value) in &perf_metrics {
        metrics.entry(name.clone()).or_insert(*value);
    }
    Ok(Some(PerfStatCapture {
        artifact_path: relative_path(workspace_root, perf_stat_path_abs),
        collector: collector.unwrap_or_else(|| "perf stat".to_string()),
        metrics: perf_metrics,
    }))
}

fn write_command_capture(
    path: &Path,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    argv: &[String],
) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    writeln!(file, "# cwd: {}", cwd.display())?;
    for (key, value) in env {
        writeln!(file, "# env: {key}={value}")?;
    }
    for arg in argv {
        write!(file, "{} ", shell_escape(arg))?;
    }
    writeln!(file)?;
    Ok(())
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '-' | '_' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("{value:?}")
    }
}

fn parse_metrics_file(path: &Path) -> Result<MetricMap> {
    let mut metrics = MetricMap::new();
    if !path.is_file() {
        return Ok(metrics);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read metrics file {}", path.display()))?;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        let Ok(parsed) = value.trim().parse::<f64>() else {
            continue;
        };
        if parsed.is_finite() {
            metrics.insert(name.trim().to_string(), parsed);
        }
    }
    Ok(metrics)
}

fn parse_perf_stat_file(path: &Path) -> Result<MetricMap> {
    let mut metrics = MetricMap::new();
    if !path.is_file() {
        return Ok(metrics);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read perf stat output {}", path.display()))?;
    for line in raw.lines() {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        let Some(value) = parse_perf_counter_value(fields[0]) else {
            continue;
        };
        let unit = fields[1];
        let event_name = fields[2];
        if event_name.is_empty() {
            continue;
        }
        metrics.insert(normalize_perf_metric_name(event_name, unit), value);
    }
    if let (Some(instructions), Some(cycles)) = (
        metrics.get("instructions").copied(),
        metrics.get("cycles").copied(),
    ) {
        if cycles > 0.0 {
            metrics
                .entry("ipc".to_string())
                .or_insert(instructions / cycles);
        }
        if instructions > 0.0 {
            metrics
                .entry("cpi".to_string())
                .or_insert(cycles / instructions);
        }
    }
    if let (Some(branches), Some(branch_misses)) = (
        metrics.get("branches").copied(),
        metrics.get("branch_misses").copied(),
    ) {
        if branches > 0.0 {
            metrics
                .entry("branch_miss_rate_pct".to_string())
                .or_insert((branch_misses / branches) * 100.0);
        }
    }
    if let (Some(cache_references), Some(cache_misses)) = (
        metrics.get("cache_references").copied(),
        metrics.get("cache_misses").copied(),
    ) {
        if cache_references > 0.0 {
            metrics
                .entry("cache_miss_rate_pct".to_string())
                .or_insert((cache_misses / cache_references) * 100.0);
        }
    }
    Ok(metrics)
}

fn parse_perf_counter_value(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return None;
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn normalize_perf_metric_name(event_name: &str, unit: &str) -> String {
    let base = sanitize_metric_name(event_name);
    let unit = sanitize_metric_name(unit);
    match (base.as_str(), unit.as_str()) {
        ("task_clock", "msec") => "task_clock_ms".to_string(),
        (_, "") => base,
        _ => format!("{base}_{unit}"),
    }
}

fn sanitize_metric_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn resolve_target_pids(_workspace_root: &Path, selector: u32, is_uid: bool) -> Result<Vec<u32>> {
    let mut pids = Vec::new();
    for entry in std::fs::read_dir("/proc").context("failed to scan /proc for workload pids")? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let status_path = entry.path().join("status");
        let Ok(raw) = std::fs::read_to_string(&status_path) else {
            continue;
        };
        let mut uid_match = None;
        let mut gid_match = None;
        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                uid_match = rest
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u32>().ok());
            }
            if let Some(rest) = line.strip_prefix("Gid:") {
                gid_match = rest
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u32>().ok());
            }
        }
        if is_uid {
            if uid_match == Some(selector) {
                pids.push(pid);
            }
        } else if gid_match == Some(selector) {
            pids.push(pid);
        }
    }
    if pids.is_empty() {
        let kind = if is_uid { "uid" } else { "gid" };
        bail!("no running pids found for workload target {kind}={selector}");
    }
    pids.sort_unstable();
    Ok(pids)
}

fn join_pids(pids: Vec<u32>) -> String {
    pids.into_iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn summarize_run(
    status: CommandStatus,
    exit_code: Option<i32>,
    duration_ms: u64,
    metrics: &MetricMap,
    stderr_path: &Path,
) -> Result<String> {
    let mut summary = format!(
        "status={} exit={} duration_ms={} metrics={}",
        status.as_str(),
        exit_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        duration_ms,
        metrics.len()
    );
    if let Some(line) = summarize_stderr(stderr_path)? {
        summary = append_detail(&summary, &format!("stderr={line}"));
    }
    Ok(summary)
}

fn summarize_stderr(path: &Path) -> Result<Option<String>> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    for needle in ["Error:", "error:", "failed", "timeout", "not found"] {
        if let Some(line) = raw.lines().find(|line| line.contains(needle)) {
            return Ok(Some(line.trim().to_string()));
        }
    }
    Ok(raw
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string()))
}

fn append_log_line(path: &Path, line: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn append_detail(summary: &str, detail: &str) -> String {
    format!("{summary} {detail}")
}

fn resolve_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

fn relative_path(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        PerfStatRunOptions, WorkloadRunOptions, capture_workload_run, parse_perf_stat_file,
    };
    use crate::experiment::{PerfStatPolicy, PerfStatProfile, SchedulerKind};
    use crate::workload::{WorkloadContract, WorkloadTarget};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[tokio::test]
    async fn captures_script_workload_metrics() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let script = workspace.join("run.sh");
        std::fs::write(
            &script,
            "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p \"$SCHED_CLAW_ARTIFACT_DIR\"\nprintf 'latency_ms=7\\nthroughput=12\\n' >\"$SCHED_CLAW_METRICS_FILE\"\nprintf 'ok\\n'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let capture = capture_workload_run(
            workspace,
            "demo",
            &WorkloadContract {
                name: "bench".to_string(),
                target: Some(WorkloadTarget::Script {
                    cwd: Some(workspace.display().to_string()),
                    argv: vec![script.display().to_string()],
                    env: BTreeMap::new(),
                }),
                ..Default::default()
            },
            &WorkloadRunOptions {
                label: "baseline-a".to_string(),
                scheduler: SchedulerKind::Cfs,
                candidate_id: None,
                artifact_dir: None,
                metrics_file_name: "metrics.env".to_string(),
                timeout_seconds: None,
                extra_env: BTreeMap::new(),
                perf_stat: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(capture.run.metrics.get("latency_ms"), Some(&7.0));
        assert_eq!(capture.run.metrics.get("throughput"), Some(&12.0));
        assert!(workspace.join(&capture.command_path).is_file());
    }

    #[tokio::test]
    async fn rewrites_artifact_dir_argument_for_script_targets() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let script = workspace.join("run.sh");
        std::fs::write(
            &script,
            "#!/usr/bin/env bash\nset -euo pipefail\nif [ \"$1\" != \"--artifact-dir\" ]; then\n  exit 3\nfi\nmkdir -p \"$2\"\nprintf 'ipc=1\\n' >\"$2/metrics.env\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let capture = capture_workload_run(
            workspace,
            "demo",
            &WorkloadContract {
                name: "bench".to_string(),
                target: Some(WorkloadTarget::Script {
                    cwd: Some(workspace.display().to_string()),
                    argv: vec![
                        script.display().to_string(),
                        "--artifact-dir".to_string(),
                        "/tmp/old".to_string(),
                    ],
                    env: BTreeMap::new(),
                }),
                ..Default::default()
            },
            &WorkloadRunOptions {
                label: "baseline-b".to_string(),
                scheduler: SchedulerKind::Cfs,
                candidate_id: None,
                artifact_dir: None,
                metrics_file_name: "metrics.env".to_string(),
                timeout_seconds: None,
                extra_env: BTreeMap::new(),
                perf_stat: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(capture.run.metrics.get("ipc"), Some(&1.0));
        assert!(capture.manifest_artifact_dir.contains("baseline-b"));
    }

    #[test]
    fn parses_perf_stat_csv_and_derives_proxy_metrics() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("perf.stat.csv");
        std::fs::write(
            &path,
            "1000,,cycles,0,100.00,,\n2000,,instructions,0,100.00,,\n100,,branches,0,100.00,,\n5,,branch-misses,0,100.00,,\n",
        )
        .unwrap();
        let metrics = parse_perf_stat_file(&path).unwrap();
        assert_eq!(metrics.get("cycles"), Some(&1000.0));
        assert_eq!(metrics.get("instructions"), Some(&2000.0));
        assert_eq!(metrics.get("ipc"), Some(&2.0));
        assert_eq!(metrics.get("cpi"), Some(&0.5));
        assert_eq!(metrics.get("branch_miss_rate_pct"), Some(&5.0));
    }

    #[tokio::test]
    async fn captures_script_workload_with_fake_perf_stat() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let perf = workspace.join("perf");
        let script = workspace.join("run.sh");
        std::fs::write(
            &perf,
            "#!/usr/bin/env bash\nset -euo pipefail\nout=\nargs=()\nwhile [ \"$#\" -gt 0 ]; do\n  case \"$1\" in\n    stat|--no-big-num|-x,)\n      shift\n      ;;\n    -o)\n      out=\"$2\"\n      shift 2\n      ;;\n    -e)\n      shift 2\n      ;;\n    --)\n      shift\n      break\n      ;;\n    *)\n      args+=(\"$1\")\n      shift\n      ;;\n  esac\ndone\nmkdir -p \"$(dirname \"$out\")\"\nprintf '1000,,cycles,0,100.00,,\\n2000,,instructions,0,100.00,,\\n' >\"$out\"\n\"$@\"\n",
        )
        .unwrap();
        std::fs::write(
            &script,
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'latency_ms=7\\n' >\"$SCHED_CLAW_METRICS_FILE\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&perf, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let capture = capture_workload_run(
            workspace,
            "demo",
            &WorkloadContract {
                name: "bench".to_string(),
                target: Some(WorkloadTarget::Script {
                    cwd: Some(workspace.display().to_string()),
                    argv: vec![script.display().to_string()],
                    env: BTreeMap::new(),
                }),
                ..Default::default()
            },
            &WorkloadRunOptions {
                label: "baseline-perf".to_string(),
                scheduler: SchedulerKind::Cfs,
                candidate_id: None,
                artifact_dir: None,
                metrics_file_name: "metrics.env".to_string(),
                timeout_seconds: None,
                extra_env: BTreeMap::new(),
                perf_stat: Some(PerfStatRunOptions {
                    perf_bin: perf.display().to_string(),
                    policy: PerfStatPolicy {
                        profile: PerfStatProfile::ProxyBasic,
                        events: Vec::new(),
                        notes: None,
                    },
                }),
            },
        )
        .await
        .unwrap();
        assert_eq!(capture.run.metrics.get("latency_ms"), Some(&7.0));
        assert_eq!(capture.run.metrics.get("ipc"), Some(&2.0));
        assert!(capture.perf_stat.is_some());
        assert!(
            capture
                .perf_stat
                .as_ref()
                .unwrap()
                .artifact_path
                .ends_with("perf.stat.csv")
        );
    }
}
