use crate::daemon_protocol::DaemonLogsSnapshot;
use crate::experiment::{CommandStatus, RecordedRun, SchedulerKind, experiments_dir, now_unix_ms};
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
}

#[derive(Clone, Debug)]
pub struct WorkloadRunCapture {
    pub run: RecordedRun,
    pub manifest_artifact_dir: String,
    pub command_path: String,
    pub stdout_path: String,
    pub stderr_path: String,
    pub metrics_path: String,
    pub daemon_logs_path: Option<String>,
    pub summary: String,
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
    let WorkloadTarget::Script { cwd, argv, env } = target else {
        bail!(
            "experiment run currently supports only script workload targets; found {}",
            workload.effective_target().kind_label()
        );
    };
    if argv.is_empty() {
        bail!("script workload target must define a non-empty argv");
    }

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

    let run_cwd = cwd
        .as_deref()
        .map(|value| resolve_path(workspace_root, value))
        .unwrap_or_else(|| workspace_root.to_path_buf());
    std::fs::create_dir_all(&run_cwd)
        .with_context(|| format!("failed to prepare workload cwd {}", run_cwd.display()))?;

    let command_path_abs = artifact_dir_abs.join("workload.command.txt");
    let stdout_path_abs = artifact_dir_abs.join("workload.stdout.log");
    let stderr_path_abs = artifact_dir_abs.join("workload.stderr.log");
    let metrics_path_abs = artifact_dir_abs.join(&options.metrics_file_name);
    let mut run_env = env;
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

    let argv = rewrite_artifact_dir_arg(argv, &artifact_dir_abs);
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
    daemon_logs_path: Option<String>,
) -> Result<WorkloadRunCapture> {
    let metrics = parse_metrics_file(metrics_path_abs)?;
    let summary = summarize_run(status, exit_code, duration_ms, &metrics, stderr_path_abs)?;
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
    use super::{WorkloadRunOptions, capture_workload_run};
    use crate::experiment::SchedulerKind;
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
            },
        )
        .await
        .unwrap();
        assert_eq!(capture.run.metrics.get("ipc"), Some(&1.0));
        assert!(capture.manifest_artifact_dir.contains("baseline-b"));
    }
}
