use crate::daemon_protocol::{
    ActiveDeploymentSnapshot, DEFAULT_LOG_TAIL_LINES, DEFAULT_STOP_TIMEOUT_MS, DaemonLogLine,
    DaemonLogsSnapshot, DaemonStatusSnapshot, DeploymentExitSnapshot, SchedExtDaemonRequest,
    SchedExtDaemonResponse,
};
use anyhow::{Context, Result, anyhow, bail};
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Gid, Pid, Uid, chown};
use std::collections::{BTreeMap, VecDeque};
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
use tokio::time::{Duration, timeout};
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
        let _ = self.stop_active(DEFAULT_STOP_TIMEOUT_MS).await?;
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
            SchedExtDaemonRequest::Activate {
                label,
                argv,
                cwd,
                env,
                replace_existing,
            } => {
                if replace_existing {
                    let _ = self.stop_active(DEFAULT_STOP_TIMEOUT_MS).await?;
                } else if self.state.lock().unwrap().active.is_some() {
                    bail!(
                        "a sched-ext deployment is already active; stop it first or set replace_existing=true"
                    );
                }
                let launch = self.validate_launch(label, argv, cwd, env)?;
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
                    .stop_active(graceful_timeout_ms.unwrap_or(DEFAULT_STOP_TIMEOUT_MS))
                    .await?;
                Ok(SchedExtDaemonResponse::Ack {
                    message: "stopped active sched-ext deployment".to_string(),
                    snapshot,
                })
            }
        }
    }

    fn validate_launch(
        &self,
        label: Option<String>,
        argv: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
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
            child: Arc::new(AsyncMutex::new(child)),
            logs,
        };
        let mut state = self.state.lock().unwrap();
        state.last_exit = None;
        state.last_logs.clear();
        state.active = Some(active);
        Ok(self.status_snapshot_locked(&state))
    }

    async fn stop_active(&self, graceful_timeout_ms: u64) -> Result<DaemonStatusSnapshot> {
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
        let exit = build_exit_snapshot(&active, wait_result);
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
        let exit = build_exit_snapshot(&active, exit_status);
        let logs = active.logs.lock().unwrap().snapshot_all();
        let mut state = self.state.lock().unwrap();
        if state.active.as_ref().map(|deployment| deployment.pid) == Some(active.pid) {
            state.active = None;
            state.last_exit = Some(exit);
            state.last_logs = logs;
        }
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
        log_line_count: active.logs.lock().unwrap().len(),
    }
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path)
        .with_context(|| format!("failed to canonicalize {}", path.display()))
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
            )
            .unwrap();

        assert_eq!(launch.label, "scx-demo");
        assert_eq!(launch.args, vec!["--demo".to_string()]);
        assert!(launch.executable.ends_with("bin/scx-demo"));
    }
}
