use crate::daemon_protocol::{
    ActiveDeploymentSnapshot, DEFAULT_LOG_TAIL_LINES, DEFAULT_STOP_TIMEOUT_MS, DaemonLogLine,
    DaemonLogsSnapshot, DaemonStatusSnapshot, DeploymentExitSnapshot, MAX_PERF_DURATION_MS,
    MIN_PERF_DURATION_MS, PerfCallGraphMode, PerfCollectionMode, PerfCollectionSnapshot,
    PerfTargetSelector, SchedCollectionSnapshot, SchedExtDaemonRequest, SchedExtDaemonResponse,
};
use anyhow::{Context, Result, anyhow, bail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Gid, Pid, Uid, chown};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{Duration, interval, timeout};
use tracing::{info, warn};

#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub workspace_root: PathBuf,
    pub socket_path: PathBuf,
    pub allowed_roots: Vec<PathBuf>,
    pub log_capacity: usize,
    pub client_uid: Option<u32>,
    pub client_gid: Option<u32>,
}

#[derive(Clone)]
struct DaemonServer {
    state: Arc<Mutex<DaemonState>>,
    options: Arc<NormalizedServeOptions>,
    collection_lock: Arc<AsyncMutex<()>>,
}

struct NormalizedServeOptions {
    workspace_root: PathBuf,
    socket_path: PathBuf,
    allowed_roots: Vec<PathBuf>,
    log_capacity: usize,
    client_uid: Option<Uid>,
    client_gid: Option<Gid>,
}

struct DaemonState {
    active: Option<ActiveDeployment>,
    last_exit: Option<DeploymentExitSnapshot>,
    last_logs: Vec<DaemonLogLine>,
}

#[derive(Clone)]
struct ActiveDeployment {
    label: String,
    argv: Vec<String>,
    cwd: PathBuf,
    pid: u32,
    started_at_unix_s: u64,
    lease_timeout_ms: Option<u64>,
    lease_expires_at_unix_ms: Option<u64>,
    child: Arc<AsyncMutex<Child>>,
    logs: Arc<Mutex<LogBuffer>>,
}

struct LogBuffer {
    capacity: usize,
    lines: VecDeque<DaemonLogLine>,
}

#[derive(Debug)]
struct LaunchSpec {
    label: String,
    argv: Vec<String>,
    executable: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    lease_timeout_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct PerfCollectionSpec {
    label: String,
    mode: PerfCollectionMode,
    selector: PerfTargetSelector,
    resolved_pids: Vec<u32>,
    output_dir: PathBuf,
    duration_ms: u64,
    events: Vec<String>,
    sample_frequency_hz: Option<u32>,
    call_graph: Option<PerfCallGraphMode>,
    perf_argv: Vec<String>,
}

#[derive(Clone, Debug)]
struct SchedCollectionSpec {
    label: String,
    selector: PerfTargetSelector,
    resolved_pids: Vec<u32>,
    output_dir: PathBuf,
    duration_ms: u64,
    latency_by_pid: bool,
    record_argv: Vec<String>,
    timehist_argv: Vec<String>,
    latency_argv: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum StopReason {
    Requested,
    Replaced,
    Shutdown,
    LeaseExpired,
    Exited,
}

impl StopReason {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested_stop",
            Self::Replaced => "replaced",
            Self::Shutdown => "daemon_shutdown",
            Self::LeaseExpired => "lease_expired",
            Self::Exited => "exited",
        }
    }
}

pub async fn serve(options: ServeOptions) -> Result<()> {
    let server = DaemonServer::new(options)?;
    server.prepare_socket()?;
    let listener = UnixListener::bind(&server.options.socket_path).with_context(|| {
        format!(
            "failed to bind daemon socket {}",
            server.options.socket_path.display()
        )
    })?;
    server.finalize_socket_permissions()?;
    info!(
        socket = %server.options.socket_path.display(),
        workspace_root = %server.options.workspace_root.display(),
        "sched-claw daemon listening"
    );
    let mut maintenance_tick = interval(Duration::from_millis(250));

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let server = server.clone();
                tokio::spawn(async move {
                    if let Err(error) = server.handle_client(stream).await {
                        warn!(error = %error, "failed to handle daemon client");
                    }
                });
            }
            _ = maintenance_tick.tick() => {
                if let Err(error) = server.reap_active_if_exited().await {
                    warn!(error = %error, "failed to reap active deployment");
                }
                if let Err(error) = server.enforce_active_lease().await {
                    warn!(error = %error, "failed to enforce active deployment lease");
                }
            }
            signal = tokio::signal::ctrl_c() => {
                if let Err(error) = signal {
                    warn!(error = %error, "failed to await ctrl-c");
                }
                info!("received shutdown signal");
                break;
            }
        }
    }

    server.shutdown().await?;
    if let Err(error) = std::fs::remove_file(&server.options.socket_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn!(
            error = %error,
            socket = %server.options.socket_path.display(),
            "failed to remove daemon socket during shutdown"
        );
    }
    Ok(())
}

impl DaemonServer {
    fn new(options: ServeOptions) -> Result<Self> {
        let workspace_root = canonicalize_existing(&options.workspace_root)
            .context("failed to resolve workspace root for daemon")?;
        let socket_path = if options.socket_path.is_absolute() {
            options.socket_path
        } else {
            workspace_root.join(options.socket_path)
        };
        let mut allowed_roots = if options.allowed_roots.is_empty() {
            vec![workspace_root.clone()]
        } else {
            options.allowed_roots
        };
        for root in &mut allowed_roots {
            *root = canonicalize_existing(root)
                .with_context(|| format!("failed to resolve allowed root {}", root.display()))?;
        }
        allowed_roots.sort();
        allowed_roots.dedup();

        Ok(Self {
            state: Arc::new(Mutex::new(DaemonState {
                active: None,
                last_exit: None,
                last_logs: Vec::new(),
            })),
            options: Arc::new(NormalizedServeOptions {
                workspace_root,
                socket_path,
                allowed_roots,
                log_capacity: options.log_capacity.max(1),
                client_uid: options.client_uid.map(Uid::from_raw),
                client_gid: options.client_gid.map(Gid::from_raw),
            }),
            collection_lock: Arc::new(AsyncMutex::new(())),
        })
    }

    fn prepare_socket(&self) -> Result<()> {
        if let Some(parent) = self.options.socket_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        if self.options.socket_path.exists() {
            std::fs::remove_file(&self.options.socket_path).with_context(|| {
                format!(
                    "failed to remove stale daemon socket {}",
                    self.options.socket_path.display()
                )
            })?;
        }
        Ok(())
    }

    fn finalize_socket_permissions(&self) -> Result<()> {
        let mut permissions = std::fs::metadata(&self.options.socket_path)?.permissions();
        permissions.set_mode(if self.options.client_uid.is_some() {
            0o660
        } else {
            0o600
        });
        std::fs::set_permissions(&self.options.socket_path, permissions)?;
        if self.options.client_uid.is_some() || self.options.client_gid.is_some() {
            chown(
                &self.options.socket_path,
                self.options.client_uid,
                self.options.client_gid,
            )
            .with_context(|| {
                format!(
                    "failed to chown daemon socket {}",
                    self.options.socket_path.display()
                )
            })?;
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let _ = self
            .stop_active(DEFAULT_STOP_TIMEOUT_MS, StopReason::Shutdown)
            .await?;
        Ok(())
    }

    async fn handle_client(&self, stream: UnixStream) -> Result<()> {
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await?;
        if request_line.trim().is_empty() {
            return Ok(());
        }
        let response = match serde_json::from_str::<SchedExtDaemonRequest>(request_line.trim()) {
            Ok(request) => match self.dispatch(request).await {
                Ok(response) => response,
                Err(error) => SchedExtDaemonResponse::Error {
                    message: error.to_string(),
                },
            },
            Err(error) => SchedExtDaemonResponse::Error {
                message: format!("invalid daemon request: {error}"),
            },
        };
        let payload = serde_json::to_vec(&response)?;
        write_half.write_all(&payload).await?;
        write_half.write_all(b"\n").await?;
        write_half.flush().await?;
        Ok(())
    }

    async fn dispatch(&self, request: SchedExtDaemonRequest) -> Result<SchedExtDaemonResponse> {
        self.reap_active_if_exited().await?;
        match request {
            SchedExtDaemonRequest::Status {} => Ok(SchedExtDaemonResponse::Status {
                snapshot: self.status_snapshot(),
            }),
            SchedExtDaemonRequest::Logs { tail_lines } => Ok(SchedExtDaemonResponse::Logs {
                snapshot: self.logs_snapshot(tail_lines.unwrap_or(DEFAULT_LOG_TAIL_LINES)),
            }),
            SchedExtDaemonRequest::CollectPerf {
                label,
                mode,
                selector,
                output_dir,
                duration_ms,
                events,
                sample_frequency_hz,
                call_graph,
                overwrite,
            } => {
                let spec = self.validate_perf_collection(
                    label,
                    mode,
                    selector,
                    output_dir,
                    duration_ms,
                    events,
                    sample_frequency_hz,
                    call_graph,
                    overwrite,
                )?;
                let snapshot = self.collect_perf(spec).await?;
                Ok(SchedExtDaemonResponse::PerfCollection { snapshot })
            }
            SchedExtDaemonRequest::CollectSched {
                label,
                selector,
                output_dir,
                duration_ms,
                latency_by_pid,
                overwrite,
            } => {
                let spec = self.validate_sched_collection(
                    label,
                    selector,
                    output_dir,
                    duration_ms,
                    latency_by_pid,
                    overwrite,
                )?;
                let snapshot = self.collect_sched(spec).await?;
                Ok(SchedExtDaemonResponse::SchedCollection { snapshot })
            }
            SchedExtDaemonRequest::Activate {
                label,
                argv,
                cwd,
                env,
                lease_timeout_ms,
                replace_existing,
            } => {
                if replace_existing {
                    let _ = self
                        .stop_active(DEFAULT_STOP_TIMEOUT_MS, StopReason::Replaced)
                        .await?;
                } else if self.state.lock().unwrap().active.is_some() {
                    bail!(
                        "a sched-ext deployment is already active; stop it first or set replace_existing=true"
                    );
                }
                let launch = self.validate_launch(label, argv, cwd, env, lease_timeout_ms)?;
                let snapshot = self.start_active(launch).await?;
                Ok(SchedExtDaemonResponse::Ack {
                    message: "activated sched-ext deployment".to_string(),
                    snapshot,
                })
            }
            SchedExtDaemonRequest::Stop {
                graceful_timeout_ms,
            } => {
                let snapshot = self
                    .stop_active(
                        graceful_timeout_ms.unwrap_or(DEFAULT_STOP_TIMEOUT_MS),
                        StopReason::Requested,
                    )
                    .await?;
                Ok(SchedExtDaemonResponse::Ack {
                    message: "stopped active sched-ext deployment".to_string(),
                    snapshot,
                })
            }
        }
    }

    fn validate_perf_collection(
        &self,
        label: Option<String>,
        mode: PerfCollectionMode,
        selector: PerfTargetSelector,
        output_dir: String,
        duration_ms: u64,
        events: Vec<String>,
        sample_frequency_hz: Option<u32>,
        call_graph: Option<PerfCallGraphMode>,
        overwrite: bool,
    ) -> Result<PerfCollectionSpec> {
        if !(MIN_PERF_DURATION_MS..=MAX_PERF_DURATION_MS).contains(&duration_ms) {
            bail!(
                "duration_ms must be between {} and {}",
                MIN_PERF_DURATION_MS,
                MAX_PERF_DURATION_MS
            );
        }
        let resolved_pids = self.resolve_perf_selector(&selector)?;
        if resolved_pids.is_empty() {
            bail!("no live pids resolved for perf selector");
        }

        let output_dir =
            resolve_allow_missing_path(&self.options.workspace_root, Path::new(&output_dir))
                .with_context(|| format!("failed to resolve output dir {output_dir}"))?;
        self.ensure_allowed_path_for_create(&output_dir)?;
        self.prepare_output_dir(&output_dir, overwrite)?;

        for event in &events {
            validate_perf_event(event)?;
        }
        if sample_frequency_hz.is_some() && mode != PerfCollectionMode::Record {
            bail!("sample_frequency_hz is only valid for perf record");
        }
        if call_graph.is_some() && mode != PerfCollectionMode::Record {
            bail!("call_graph is only valid for perf record");
        }
        if sample_frequency_hz == Some(0) {
            bail!("sample_frequency_hz must be greater than zero");
        }
        let perf_argv = build_perf_argv(
            mode,
            &resolved_pids,
            &events,
            sample_frequency_hz,
            call_graph,
            &output_dir,
        );
        let label = label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| match mode {
                PerfCollectionMode::Stat => "perf-stat".to_string(),
                PerfCollectionMode::Record => "perf-record".to_string(),
            });

        Ok(PerfCollectionSpec {
            label,
            mode,
            selector,
            resolved_pids,
            output_dir,
            duration_ms,
            events,
            sample_frequency_hz,
            call_graph,
            perf_argv,
        })
    }

    fn validate_sched_collection(
        &self,
        label: Option<String>,
        selector: PerfTargetSelector,
        output_dir: String,
        duration_ms: u64,
        latency_by_pid: bool,
        overwrite: bool,
    ) -> Result<SchedCollectionSpec> {
        if !(MIN_PERF_DURATION_MS..=MAX_PERF_DURATION_MS).contains(&duration_ms) {
            bail!(
                "duration_ms must be between {} and {}",
                MIN_PERF_DURATION_MS,
                MAX_PERF_DURATION_MS
            );
        }
        let resolved_pids = self.resolve_perf_selector(&selector)?;
        if resolved_pids.is_empty() {
            bail!("no live pids resolved for perf selector");
        }

        let output_dir =
            resolve_allow_missing_path(&self.options.workspace_root, Path::new(&output_dir))
                .with_context(|| format!("failed to resolve output dir {output_dir}"))?;
        self.ensure_allowed_path_for_create(&output_dir)?;
        self.prepare_output_dir(&output_dir, overwrite)?;

        let label = label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "perf-sched".to_string());
        let record_argv = build_sched_record_argv(&resolved_pids, &output_dir);
        let timehist_argv = build_sched_timehist_argv(&output_dir);
        let latency_argv = build_sched_latency_argv(&output_dir, latency_by_pid);
        Ok(SchedCollectionSpec {
            label,
            selector,
            resolved_pids,
            output_dir,
            duration_ms,
            latency_by_pid,
            record_argv,
            timehist_argv,
            latency_argv,
        })
    }

    async fn collect_perf(&self, spec: PerfCollectionSpec) -> Result<PerfCollectionSnapshot> {
        let _collection_guard = self.collection_lock.lock().await;
        let selector = spec.selector.clone();
        let resolved_pids = spec.resolved_pids.clone();

        let command_path = spec.output_dir.join("perf.command.json");
        let selector_path = spec.output_dir.join("perf.selector.json");
        let stdout_path = spec.output_dir.join("perf.stdout.log");
        let stderr_path = spec.output_dir.join("perf.stderr.log");
        let primary_output_path = spec.output_dir.join(match spec.mode {
            PerfCollectionMode::Stat => "perf.stat.csv",
            PerfCollectionMode::Record => "perf.data",
        });

        std::fs::write(&command_path, serde_json::to_vec_pretty(&spec.perf_argv)?)?;
        std::fs::write(
            &selector_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "selector": selector,
                "resolved_pids": resolved_pids,
            }))?,
        )?;

        let stdout_file = std::fs::File::create(&stdout_path)
            .with_context(|| format!("failed to create {}", stdout_path.display()))?;
        let stderr_file = std::fs::File::create(&stderr_path)
            .with_context(|| format!("failed to create {}", stderr_path.display()))?;
        let started_at_unix_ms = now_unix_ms();
        let mut child = Command::new("perf")
            .args(&spec.perf_argv)
            .current_dir(&spec.output_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .context("failed to spawn perf collector")?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow!("spawned perf collector did not expose a pid"))?;

        let mut stop_reason = "target_exited".to_string();
        let status = match timeout(Duration::from_millis(spec.duration_ms), child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                stop_reason = "duration_elapsed".to_string();
                if let Err(error) = kill(Pid::from_raw(pid as i32), Signal::SIGINT) {
                    warn!(error = %error, pid, "failed to interrupt perf collector");
                }
                match timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(status) => status?,
                    Err(_) => {
                        stop_reason = "forced_kill".to_string();
                        child.start_kill()?;
                        child.wait().await?
                    }
                }
            }
        };
        let ended_at_unix_ms = now_unix_ms();

        if !primary_output_path.is_file() {
            bail!(
                "perf collector did not produce {} ({})",
                primary_output_path.display(),
                summarize_output_file(&stdout_path, &stderr_path)?
            );
        }
        if !status.success()
            && !(stop_reason == "duration_elapsed"
                && matches!(status.code(), Some(0) | Some(130) | Some(124)))
        {
            bail!(
                "perf collector exited unsuccessfully ({:?}); {}",
                status.code(),
                summarize_output_file(&stdout_path, &stderr_path)?
            );
        }

        Ok(PerfCollectionSnapshot {
            label: spec.label,
            mode: spec.mode,
            selector: spec.selector,
            resolved_pids: spec.resolved_pids,
            requested_duration_ms: spec.duration_ms,
            events: spec.events,
            sample_frequency_hz: spec.sample_frequency_hz,
            call_graph: spec.call_graph,
            output_dir: spec.output_dir.display().to_string(),
            primary_output_path: primary_output_path.display().to_string(),
            command_path: command_path.display().to_string(),
            selector_path: selector_path.display().to_string(),
            stdout_path: stdout_path.display().to_string(),
            stderr_path: stderr_path.display().to_string(),
            started_at_unix_ms,
            ended_at_unix_ms,
            stop_reason,
            exit_code: status.code(),
            signal: status.signal(),
            perf_argv: spec.perf_argv,
        })
    }

    async fn collect_sched(&self, spec: SchedCollectionSpec) -> Result<SchedCollectionSnapshot> {
        let _collection_guard = self.collection_lock.lock().await;

        let record_command_path = spec.output_dir.join("perf.sched.record.command.json");
        let selector_path = spec.output_dir.join("perf.sched.selector.json");
        let record_stdout_path = spec.output_dir.join("perf.sched.record.stdout.log");
        let record_stderr_path = spec.output_dir.join("perf.sched.record.stderr.log");
        let data_path = spec.output_dir.join("perf.sched.data");
        let timehist_path = spec.output_dir.join("perf.sched.timehist.txt");
        let timehist_command_path = spec.output_dir.join("perf.sched.timehist.command.json");
        let timehist_stderr_path = spec.output_dir.join("perf.sched.timehist.stderr.log");
        let latency_path = spec.output_dir.join("perf.sched.latency.txt");
        let latency_command_path = spec.output_dir.join("perf.sched.latency.command.json");
        let latency_stderr_path = spec.output_dir.join("perf.sched.latency.stderr.log");

        std::fs::write(
            &record_command_path,
            serde_json::to_vec_pretty(&spec.record_argv)?,
        )?;
        std::fs::write(
            &timehist_command_path,
            serde_json::to_vec_pretty(&spec.timehist_argv)?,
        )?;
        std::fs::write(
            &latency_command_path,
            serde_json::to_vec_pretty(&spec.latency_argv)?,
        )?;
        std::fs::write(
            &selector_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "selector": spec.selector,
                "resolved_pids": spec.resolved_pids,
            }))?,
        )?;

        let record_stdout = std::fs::File::create(&record_stdout_path)
            .with_context(|| format!("failed to create {}", record_stdout_path.display()))?;
        let record_stderr = std::fs::File::create(&record_stderr_path)
            .with_context(|| format!("failed to create {}", record_stderr_path.display()))?;
        let started_at_unix_ms = now_unix_ms();
        let mut child = Command::new("perf")
            .args(&spec.record_argv)
            .current_dir(&spec.output_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(record_stdout))
            .stderr(Stdio::from(record_stderr))
            .spawn()
            .context("failed to spawn perf sched recorder")?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow!("spawned perf sched recorder did not expose a pid"))?;

        let mut stop_reason = "target_exited".to_string();
        let status = match timeout(Duration::from_millis(spec.duration_ms), child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                stop_reason = "duration_elapsed".to_string();
                if let Err(error) = kill(Pid::from_raw(pid as i32), Signal::SIGINT) {
                    warn!(error = %error, pid, "failed to interrupt perf sched recorder");
                }
                match timeout(Duration::from_secs(5), child.wait()).await {
                    Ok(status) => status?,
                    Err(_) => {
                        stop_reason = "forced_kill".to_string();
                        child.start_kill()?;
                        child.wait().await?
                    }
                }
            }
        };
        let ended_at_unix_ms = now_unix_ms();

        if !data_path.is_file() {
            bail!(
                "perf sched recorder did not produce {} ({})",
                data_path.display(),
                summarize_output_file(&record_stdout_path, &record_stderr_path)?
            );
        }
        if !status.success()
            && !(stop_reason == "duration_elapsed"
                && matches!(status.code(), Some(0) | Some(130) | Some(124)))
        {
            bail!(
                "perf sched recorder exited unsuccessfully ({:?}); {}",
                status.code(),
                summarize_output_file(&record_stdout_path, &record_stderr_path)?
            );
        }

        run_perf_sched_render(
            &spec.output_dir,
            &spec.timehist_argv,
            &timehist_path,
            &timehist_stderr_path,
            "timehist",
        )?;
        run_perf_sched_render(
            &spec.output_dir,
            &spec.latency_argv,
            &latency_path,
            &latency_stderr_path,
            "latency",
        )?;

        Ok(SchedCollectionSnapshot {
            label: spec.label,
            selector: spec.selector,
            resolved_pids: spec.resolved_pids,
            requested_duration_ms: spec.duration_ms,
            output_dir: spec.output_dir.display().to_string(),
            data_path: data_path.display().to_string(),
            record_command_path: record_command_path.display().to_string(),
            selector_path: selector_path.display().to_string(),
            record_stdout_path: record_stdout_path.display().to_string(),
            record_stderr_path: record_stderr_path.display().to_string(),
            timehist_path: timehist_path.display().to_string(),
            timehist_command_path: timehist_command_path.display().to_string(),
            timehist_stderr_path: timehist_stderr_path.display().to_string(),
            latency_path: latency_path.display().to_string(),
            latency_command_path: latency_command_path.display().to_string(),
            latency_stderr_path: latency_stderr_path.display().to_string(),
            latency_by_pid: spec.latency_by_pid,
            started_at_unix_ms,
            ended_at_unix_ms,
            stop_reason,
            exit_code: status.code(),
            signal: status.signal(),
            record_argv: spec.record_argv,
            timehist_argv: spec.timehist_argv,
            latency_argv: spec.latency_argv,
        })
    }

    fn prepare_output_dir(&self, output_dir: &Path, overwrite: bool) -> Result<()> {
        if output_dir.exists() {
            if !output_dir.is_dir() {
                bail!("output path {} is not a directory", output_dir.display());
            }
            if !overwrite && std::fs::read_dir(output_dir)?.next().is_some() {
                bail!(
                    "output directory {} is not empty; pass overwrite=true to reuse it",
                    output_dir.display()
                );
            }
            return Ok(());
        }
        std::fs::create_dir_all(output_dir)
            .with_context(|| format!("failed to create {}", output_dir.display()))?;
        Ok(())
    }

    fn resolve_perf_selector(&self, selector: &PerfTargetSelector) -> Result<Vec<u32>> {
        let mut pids = match selector {
            PerfTargetSelector::Pid { pids } => pids.clone(),
            PerfTargetSelector::Uid { uid } => resolve_proc_selector("Uid", *uid),
            PerfTargetSelector::Gid { gid } => resolve_proc_selector("Gid", *gid),
            PerfTargetSelector::Cgroup { path } => resolve_cgroup_selector(path)?,
        };
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }

    fn ensure_allowed_path_for_create(&self, path: &Path) -> Result<()> {
        let mut cursor = Some(path);
        while let Some(candidate) = cursor {
            if candidate.exists() {
                let canonical = canonicalize_existing(candidate)?;
                self.ensure_allowed_path(&canonical)?;
                return Ok(());
            }
            cursor = candidate.parent();
        }
        bail!("path {} has no existing ancestor", path.display())
    }

    fn validate_launch(
        &self,
        label: Option<String>,
        argv: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        lease_timeout_ms: Option<u64>,
    ) -> Result<LaunchSpec> {
        if argv.is_empty() {
            bail!("activate requires a non-empty argv");
        }
        let cwd = cwd
            .map(PathBuf::from)
            .unwrap_or_else(|| self.options.workspace_root.clone());
        let cwd = canonicalize_maybe_relative(&self.options.workspace_root, &cwd)
            .with_context(|| format!("failed to resolve cwd {}", cwd.display()))?;
        self.ensure_allowed_path(&cwd)?;

        // The daemon never shells out through `sh -c`. It launches one
        // workspace-local executable plus explicit argv so privileged rollout
        // stays attributable to a concrete built artifact.
        let executable_candidate = PathBuf::from(&argv[0]);
        let executable = if executable_candidate.is_absolute() {
            executable_candidate
        } else {
            cwd.join(executable_candidate)
        };
        let executable = canonicalize_existing(&executable)
            .with_context(|| format!("failed to resolve executable {}", executable.display()))?;
        self.ensure_allowed_path(&executable)?;
        let metadata = std::fs::metadata(&executable)?;
        if !metadata.is_file() {
            bail!(
                "executable path {} is not a regular file",
                executable.display()
            );
        }

        let label = label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                executable
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "sched-ext".to_string());

        Ok(LaunchSpec {
            label,
            args: argv.iter().skip(1).cloned().collect(),
            argv,
            executable,
            cwd,
            env,
            lease_timeout_ms: lease_timeout_ms.map(|value| value.max(1)),
        })
    }

    fn ensure_allowed_path(&self, path: &Path) -> Result<()> {
        if self
            .options
            .allowed_roots
            .iter()
            .any(|root| path.starts_with(root))
        {
            return Ok(());
        }
        bail!(
            "path {} is outside the daemon allowlist: {}",
            path.display(),
            self.options
                .allowed_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    async fn start_active(&self, launch: LaunchSpec) -> Result<DaemonStatusSnapshot> {
        let mut command = Command::new(&launch.executable);
        command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            );
        for (key, value) in &launch.env {
            command.env(key, value);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn sched-ext executable {}",
                launch.executable.display()
            )
        })?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow!("spawned child did not expose a pid"))?;
        let logs = Arc::new(Mutex::new(LogBuffer::new(self.options.log_capacity)));
        if let Some(stdout) = child.stdout.take() {
            spawn_stdout_reader(stdout, logs.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_reader(stderr, logs.clone());
        }
        let active = ActiveDeployment {
            label: launch.label,
            argv: launch.argv,
            cwd: launch.cwd,
            pid,
            started_at_unix_s: now_unix_s(),
            lease_timeout_ms: launch.lease_timeout_ms,
            lease_expires_at_unix_ms: launch
                .lease_timeout_ms
                .map(|value| now_unix_ms().saturating_add(value)),
            child: Arc::new(AsyncMutex::new(child)),
            logs,
        };
        let mut state = self.state.lock().unwrap();
        state.last_exit = None;
        state.last_logs.clear();
        state.active = Some(active);
        Ok(self.status_snapshot_locked(&state))
    }

    async fn stop_active(
        &self,
        graceful_timeout_ms: u64,
        reason: StopReason,
    ) -> Result<DaemonStatusSnapshot> {
        self.reap_active_if_exited().await?;
        let Some(active) = self.state.lock().unwrap().active.clone() else {
            return Ok(self.status_snapshot());
        };
        if let Err(error) = kill(Pid::from_raw(active.pid as i32), Signal::SIGTERM) {
            warn!(error = %error, pid = active.pid, "failed to send SIGTERM to active deployment");
        }
        let wait_result = {
            let mut child = active.child.lock().await;
            match timeout(
                Duration::from_millis(graceful_timeout_ms.max(1)),
                child.wait(),
            )
            .await
            {
                Ok(status) => status?,
                Err(_) => {
                    warn!(pid = active.pid, "graceful stop timed out; sending SIGKILL");
                    child.start_kill()?;
                    child.wait().await?
                }
            }
        };
        let exit = build_exit_snapshot(&active, wait_result, reason);
        let logs = active.logs.lock().unwrap().snapshot_all();
        let mut state = self.state.lock().unwrap();
        if state.active.as_ref().map(|deployment| deployment.pid) == Some(active.pid) {
            state.active = None;
            state.last_logs = logs;
            state.last_exit = Some(exit);
        }
        Ok(self.status_snapshot_locked(&state))
    }

    async fn reap_active_if_exited(&self) -> Result<()> {
        let Some(active) = self.state.lock().unwrap().active.clone() else {
            return Ok(());
        };
        let exit_status = {
            let mut child = active.child.lock().await;
            child.try_wait()?
        };
        let Some(exit_status) = exit_status else {
            return Ok(());
        };
        let exit = build_exit_snapshot(&active, exit_status, StopReason::Exited);
        let logs = active.logs.lock().unwrap().snapshot_all();
        let mut state = self.state.lock().unwrap();
        if state.active.as_ref().map(|deployment| deployment.pid) == Some(active.pid) {
            state.active = None;
            state.last_exit = Some(exit);
            state.last_logs = logs;
        }
        Ok(())
    }

    async fn enforce_active_lease(&self) -> Result<()> {
        let Some(active) = self.state.lock().unwrap().active.clone() else {
            return Ok(());
        };
        let Some(lease_expires_at_unix_ms) = active.lease_expires_at_unix_ms else {
            return Ok(());
        };
        if now_unix_ms() < lease_expires_at_unix_ms {
            return Ok(());
        }
        warn!(
            pid = active.pid,
            label = %active.label,
            lease_expires_at_unix_ms,
            "active sched-ext deployment exceeded its lease; stopping"
        );
        let _ = self
            .stop_active(DEFAULT_STOP_TIMEOUT_MS, StopReason::LeaseExpired)
            .await?;
        Ok(())
    }

    fn status_snapshot(&self) -> DaemonStatusSnapshot {
        let state = self.state.lock().unwrap();
        self.status_snapshot_locked(&state)
    }

    fn status_snapshot_locked(&self, state: &DaemonState) -> DaemonStatusSnapshot {
        DaemonStatusSnapshot {
            daemon_pid: std::process::id(),
            workspace_root: self.options.workspace_root.display().to_string(),
            socket_path: self.options.socket_path.display().to_string(),
            allowed_roots: self
                .options
                .allowed_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect(),
            active: state
                .active
                .as_ref()
                .map(|active| ActiveDeploymentSnapshot {
                    label: active.label.clone(),
                    argv: active.argv.clone(),
                    cwd: active.cwd.display().to_string(),
                    pid: active.pid,
                    started_at_unix_s: active.started_at_unix_s,
                    lease_timeout_ms: active.lease_timeout_ms,
                    lease_expires_at_unix_ms: active.lease_expires_at_unix_ms,
                    log_line_count: active.logs.lock().unwrap().len(),
                }),
            last_exit: state.last_exit.clone(),
        }
    }

    fn logs_snapshot(&self, tail_lines: usize) -> DaemonLogsSnapshot {
        let state = self.state.lock().unwrap();
        if let Some(active) = &state.active {
            let (lines, truncated) = active.logs.lock().unwrap().tail(tail_lines.max(1));
            return DaemonLogsSnapshot {
                active_label: Some(active.label.clone()),
                truncated,
                lines,
            };
        }
        let total = state.last_logs.len();
        let tail = tail_lines.max(1);
        let start = total.saturating_sub(tail);
        DaemonLogsSnapshot {
            active_label: state
                .last_exit
                .as_ref()
                .map(|snapshot| snapshot.label.clone()),
            truncated: start > 0,
            lines: state.last_logs[start..].to_vec(),
        }
    }
}

impl LogBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            lines: VecDeque::new(),
        }
    }

    fn push(&mut self, source: &str, line: String) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(DaemonLogLine {
            source: source.to_string(),
            emitted_at_unix_ms: now_unix_ms(),
            line,
        });
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    fn snapshot_all(&self) -> Vec<DaemonLogLine> {
        self.lines.iter().cloned().collect()
    }

    fn tail(&self, tail_lines: usize) -> (Vec<DaemonLogLine>, bool) {
        let total = self.lines.len();
        let start = total.saturating_sub(tail_lines.max(1));
        (self.lines.iter().skip(start).cloned().collect(), start > 0)
    }
}

fn spawn_stdout_reader(stdout: ChildStdout, logs: Arc<Mutex<LogBuffer>>) {
    tokio::spawn(async move {
        if let Err(error) = read_log_stream("stdout", stdout, logs.clone()).await {
            logs.lock()
                .unwrap()
                .push("internal", format!("stdout reader failed: {error}"));
        }
    });
}

fn spawn_stderr_reader(stderr: ChildStderr, logs: Arc<Mutex<LogBuffer>>) {
    tokio::spawn(async move {
        if let Err(error) = read_log_stream("stderr", stderr, logs.clone()).await {
            logs.lock()
                .unwrap()
                .push("internal", format!("stderr reader failed: {error}"));
        }
    });
}

async fn read_log_stream<T>(source: &str, stream: T, logs: Arc<Mutex<LogBuffer>>) -> Result<()>
where
    T: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            break;
        }
        logs.lock()
            .unwrap()
            .push(source, line.trim_end_matches('\n').to_string());
    }
    Ok(())
}

fn build_exit_snapshot(
    active: &ActiveDeployment,
    status: std::process::ExitStatus,
    reason: StopReason,
) -> DeploymentExitSnapshot {
    DeploymentExitSnapshot {
        label: active.label.clone(),
        argv: active.argv.clone(),
        cwd: active.cwd.display().to_string(),
        pid: active.pid,
        started_at_unix_s: active.started_at_unix_s,
        ended_at_unix_s: now_unix_s(),
        exit_code: status.code(),
        signal: status.signal(),
        exit_reason: reason.as_str().to_string(),
        lease_timeout_ms: active.lease_timeout_ms,
        lease_expires_at_unix_ms: active.lease_expires_at_unix_ms,
        log_line_count: active.logs.lock().unwrap().len(),
    }
}

fn build_perf_argv(
    mode: PerfCollectionMode,
    resolved_pids: &[u32],
    events: &[String],
    sample_frequency_hz: Option<u32>,
    call_graph: Option<PerfCallGraphMode>,
    output_dir: &Path,
) -> Vec<String> {
    let mut argv = Vec::new();
    match mode {
        PerfCollectionMode::Stat => {
            argv.extend([
                "stat".to_string(),
                "-x,".to_string(),
                "--no-big-num".to_string(),
                "-o".to_string(),
                output_dir.join("perf.stat.csv").display().to_string(),
            ]);
        }
        PerfCollectionMode::Record => {
            argv.extend([
                "record".to_string(),
                "-o".to_string(),
                output_dir.join("perf.data").display().to_string(),
            ]);
            if let Some(frequency) = sample_frequency_hz {
                argv.extend(["--freq".to_string(), frequency.to_string()]);
            }
            if let Some(mode) = call_graph {
                argv.extend(["--call-graph".to_string(), perf_call_graph_arg(mode)]);
            }
        }
    }
    for event in events {
        argv.extend(["-e".to_string(), event.clone()]);
    }
    argv.extend([
        "-p".to_string(),
        resolved_pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(","),
    ]);
    argv
}

fn build_sched_record_argv(resolved_pids: &[u32], output_dir: &Path) -> Vec<String> {
    vec![
        "sched".to_string(),
        "record".to_string(),
        "-o".to_string(),
        output_dir.join("perf.sched.data").display().to_string(),
        "-p".to_string(),
        resolved_pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(","),
    ]
}

fn build_sched_timehist_argv(output_dir: &Path) -> Vec<String> {
    vec![
        "sched".to_string(),
        "timehist".to_string(),
        "-i".to_string(),
        output_dir.join("perf.sched.data").display().to_string(),
    ]
}

fn build_sched_latency_argv(output_dir: &Path, by_pid: bool) -> Vec<String> {
    let mut argv = vec![
        "sched".to_string(),
        "latency".to_string(),
        "-i".to_string(),
        output_dir.join("perf.sched.data").display().to_string(),
    ];
    if by_pid {
        argv.push("-p".to_string());
    }
    argv
}

fn perf_call_graph_arg(mode: PerfCallGraphMode) -> String {
    match mode {
        PerfCallGraphMode::FramePointer => "fp".to_string(),
        PerfCallGraphMode::Dwarf => "dwarf".to_string(),
        PerfCallGraphMode::Lbr => "lbr".to_string(),
    }
}

fn validate_perf_event(event: &str) -> Result<()> {
    let trimmed = event.trim();
    if trimmed.is_empty() {
        bail!("perf events must be non-empty");
    }
    if trimmed
        .chars()
        .any(|value| matches!(value, '\0' | '\n' | '\r'))
    {
        bail!("perf events may not contain control characters");
    }
    Ok(())
}

fn resolve_proc_selector(field: &str, expected: u32) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let status_path = entry.path().join("status");
        let Ok(contents) = std::fs::read_to_string(status_path) else {
            continue;
        };
        let matched = contents.lines().find_map(|line| {
            let (key, values) = line.split_once(':')?;
            if key != field {
                return None;
            }
            values
                .split_whitespace()
                .next()?
                .parse::<u32>()
                .ok()
                .map(|value| value == expected)
        });
        if matched == Some(true) {
            pids.push(pid);
        }
    }
    pids
}

fn resolve_cgroup_selector(raw_path: &str) -> Result<Vec<u32>> {
    let raw_path = Path::new(raw_path);
    if raw_path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("cgroup selector may not contain parent directory segments");
    }
    let cgroup_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        Path::new("/sys/fs/cgroup").join(raw_path)
    };
    if !cgroup_path.starts_with("/sys/fs/cgroup") {
        bail!(
            "cgroup path {} must stay under /sys/fs/cgroup",
            cgroup_path.display()
        );
    }
    let procs_path = if cgroup_path.is_dir() {
        cgroup_path.join("cgroup.procs")
    } else {
        cgroup_path
    };
    let contents = std::fs::read_to_string(&procs_path)
        .with_context(|| format!("failed to read {}", procs_path.display()))?;
    Ok(contents
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect())
}

fn summarize_output_file(stdout_path: &Path, stderr_path: &Path) -> Result<String> {
    let stdout = std::fs::read(stdout_path).unwrap_or_default();
    let stderr = std::fs::read(stderr_path).unwrap_or_default();
    Ok(summarize_output(&stdout, &stderr))
}

fn run_perf_sched_render(
    cwd: &Path,
    argv: &[String],
    output_path: &Path,
    stderr_path: &Path,
    label: &str,
) -> Result<()> {
    let stdout = std::fs::File::create(output_path)
        .with_context(|| format!("failed to create {}", output_path.display()))?;
    let stderr = std::fs::File::create(stderr_path)
        .with_context(|| format!("failed to create {}", stderr_path.display()))?;
    let status = std::process::Command::new("perf")
        .args(argv)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .status()
        .with_context(|| format!("failed to run perf sched {label}"))?;
    if !status.success() || !output_path.is_file() {
        bail!(
            "perf sched {label} exited unsuccessfully ({:?}); {}",
            status.code(),
            summarize_output_file(output_path, stderr_path)?
        );
    }
    Ok(())
}

fn summarize_output(stdout: &[u8], stderr: &[u8]) -> String {
    for source in [stderr, stdout] {
        if let Some(line) = String::from_utf8_lossy(source)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            return line.to_string();
        }
    }
    "<no output>".to_string()
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path)
        .with_context(|| format!("failed to canonicalize {}", path.display()))
}

fn resolve_allow_missing_path(base: &Path, path: &Path) -> Result<PathBuf> {
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!(
            "path {} may not contain parent directory segments",
            path.display()
        );
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    if candidate.exists() {
        return canonicalize_existing(&candidate);
    }

    let mut existing = candidate.as_path();
    let mut missing = Vec::<OsString>::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            bail!("path {} has no existing ancestor", candidate.display());
        };
        missing.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            bail!("path {} has no existing ancestor", candidate.display());
        };
        existing = parent;
    }

    let mut resolved = canonicalize_existing(existing)?;
    for segment in missing.iter().rev() {
        resolved.push(segment);
    }
    Ok(resolved)
}

fn canonicalize_maybe_relative(base: &Path, path: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    canonicalize_existing(&candidate)
}

fn now_unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{DaemonServer, ServeOptions};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn validate_launch_rejects_paths_outside_allowed_roots() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_exec = outside.path().join("scx-demo");
        std::fs::write(&outside_exec, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&outside_exec).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&outside_exec, permissions).unwrap();

        let server = DaemonServer::new(ServeOptions {
            workspace_root: workspace.path().to_path_buf(),
            socket_path: workspace.path().join("sched-claw.sock"),
            allowed_roots: vec![workspace.path().to_path_buf()],
            log_capacity: 16,
            client_uid: None,
            client_gid: None,
        })
        .unwrap();

        let error = server
            .validate_launch(
                None,
                vec![outside_exec.display().to_string()],
                None,
                BTreeMap::new(),
                None,
            )
            .unwrap_err();

        assert!(error.to_string().contains("outside the daemon allowlist"));
    }

    #[test]
    fn validate_launch_accepts_workspace_relative_executable() {
        let workspace = tempdir().unwrap();
        let bin_dir = workspace.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let executable = bin_dir.join("scx-demo");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let server = DaemonServer::new(ServeOptions {
            workspace_root: workspace.path().to_path_buf(),
            socket_path: workspace.path().join("sched-claw.sock"),
            allowed_roots: vec![workspace.path().to_path_buf()],
            log_capacity: 16,
            client_uid: None,
            client_gid: None,
        })
        .unwrap();

        let launch = server
            .validate_launch(
                None,
                vec!["./bin/scx-demo".to_string(), "--demo".to_string()],
                None,
                BTreeMap::new(),
                None,
            )
            .unwrap();

        assert_eq!(launch.label, "scx-demo");
        assert_eq!(launch.args, vec!["--demo".to_string()]);
        assert!(launch.executable.ends_with("bin/scx-demo"));
    }
}
