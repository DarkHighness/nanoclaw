use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolError};
use agent_env::shell_or_default;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tracing::{debug, warn};
use types::{
    MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec, new_opaque_id,
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 32 * 1024;
const MAX_ALLOWED_OUTPUT_CHARS: usize = 256 * 1024;
const DEFAULT_POLL_WAIT_MS: u64 = 0;
const MAX_POLL_WAIT_MS: u64 = 30_000;
const MAX_TRACKED_BASH_SESSIONS: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BashExecutionMode {
    Run,
    Start,
    Poll,
    Continue,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BashToolInput {
    #[serde(default)]
    pub mode: Option<BashExecutionMode>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub poll_wait_ms: Option<u64>,
    #[serde(default)]
    pub stdout_start_char: Option<usize>,
    #[serde(default)]
    pub stderr_start_char: Option<usize>,
    #[serde(default)]
    pub max_output_chars: Option<usize>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default)]
pub struct BashTool;

#[derive(Clone, Debug)]
struct OutputSlice {
    text: String,
    truncated: bool,
    original_chars: usize,
}

#[derive(Clone, Debug)]
struct OutputWindow {
    text: String,
    start_char: usize,
    end_char: usize,
    total_chars: usize,
    truncated: bool,
    remaining_chars: usize,
    next_start_char: Option<usize>,
}

#[derive(Clone, Debug)]
enum SessionStatus {
    Running,
    Completed {
        exit_code: i32,
        timed_out: bool,
        cancelled: bool,
        error: Option<String>,
        finished_at_unix_s: u64,
    },
}

impl SessionStatus {
    fn completed(exit_code: i32, timed_out: bool, cancelled: bool, error: Option<String>) -> Self {
        Self::Completed {
            exit_code,
            timed_out,
            cancelled,
            error,
            finished_at_unix_s: unix_timestamp_s(),
        }
    }

    fn state_label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed {
                cancelled: true, ..
            } => "cancelled",
            Self::Completed {
                timed_out: true, ..
            } => "timed_out",
            Self::Completed { error: Some(_), .. } => "failed",
            Self::Completed { .. } => "completed",
        }
    }
}

#[derive(Clone, Debug)]
struct SessionStatusSnapshot {
    state: &'static str,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
    error: Option<String>,
    finished_at_unix_s: Option<u64>,
}

#[derive(Debug)]
struct BashSession {
    id: String,
    command: String,
    cwd: PathBuf,
    shell: String,
    timeout_ms: u64,
    env_keys: Vec<String>,
    started_at_unix_s: u64,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    status: Arc<Mutex<SessionStatus>>,
    completion: Arc<Notify>,
    cancel_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl BashSession {
    fn snapshot_status(&self) -> SessionStatusSnapshot {
        let status = self.status.lock().expect("bash session status lock");
        match &*status {
            SessionStatus::Running => SessionStatusSnapshot {
                state: "running",
                exit_code: None,
                timed_out: false,
                cancelled: false,
                error: None,
                finished_at_unix_s: None,
            },
            SessionStatus::Completed {
                exit_code,
                timed_out,
                cancelled,
                error,
                finished_at_unix_s,
            } => SessionStatusSnapshot {
                state: status.state_label(),
                exit_code: Some(*exit_code),
                timed_out: *timed_out,
                cancelled: *cancelled,
                error: error.clone(),
                finished_at_unix_s: Some(*finished_at_unix_s),
            },
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            *self.status.lock().expect("bash session status lock"),
            SessionStatus::Running
        )
    }

    fn cancel(&self) -> bool {
        self.cancel_tx
            .lock()
            .expect("bash session cancel lock")
            .take()
            .is_some_and(|tx| tx.send(()).is_ok())
    }

    fn completed_timestamp(&self) -> Option<u64> {
        match &*self.status.lock().expect("bash session status lock") {
            SessionStatus::Completed {
                finished_at_unix_s, ..
            } => Some(*finished_at_unix_s),
            SessionStatus::Running => None,
        }
    }

    fn output_windows(
        &self,
        max_output_chars: usize,
        stdout_start_char: usize,
        stderr_start_char: usize,
    ) -> (OutputWindow, OutputWindow) {
        let stdout = self.stdout.lock().expect("stdout lock");
        let stderr = self.stderr.lock().expect("stderr lock");
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        (
            slice_output_window(stdout.as_ref(), stdout_start_char, max_output_chars),
            slice_output_window(stderr.as_ref(), stderr_start_char, max_output_chars),
        )
    }
}

type SessionRegistry = HashMap<String, Arc<BashSession>>;
static BASH_SESSIONS: OnceLock<RwLock<SessionRegistry>> = OnceLock::new();

impl BashTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "Run shell commands in the workspace. Supports synchronous run, long-running background sessions with poll/continue, and cancellation.".to_string(),
            input_schema: serde_json::to_value(schema_for!(BashToolInput)).expect("bash schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Run Shell Command", false, true, false, true),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: BashToolInput = serde_json::from_value(arguments)?;
        match input.mode.unwrap_or(BashExecutionMode::Run) {
            BashExecutionMode::Run => execute_run(call_id, input, ctx).await,
            BashExecutionMode::Start => execute_start(call_id, input, ctx).await,
            BashExecutionMode::Poll | BashExecutionMode::Continue => {
                execute_poll(call_id, input).await
            }
            BashExecutionMode::Cancel => execute_cancel(call_id, input).await,
        }
    }
}

fn bash_sessions() -> &'static RwLock<SessionRegistry> {
    BASH_SESSIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

async fn execute_run(
    call_id: ToolCallId,
    input: BashToolInput,
    ctx: &ToolExecutionContext,
) -> Result<ToolResult> {
    let external_call_id = call_id.to_string();
    let command = resolve_command(&input)?;
    let cwd = resolve_cwd(&input, ctx)?;
    let shell = shell_or_default("/bin/sh");
    let max_output_chars = input
        .max_output_chars
        .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS)
        .clamp(1, MAX_ALLOWED_OUTPUT_CHARS);
    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1);

    let mut child = Command::new(&shell);
    child
        .arg("-lc")
        .arg(&command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(env) = &input.env {
        child.envs(env);
    }

    let future = child.output();
    let output = match timeout(Duration::from_millis(timeout_ms), future).await {
        Ok(result) => result?,
        Err(_) => {
            warn!(
                cwd = %cwd.display(),
                timeout_ms,
                "bash run command timed out"
            );
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.into(),
                tool_name: "bash".to_string(),
                parts: vec![MessagePart::text(format!(
                    "[bash cwd={} timeout_ms={} mode=run]\nCommand timed out after {timeout_ms}ms.\ncommand> {}",
                    cwd.display(),
                    timeout_ms,
                    command
                ))],
                metadata: Some(serde_json::json!({
                    "mode": "run",
                    "cwd": cwd,
                    "shell": shell,
                    "command": command,
                    "timeout_ms": timeout_ms,
                    "timed_out": true,
                })),
                is_error: true,
            });
        }
    };

    let stdout = truncate_output(&String::from_utf8_lossy(&output.stdout), max_output_chars);
    let stderr = truncate_output(&String::from_utf8_lossy(&output.stderr), max_output_chars);
    let exit_code = output.status.code().unwrap_or(-1);
    let text = render_output(&command, &cwd, exit_code, timeout_ms, &stdout, &stderr);

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id.into(),
        tool_name: "bash".to_string(),
        parts: vec![MessagePart::text(text)],
        metadata: Some(serde_json::json!({
            "mode": "run",
            "cwd": cwd,
            "shell": shell,
            "command": command,
            "timeout_ms": timeout_ms,
            "exit_code": exit_code,
            "timed_out": false,
            "max_output_chars": max_output_chars,
            "stdout": {
                "chars": stdout.original_chars,
                "truncated": stdout.truncated,
            },
            "stderr": {
                "chars": stderr.original_chars,
                "truncated": stderr.truncated,
            },
            "env": input.env.as_ref().map(|env| env.keys().cloned().collect::<Vec<_>>()),
        })),
        is_error: !output.status.success(),
    })
}

async fn execute_start(
    call_id: ToolCallId,
    input: BashToolInput,
    ctx: &ToolExecutionContext,
) -> Result<ToolResult> {
    let external_call_id = call_id.to_string();
    let command = resolve_command(&input)?;
    let cwd = resolve_cwd(&input, ctx)?;
    let shell = shell_or_default("/bin/sh");
    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1);
    let env = input.env.unwrap_or_default();
    let env_keys = env.keys().cloned().collect::<Vec<_>>();
    let started_at_unix_s = unix_timestamp_s();

    let mut child = Command::new(&shell);
    child
        .arg("-lc")
        .arg(&command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .envs(env);

    let child = child.spawn()?;
    let session_id = format!("bash-{}", new_opaque_id());
    debug!(
        session_id = %session_id,
        cwd = %cwd.display(),
        timeout_ms,
        "started background bash session"
    );
    let stdout = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
    let status = Arc::new(Mutex::new(SessionStatus::Running));
    let completion = Arc::new(Notify::new());
    let (cancel_tx, cancel_rx) = oneshot::channel();

    let session = Arc::new(BashSession {
        id: session_id.clone(),
        command: command.clone(),
        cwd: cwd.clone(),
        shell: shell.clone(),
        timeout_ms,
        env_keys,
        started_at_unix_s,
        stdout: stdout.clone(),
        stderr: stderr.clone(),
        status: status.clone(),
        completion: completion.clone(),
        cancel_tx: Arc::new(Mutex::new(Some(cancel_tx))),
    });

    {
        let mut registry = bash_sessions()
            .write()
            .expect("bash session registry write lock");
        prune_completed_sessions(&mut registry);
        registry.insert(session_id.clone(), session);
    }

    tokio::spawn(async move {
        run_background_command(
            child, timeout_ms, stdout, stderr, status, completion, cancel_rx,
        )
        .await;
    });

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id.into(),
        tool_name: "bash".to_string(),
        parts: vec![MessagePart::text(format!(
            "[bash session_id={} state=running mode=start]\ncommand> {}\ncwd> {}\ntimeout_ms> {}\n\nUse mode=\"poll\" with this session_id to collect output.\nUse mode=\"cancel\" to stop the session.",
            session_id,
            command,
            cwd.display(),
            timeout_ms
        ))],
        metadata: Some(serde_json::json!({
            "mode": "start",
            "session_id": session_id,
            "state": "running",
            "command": command,
            "cwd": cwd,
            "shell": shell,
            "timeout_ms": timeout_ms,
            "started_at_unix_s": started_at_unix_s,
        })),
        is_error: false,
    })
}

async fn execute_poll(call_id: ToolCallId, input: BashToolInput) -> Result<ToolResult> {
    let external_call_id = call_id.to_string();
    let session_id = resolve_session_id(&input)?;
    let max_output_chars = input
        .max_output_chars
        .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS)
        .clamp(1, MAX_ALLOWED_OUTPUT_CHARS);
    let poll_wait_ms = input
        .poll_wait_ms
        .unwrap_or(DEFAULT_POLL_WAIT_MS)
        .min(MAX_POLL_WAIT_MS);
    let stdout_start_char = input.stdout_start_char.unwrap_or(0);
    let stderr_start_char = input.stderr_start_char.unwrap_or(0);

    let session = {
        let registry = bash_sessions()
            .read()
            .expect("bash session registry read lock");
        registry.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Ok(ToolResult::error(
            call_id,
            "bash",
            format!("Unknown bash session_id `{session_id}`"),
        ));
    };

    if poll_wait_ms > 0 && session.is_running() {
        tokio::select! {
            _ = session.completion.notified() => {}
            _ = sleep(Duration::from_millis(poll_wait_ms)) => {}
        }
    }

    let status = session.snapshot_status();
    let (stdout, stderr) =
        session.output_windows(max_output_chars, stdout_start_char, stderr_start_char);
    debug!(
        session_id = %session.id,
        state = status.state,
        stdout_start = stdout.start_char,
        stderr_start = stderr.start_char,
        "polled bash session"
    );
    let text = render_poll_output(&session, &status, &stdout, &stderr, max_output_chars);

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id.into(),
        tool_name: "bash".to_string(),
        parts: vec![MessagePart::text(text)],
        metadata: Some(serde_json::json!({
            "mode": "poll",
            "session_id": session.id,
            "state": status.state,
            "exit_code": status.exit_code,
            "timed_out": status.timed_out,
            "cancelled": status.cancelled,
            "error": status.error,
            "started_at_unix_s": session.started_at_unix_s,
            "finished_at_unix_s": status.finished_at_unix_s,
            "command": session.command,
            "cwd": session.cwd,
            "shell": session.shell,
            "timeout_ms": session.timeout_ms,
            "poll_wait_ms": poll_wait_ms,
            "max_output_chars": max_output_chars,
            "stdout": {
                "start_char": stdout.start_char,
                "end_char": stdout.end_char,
                "total_chars": stdout.total_chars,
                "remaining_chars": stdout.remaining_chars,
                "truncated": stdout.truncated,
                "next_start_char": stdout.next_start_char,
            },
            "stderr": {
                "start_char": stderr.start_char,
                "end_char": stderr.end_char,
                "total_chars": stderr.total_chars,
                "remaining_chars": stderr.remaining_chars,
                "truncated": stderr.truncated,
                "next_start_char": stderr.next_start_char,
            },
            "env": session.env_keys,
        })),
        is_error: status.error.is_some()
            || (matches!(status.exit_code, Some(code) if code != 0)
                && !status.cancelled
                && !status.timed_out),
    })
}

async fn execute_cancel(call_id: ToolCallId, input: BashToolInput) -> Result<ToolResult> {
    let external_call_id = call_id.to_string();
    let session_id = resolve_session_id(&input)?;
    let poll_wait_ms = input.poll_wait_ms.unwrap_or(1_000).min(MAX_POLL_WAIT_MS);

    let session = {
        let registry = bash_sessions()
            .read()
            .expect("bash session registry read lock");
        registry.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return Ok(ToolResult::error(
            call_id,
            "bash",
            format!("Unknown bash session_id `{session_id}`"),
        ));
    };

    let cancellation_requested = session.cancel();
    debug!(
        session_id = %session.id,
        cancellation_requested,
        "requested bash session cancellation"
    );
    if poll_wait_ms > 0 && session.is_running() {
        tokio::select! {
            _ = session.completion.notified() => {}
            _ = sleep(Duration::from_millis(poll_wait_ms)) => {}
        }
    }
    let status = session.snapshot_status();

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id.into(),
        tool_name: "bash".to_string(),
        parts: vec![MessagePart::text(format!(
            "[bash session_id={} mode=cancel]\ncancellation_requested> {}\nstate> {}\nexit_code> {}\n",
            session.id,
            cancellation_requested,
            status.state,
            status
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "running".to_string())
        ))],
        metadata: Some(serde_json::json!({
            "mode": "cancel",
            "session_id": session.id,
            "cancellation_requested": cancellation_requested,
            "state": status.state,
            "exit_code": status.exit_code,
            "timed_out": status.timed_out,
            "cancelled": status.cancelled,
            "error": status.error,
            "finished_at_unix_s": status.finished_at_unix_s,
        })),
        is_error: false,
    })
}

fn prune_completed_sessions(registry: &mut SessionRegistry) {
    if registry.len() < MAX_TRACKED_BASH_SESSIONS {
        return;
    }
    let mut completed = registry
        .iter()
        .filter_map(|(session_id, session)| {
            session
                .completed_timestamp()
                .map(|finished_at| (session_id.clone(), finished_at))
        })
        .collect::<Vec<_>>();
    completed.sort_by_key(|(_, finished_at)| *finished_at);

    let remove_count = registry.len().saturating_sub(MAX_TRACKED_BASH_SESSIONS) + 1;
    for (session_id, _) in completed.into_iter().take(remove_count) {
        registry.remove(&session_id);
    }
}

async fn run_background_command(
    mut child: Child,
    timeout_ms: u64,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    status: Arc<Mutex<SessionStatus>>,
    completion: Arc<Notify>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let stdout_task = child
        .stdout
        .take()
        .map(|pipe| spawn_capture_task(pipe, stdout));
    let stderr_task = child
        .stderr
        .take()
        .map(|pipe| spawn_capture_task(pipe, stderr));

    // The monitor owns cancellation, timeout, and exit-state transitions so
    // `poll` and `cancel` callers observe one consistent session contract.
    let next_status = tokio::select! {
        _ = &mut cancel_rx => {
            let _ = child.kill().await;
            match child.wait().await {
                Ok(wait_status) => SessionStatus::completed(wait_status.code().unwrap_or(-1), false, true, None),
                Err(error) => SessionStatus::completed(-1, false, true, Some(error.to_string())),
            }
        }
        wait_result = timeout(Duration::from_millis(timeout_ms.max(1)), child.wait()) => {
            match wait_result {
                Ok(Ok(wait_status)) => SessionStatus::completed(wait_status.code().unwrap_or(-1), false, false, None),
                Ok(Err(error)) => SessionStatus::completed(-1, false, false, Some(error.to_string())),
                Err(_) => {
                    let _ = child.kill().await;
                    match child.wait().await {
                        Ok(wait_status) => SessionStatus::completed(wait_status.code().unwrap_or(-1), true, false, None),
                        Err(error) => SessionStatus::completed(-1, true, false, Some(error.to_string())),
                    }
                }
            }
        }
    };

    // Surface the terminal session state as soon as process lifecycle settles.
    // Output draining can lag behind exit/cancel because descendants may keep the
    // stdio pipes open briefly; control-plane callers should not stay stuck in
    // `running` while logs finish flushing.
    *status.lock().expect("bash session status lock") = next_status;
    completion.notify_waiters();

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
}

fn spawn_capture_task<R>(mut stream: R, buffer: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0u8; 4 * 1024];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(read) => {
                    buffer
                        .lock()
                        .expect("bash output buffer lock")
                        .extend_from_slice(&chunk[..read]);
                }
                Err(_) => break,
            }
        }
    })
}

fn resolve_command(input: &BashToolInput) -> Result<String> {
    input
        .command
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::invalid("bash requires a non-empty `command` for this mode"))
}

fn resolve_session_id(input: &BashToolInput) -> Result<String> {
    input
        .session_id
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::invalid("bash requires a non-empty `session_id` for this mode"))
}

fn resolve_cwd(input: &BashToolInput, ctx: &ToolExecutionContext) -> Result<PathBuf> {
    let cwd = resolve_tool_path_against_workspace_root(
        input.cwd.as_deref().unwrap_or("."),
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    if ctx.workspace_only {
        ctx.assert_path_allowed(&cwd)?;
    }
    Ok(cwd)
}

fn slice_output_window(output: &str, start_char: usize, max_chars: usize) -> OutputWindow {
    let total_chars = output.chars().count();
    let start_char = start_char.min(total_chars);
    let tail = output.chars().skip(start_char).collect::<String>();
    let (preview, truncated) = truncate_text_with_limit(&tail, max_chars);
    let returned_chars = preview.chars().count();
    let end_char = start_char + returned_chars;
    let remaining_chars = total_chars.saturating_sub(end_char);
    OutputWindow {
        text: preview,
        start_char,
        end_char,
        total_chars,
        truncated,
        remaining_chars,
        next_start_char: truncated.then_some(end_char),
    }
}

fn render_poll_output(
    session: &BashSession,
    status: &SessionStatusSnapshot,
    stdout: &OutputWindow,
    stderr: &OutputWindow,
    max_output_chars: usize,
) -> String {
    let mut sections = vec![
        format!(
            "[bash session_id={} state={} mode=poll]",
            session.id, status.state
        ),
        format!("command> {}", session.command),
        format!("cwd> {}", session.cwd.display()),
        format!("stdout_start_char> {}", stdout.start_char),
        format!("stdout_end_char> {}", stdout.end_char),
        format!("stdout_total_chars> {}", stdout.total_chars),
        format!("stderr_start_char> {}", stderr.start_char),
        format!("stderr_end_char> {}", stderr.end_char),
        format!("stderr_total_chars> {}", stderr.total_chars),
    ];
    if let Some(exit_code) = status.exit_code {
        sections.push(format!("exit_code> {exit_code}"));
    }
    if status.timed_out {
        sections.push("timed_out> true".to_string());
    }
    if status.cancelled {
        sections.push("cancelled> true".to_string());
    }
    if let Some(error) = &status.error {
        sections.push(format!("error> {error}"));
    }
    if !stdout.text.is_empty() {
        sections.push(format!("stdout>\n{}", stdout.text));
    }
    if !stderr.text.is_empty() {
        sections.push(format!("stderr>\n{}", stderr.text));
    }
    if let Some(next_stdout) = stdout.next_start_char {
        sections.push(format!(
            "[stdout truncated to {max_output_chars} chars; continue with stdout_start_char={next_stdout}]"
        ));
    }
    if let Some(next_stderr) = stderr.next_start_char {
        sections.push(format!(
            "[stderr truncated to {max_output_chars} chars; continue with stderr_start_char={next_stderr}]"
        ));
    }
    sections.join("\n\n")
}

fn truncate_text_with_limit(value: &str, limit: usize) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= limit {
        return (value.to_string(), false);
    }
    (value.chars().take(limit).collect::<String>(), true)
}

fn truncate_output(output: &str, limit: usize) -> OutputSlice {
    let trimmed = output.trim_end();
    let original_chars = trimmed.chars().count();
    if original_chars <= limit {
        return OutputSlice {
            text: trimmed.to_string(),
            truncated: false,
            original_chars,
        };
    }
    let prefix = trimmed.chars().take(limit).collect::<String>();
    OutputSlice {
        text: format!("{prefix}\n...[truncated]"),
        truncated: true,
        original_chars,
    }
}

fn render_output(
    command: &str,
    cwd: &std::path::Path,
    exit_code: i32,
    timeout_ms: u64,
    stdout: &OutputSlice,
    stderr: &OutputSlice,
) -> String {
    let mut sections = vec![
        format!(
            "[bash cwd={} exit_code={} timeout_ms={} mode=run]",
            cwd.display(),
            exit_code,
            timeout_ms
        ),
        format!("command> {command}"),
    ];
    if !stdout.text.is_empty() {
        sections.push(format!("stdout>\n{}", stdout.text));
    }
    if !stderr.text.is_empty() {
        sections.push(format!("stderr>\n{}", stderr.text));
    }
    if stdout.truncated || stderr.truncated {
        sections.push(format!(
            "[output truncated to {} chars per stream]",
            stdout
                .original_chars
                .max(stderr.original_chars)
                .min(MAX_ALLOWED_OUTPUT_CHARS)
        ));
    }
    sections.join("\n\n")
}

fn unix_timestamp_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{BashExecutionMode, BashTool, BashToolInput};
    use crate::{Tool, ToolExecutionContext};
    use std::collections::BTreeMap;
    use types::ToolCallId;

    #[tokio::test]
    async fn bash_tool_captures_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: None,
                    command: Some("printf hello".to_string()),
                    session_id: None,
                    cwd: None,
                    timeout_ms: Some(5_000),
                    poll_wait_ms: None,
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.text_content().contains("stdout>\nhello"));
    }

    #[tokio::test]
    async fn bash_tool_can_inject_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: None,
                    command: Some("printf %s \"$PATCH_ENV\"".to_string()),
                    session_id: None,
                    cwd: None,
                    timeout_ms: Some(5_000),
                    poll_wait_ms: None,
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: None,
                    env: Some(BTreeMap::from([(
                        "PATCH_ENV".to_string(),
                        "value".to_string(),
                    )])),
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.text_content().contains("stdout>\nvalue"));
    }

    #[tokio::test]
    async fn bash_tool_supports_background_start_poll_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new();
        let start = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: Some(BashExecutionMode::Start),
                    command: Some("printf begin; sleep 5; printf end".to_string()),
                    session_id: None,
                    cwd: None,
                    timeout_ms: Some(10_000),
                    poll_wait_ms: None,
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let session_id = start.metadata.unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let poll = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: Some(BashExecutionMode::Poll),
                    command: None,
                    session_id: Some(session_id.clone()),
                    cwd: None,
                    timeout_ms: None,
                    poll_wait_ms: Some(100),
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(poll.text_content().contains("state="));

        let cancel = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: Some(BashExecutionMode::Cancel),
                    command: None,
                    session_id: Some(session_id.clone()),
                    cwd: None,
                    timeout_ms: None,
                    poll_wait_ms: Some(2_000),
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(!cancel.is_error);
        assert!(cancel.text_content().contains("cancellation_requested>"));

        let final_poll = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: Some(BashExecutionMode::Poll),
                    command: None,
                    session_id: Some(session_id),
                    cwd: None,
                    timeout_ms: None,
                    poll_wait_ms: Some(1_000),
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(final_poll.text_content().contains("begin"));
        assert!(
            final_poll.text_content().contains("state=cancelled")
                || final_poll.text_content().contains("state=completed")
        );
    }

    #[tokio::test]
    async fn bash_tool_poll_supports_char_windows() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new();
        let start = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: Some(BashExecutionMode::Start),
                    command: Some("printf abcdef".to_string()),
                    session_id: None,
                    cwd: None,
                    timeout_ms: Some(5_000),
                    poll_wait_ms: None,
                    stdout_start_char: None,
                    stderr_start_char: None,
                    max_output_chars: Some(1024),
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let session_id = start.metadata.unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let poll = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    mode: Some(BashExecutionMode::Poll),
                    command: None,
                    session_id: Some(session_id),
                    cwd: None,
                    timeout_ms: None,
                    poll_wait_ms: Some(1_000),
                    stdout_start_char: Some(2),
                    stderr_start_char: Some(0),
                    max_output_chars: Some(2),
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(poll.text_content().contains("stdout>\ncd"));
        let metadata = poll.metadata.unwrap();
        assert_eq!(metadata["stdout"]["next_start_char"], 4);
    }
}
