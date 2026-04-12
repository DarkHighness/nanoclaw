use crate::HOST_FEATURE_HOST_PROCESS_SURFACES;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::process::{
    ExecRequest, ExecutionOrigin, HostProcessExecutor, ProcessExecutor, ProcessStdio, RuntimeScope,
    SandboxPolicy, sandbox_backend_status,
};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use agent_env::shell_or_default;
use async_trait::async_trait;
use dashmap::DashMap;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::debug;
use types::{
    MessagePart, ToolAvailability, ToolCallId, ToolOutputMode, ToolResult, ToolSpec, new_opaque_id,
};

const DEFAULT_YIELD_TIME_MS: u64 = 1_000;
const MAX_YIELD_TIME_MS: u64 = 30_000;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 32 * 1024;
const MAX_ALLOWED_OUTPUT_CHARS: usize = 256 * 1024;
const MAX_TRACKED_EXEC_SESSIONS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ExecSessionId(String);

impl ExecSessionId {
    fn new() -> Self {
        Self(format!("exec-{}", new_opaque_id()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExecSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for ExecSessionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ExecCommandToolInput {
    pub cmd: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub login: Option<bool>,
    #[serde(default)]
    pub tty: Option<bool>,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_chars: Option<usize>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WriteStdinToolInput {
    pub session_id: String,
    #[serde(default)]
    pub chars: Option<String>,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_chars: Option<usize>,
    #[serde(default)]
    pub close_stdin: Option<bool>,
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

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ExecOutputWindowResult {
    text: String,
    start_char: usize,
    end_char: usize,
    total_chars: usize,
    truncated: bool,
    remaining_chars: usize,
    next_start_char: Option<usize>,
}

impl From<&OutputWindow> for ExecOutputWindowResult {
    fn from(value: &OutputWindow) -> Self {
        Self {
            text: value.text.clone(),
            start_char: value.start_char,
            end_char: value.end_char,
            total_chars: value.total_chars,
            truncated: value.truncated,
            remaining_chars: value.remaining_chars,
            next_start_char: value.next_start_char,
        }
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ExecSessionOutput {
    session_id: String,
    state: String,
    command: String,
    cwd: String,
    shell: String,
    login: bool,
    tty: bool,
    stdin_open: bool,
    wrote_chars: usize,
    closed_stdin: bool,
    yield_time_ms: u64,
    max_output_chars: usize,
    started_at_unix_s: u64,
    finished_at_unix_s: Option<u64>,
    exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    stdout: ExecOutputWindowResult,
    stderr: ExecOutputWindowResult,
}

#[derive(Clone, Debug)]
enum SessionStatus {
    Running,
    Completed {
        exit_code: i32,
        error: Option<String>,
        finished_at_unix_s: u64,
    },
}

impl SessionStatus {
    fn completed(exit_code: i32, error: Option<String>) -> Self {
        Self::Completed {
            exit_code,
            error,
            finished_at_unix_s: unix_timestamp_s(),
        }
    }

    fn state_label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed { error: Some(_), .. } => "failed",
            Self::Completed { .. } => "completed",
        }
    }
}

#[derive(Clone, Debug)]
struct SessionStatusSnapshot {
    state: &'static str,
    exit_code: Option<i32>,
    error: Option<String>,
    finished_at_unix_s: Option<u64>,
}

#[derive(Debug)]
struct ExecSession {
    id: ExecSessionId,
    command: String,
    cwd: PathBuf,
    shell: String,
    login: bool,
    tty: bool,
    started_at_unix_s: u64,
    stdin: AsyncMutex<Option<ChildStdin>>,
    stdin_open: Arc<AtomicBool>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_cursor: Mutex<usize>,
    stderr_cursor: Mutex<usize>,
    status: Arc<Mutex<SessionStatus>>,
    completion: Arc<Notify>,
    output_notify: Arc<Notify>,
}

impl ExecSession {
    fn snapshot_status(&self) -> SessionStatusSnapshot {
        let status = self.status.lock().expect("exec session status lock");
        match &*status {
            SessionStatus::Running => SessionStatusSnapshot {
                state: "running",
                exit_code: None,
                error: None,
                finished_at_unix_s: None,
            },
            SessionStatus::Completed {
                exit_code,
                error,
                finished_at_unix_s,
            } => SessionStatusSnapshot {
                state: status.state_label(),
                exit_code: Some(*exit_code),
                error: error.clone(),
                finished_at_unix_s: Some(*finished_at_unix_s),
            },
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            *self.status.lock().expect("exec session status lock"),
            SessionStatus::Running
        )
    }

    fn completed_timestamp(&self) -> Option<u64> {
        match &*self.status.lock().expect("exec session status lock") {
            SessionStatus::Completed {
                finished_at_unix_s, ..
            } => Some(*finished_at_unix_s),
            SessionStatus::Running => None,
        }
    }

    async fn wait_for_activity(&self, yield_time_ms: u64) {
        if yield_time_ms == 0 {
            return;
        }
        tokio::select! {
            _ = self.output_notify.notified() => {}
            _ = self.completion.notified() => {}
            _ = sleep(Duration::from_millis(yield_time_ms)) => {}
        }
    }

    async fn write_input(&self, chars: &str) -> Result<usize> {
        if chars.is_empty() {
            return Ok(0);
        }
        if !self.is_running() {
            return Err(ToolError::invalid_state(format!(
                "exec session `{}` has already exited",
                self.id
            )));
        }
        let mut stdin_guard = self.stdin.lock().await;
        let Some(stdin) = stdin_guard.as_mut() else {
            return Err(ToolError::invalid_state(format!(
                "exec session `{}` stdin is already closed",
                self.id
            )));
        };
        if let Err(error) = stdin.write_all(chars.as_bytes()).await {
            self.stdin_open.store(false, Ordering::SeqCst);
            stdin_guard.take();
            return Err(exec_stdin_error(self, "write", error.kind(), error));
        }
        if let Err(error) = stdin.flush().await {
            self.stdin_open.store(false, Ordering::SeqCst);
            stdin_guard.take();
            return Err(exec_stdin_error(self, "flush", error.kind(), error));
        }
        Ok(chars.chars().count())
    }

    async fn close_stdin(&self) -> bool {
        let mut stdin = self.stdin.lock().await;
        let closed = stdin.take().is_some();
        self.stdin_open.store(false, Ordering::SeqCst);
        closed
    }

    fn output_windows(&self, max_output_chars: usize) -> (OutputWindow, OutputWindow) {
        let stdout = self.stdout.lock().expect("exec stdout lock");
        let stderr = self.stderr.lock().expect("exec stderr lock");
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);

        // Sessions advance their own delivery cursors so follow-up polling only
        // needs the session id. That keeps the public tool surface aligned with
        // Codex-style stdin continuation instead of exposing transport offsets.
        let mut stdout_cursor = self.stdout_cursor.lock().expect("exec stdout cursor lock");
        let mut stderr_cursor = self.stderr_cursor.lock().expect("exec stderr cursor lock");
        let stdout_window = slice_output_window(stdout.as_ref(), *stdout_cursor, max_output_chars);
        let stderr_window = slice_output_window(stderr.as_ref(), *stderr_cursor, max_output_chars);
        *stdout_cursor = stdout_window.end_char;
        *stderr_cursor = stderr_window.end_char;
        (stdout_window, stderr_window)
    }
}

type SessionRegistry = DashMap<ExecSessionId, Arc<ExecSession>>;

static EXEC_SESSIONS: OnceLock<SessionRegistry> = OnceLock::new();

fn exec_sessions() -> &'static SessionRegistry {
    EXEC_SESSIONS.get_or_init(SessionRegistry::new)
}

fn get_session(session_id: &ExecSessionId) -> Option<Arc<ExecSession>> {
    exec_sessions()
        .get(session_id)
        .map(|entry| Arc::clone(entry.value()))
}

fn insert_session(session: Arc<ExecSession>) {
    let registry = exec_sessions();
    // Bound retained sessions so long-lived hosts do not accumulate completed
    // interactive processes after the model has already moved on.
    prune_completed_sessions(registry);
    registry.insert(session.id.clone(), session);
}

fn prune_completed_sessions(registry: &SessionRegistry) {
    if registry.len() < MAX_TRACKED_EXEC_SESSIONS {
        return;
    }

    let mut completed = registry
        .iter()
        .filter_map(|entry| {
            entry
                .value()
                .completed_timestamp()
                .map(|finished_at| (entry.key().clone(), finished_at))
        })
        .collect::<Vec<_>>();
    completed.sort_by_key(|(_, finished_at)| *finished_at);

    let remove_count = registry.len().saturating_sub(MAX_TRACKED_EXEC_SESSIONS) + 1;
    for (session_id, _) in completed.into_iter().take(remove_count) {
        registry.remove(&session_id);
    }
}

#[derive(Clone)]
pub struct ExecCommandTool {
    process_executor: Arc<dyn ProcessExecutor>,
    sandbox_policy: SandboxPolicy,
}

impl fmt::Debug for ExecCommandTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecCommandTool").finish_non_exhaustive()
    }
}

impl Default for ExecCommandTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecCommandTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            process_executor: Arc::new(HostProcessExecutor),
            sandbox_policy: SandboxPolicy::default(),
        }
    }

    #[must_use]
    pub fn with_process_executor(process_executor: Arc<dyn ProcessExecutor>) -> Self {
        Self {
            process_executor,
            sandbox_policy: SandboxPolicy::default(),
        }
    }

    #[must_use]
    pub fn with_process_executor_and_policy(
        process_executor: Arc<dyn ProcessExecutor>,
        sandbox_policy: SandboxPolicy,
    ) -> Self {
        Self {
            process_executor,
            sandbox_policy,
        }
    }

    fn effective_sandbox_policy(&self, ctx: &ToolExecutionContext) -> SandboxPolicy {
        ctx.effective_sandbox_policy
            .clone()
            .unwrap_or_else(|| self.sandbox_policy.clone())
    }

    fn ensure_effective_policy_supported(
        &self,
        ctx: &ToolExecutionContext,
    ) -> Result<SandboxPolicy> {
        let policy = self.effective_sandbox_policy(ctx);
        let status = sandbox_backend_status(&policy);
        if policy.requires_enforcement() && !status.is_available() {
            let reason = status
                .reason()
                .unwrap_or("no compatible sandbox backend is available");
            return Err(ToolError::invalid_state(format!(
                "exec_command is unavailable while the current sandbox mode requires enforcement, but {reason}. Switch /permissions to danger-full-access or enable a supported sandbox backend."
            )));
        }
        Ok(policy)
    }
}

#[derive(Clone, Debug, Default)]
pub struct WriteStdinTool;

impl WriteStdinTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ExecCommandTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "exec_command",
            "Run a shell command in an interactive exec session. Returns incremental stdout/stderr and a session id that can be continued with write_stdin.",
            serde_json::to_value(schema_for!(ExecCommandToolInput))
                .expect("exec_command schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(ExecSessionOutput))
                .expect("exec_command output schema"),
        )
        .with_availability(ToolAvailability {
            feature_flags: vec![HOST_FEATURE_HOST_PROCESS_SURFACES.to_string()],
            ..ToolAvailability::default()
        })
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: ExecCommandToolInput = serde_json::from_value(arguments)?;
        execute_exec_command(self, call_id, input, ctx).await
    }
}

#[async_trait]
impl Tool for WriteStdinTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "write_stdin",
            "Write characters to an existing exec session stdin, or poll for more output by omitting chars. Set close_stdin=true to send EOF after any write.",
            serde_json::to_value(schema_for!(WriteStdinToolInput))
                .expect("write_stdin schema"),
            ToolOutputMode::Text,
            // Harmfulness belongs to the original exec_command. Continuing an
            // existing session should not open a second approval gate on the
            // follow-up stdin payload itself.
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(ExecSessionOutput))
                .expect("write_stdin output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: WriteStdinToolInput = serde_json::from_value(arguments)?;
        execute_write_stdin(call_id, input).await
    }
}

async fn execute_exec_command(
    tool: &ExecCommandTool,
    call_id: ToolCallId,
    input: ExecCommandToolInput,
    ctx: &ToolExecutionContext,
) -> Result<ToolResult> {
    let external_call_id = types::CallId::from(&call_id);
    if input.tty.unwrap_or(false) {
        return Ok(ToolResult::error(
            call_id,
            "exec_command",
            "tty=true is not supported by the current local runtime",
        )
        .with_call_id(external_call_id));
    }

    let command = resolve_exec_command(&input)?;
    let cwd = resolve_exec_cwd(input.workdir.as_deref(), ctx)?;
    let shell = input
        .shell
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| shell_or_default("/bin/sh"));
    let login = input.login.unwrap_or(true);
    let yield_time_ms = input
        .yield_time_ms
        .unwrap_or(DEFAULT_YIELD_TIME_MS)
        .min(MAX_YIELD_TIME_MS);
    let max_output_chars = input
        .max_output_chars
        .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS)
        .clamp(1, MAX_ALLOWED_OUTPUT_CHARS);
    let sandbox_policy = tool.ensure_effective_policy_supported(ctx)?;
    let env = input.env.unwrap_or_default();

    let mut child = tool
        .process_executor
        .prepare(ExecRequest {
            program: shell.clone(),
            args: exec_args(login, &command),
            cwd: Some(cwd.clone()),
            env,
            stdin: ProcessStdio::Piped,
            stdout: ProcessStdio::Piped,
            stderr: ProcessStdio::Piped,
            kill_on_drop: true,
            origin: ExecutionOrigin::HostUtility {
                name: "exec_command".to_string(),
            },
            runtime_scope: runtime_scope_from_context(ctx),
            sandbox_policy,
        })?
        .spawn()
        .map_err(|error| {
            ToolError::invalid_state(format!("failed to start exec session: {error}"))
        })?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ToolError::invalid_state("exec session did not expose stdin".to_string()))?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ToolError::invalid_state("exec session did not expose stdout".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ToolError::invalid_state("exec session did not expose stderr".to_string())
    })?;

    let session_id = ExecSessionId::new();
    let status = Arc::new(Mutex::new(SessionStatus::Running));
    let completion = Arc::new(Notify::new());
    let output_notify = Arc::new(Notify::new());
    let session = Arc::new(ExecSession {
        id: session_id.clone(),
        command: command.clone(),
        cwd: cwd.clone(),
        shell: shell.clone(),
        login,
        tty: false,
        started_at_unix_s: unix_timestamp_s(),
        stdin: AsyncMutex::new(Some(stdin)),
        stdin_open: Arc::new(AtomicBool::new(true)),
        stdout: Arc::new(Mutex::new(Vec::new())),
        stderr: Arc::new(Mutex::new(Vec::new())),
        stdout_cursor: Mutex::new(0),
        stderr_cursor: Mutex::new(0),
        status: status.clone(),
        completion: completion.clone(),
        output_notify: output_notify.clone(),
    });

    insert_session(session.clone());
    debug!(
        session_id = %session_id,
        cwd = %cwd.display(),
        login,
        "started exec session"
    );

    tokio::spawn(run_exec_session(
        child,
        stdout,
        stderr,
        Arc::clone(&session.stdout),
        Arc::clone(&session.stderr),
        status,
        completion,
        output_notify,
        Arc::clone(&session.stdin_open),
    ));

    session.wait_for_activity(yield_time_ms).await;
    let result = build_session_result(
        call_id,
        "exec_command",
        &session,
        yield_time_ms,
        max_output_chars,
        0,
        false,
    );
    Ok(result.with_call_id(external_call_id))
}

async fn execute_write_stdin(
    call_id: ToolCallId,
    input: WriteStdinToolInput,
) -> Result<ToolResult> {
    let external_call_id = types::CallId::from(&call_id);
    let session_id = resolve_exec_session_id(&input.session_id)?;
    let session = match get_session(&session_id) {
        Some(session) => session,
        None => {
            return Ok(ToolResult::error(
                call_id,
                "write_stdin",
                format!("Unknown exec session_id `{session_id}`"),
            )
            .with_call_id(external_call_id));
        }
    };
    let yield_time_ms = input
        .yield_time_ms
        .unwrap_or(DEFAULT_YIELD_TIME_MS)
        .min(MAX_YIELD_TIME_MS);
    let max_output_chars = input
        .max_output_chars
        .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS)
        .clamp(1, MAX_ALLOWED_OUTPUT_CHARS);
    let chars = input.chars.unwrap_or_default();
    let wrote_chars = session.write_input(&chars).await?;
    let closed_stdin = if input.close_stdin.unwrap_or(false) {
        session.close_stdin().await
    } else {
        false
    };

    session.wait_for_activity(yield_time_ms).await;
    let result = build_session_result(
        call_id,
        "write_stdin",
        &session,
        yield_time_ms,
        max_output_chars,
        wrote_chars,
        closed_stdin,
    );
    Ok(result.with_call_id(external_call_id))
}

fn build_session_result(
    call_id: ToolCallId,
    tool_name: &str,
    session: &ExecSession,
    yield_time_ms: u64,
    max_output_chars: usize,
    wrote_chars: usize,
    closed_stdin: bool,
) -> ToolResult {
    let status = session.snapshot_status();
    let stdin_open = session.stdin_open.load(Ordering::SeqCst);
    let (stdout, stderr) = session.output_windows(max_output_chars);
    let structured = ExecSessionOutput {
        session_id: session.id.as_str().to_string(),
        state: status.state.to_string(),
        command: session.command.clone(),
        cwd: session.cwd.display().to_string(),
        shell: session.shell.clone(),
        login: session.login,
        tty: session.tty,
        stdin_open,
        wrote_chars,
        closed_stdin,
        yield_time_ms,
        max_output_chars,
        started_at_unix_s: session.started_at_unix_s,
        finished_at_unix_s: status.finished_at_unix_s,
        exit_code: status.exit_code,
        error: status.error.clone(),
        stdout: ExecOutputWindowResult::from(&stdout),
        stderr: ExecOutputWindowResult::from(&stderr),
    };
    let metadata = serde_json::json!({
        "session_id": session.id.as_str(),
        "state": status.state,
        "command": session.command,
        "cwd": session.cwd,
        "shell": session.shell,
        "login": session.login,
        "tty": session.tty,
        "stdin_open": stdin_open,
        "wrote_chars": wrote_chars,
        "closed_stdin": closed_stdin,
        "yield_time_ms": yield_time_ms,
        "max_output_chars": max_output_chars,
        "started_at_unix_s": session.started_at_unix_s,
        "finished_at_unix_s": status.finished_at_unix_s,
        "exit_code": status.exit_code,
        "error": status.error,
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
        }
    });

    ToolResult {
        id: call_id,
        call_id: types::CallId::new(),
        tool_name: tool_name.into(),
        parts: vec![MessagePart::text(render_session_output(
            tool_name,
            session,
            &status,
            &stdout,
            &stderr,
            wrote_chars,
            closed_stdin,
        ))],
        attachments: Vec::new(),
        structured_content: Some(
            serde_json::to_value(structured).expect("exec session structured output"),
        ),
        continuation: None,
        metadata: Some(metadata),
        is_error: status.error.is_some() || matches!(status.exit_code, Some(code) if code != 0),
    }
}

async fn run_exec_session(
    mut child: Child,
    stdout_pipe: ChildStdout,
    stderr_pipe: ChildStderr,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    status: Arc<Mutex<SessionStatus>>,
    completion: Arc<Notify>,
    output_notify: Arc<Notify>,
    stdin_open: Arc<AtomicBool>,
) {
    let stdout_task = spawn_capture_task(stdout_pipe, stdout, Arc::clone(&output_notify));
    let stderr_task = spawn_capture_task(stderr_pipe, stderr, Arc::clone(&output_notify));

    let next_status = match child.wait().await {
        Ok(wait_status) => SessionStatus::completed(wait_status.code().unwrap_or(-1), None),
        Err(error) => SessionStatus::completed(-1, Some(error.to_string())),
    };

    stdin_open.store(false, Ordering::SeqCst);
    *status.lock().expect("exec session status lock") = next_status;
    completion.notify_waiters();

    let _ = stdout_task.await;
    let _ = stderr_task.await;
}

fn spawn_capture_task<R>(
    mut stream: R,
    buffer: Arc<Mutex<Vec<u8>>>,
    output_notify: Arc<Notify>,
) -> JoinHandle<()>
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
                        .expect("exec output buffer lock")
                        .extend_from_slice(&chunk[..read]);
                    output_notify.notify_waiters();
                }
                Err(_) => break,
            }
        }
    })
}

fn resolve_exec_command(input: &ExecCommandToolInput) -> Result<String> {
    resolve_shell_command(&input.cmd, "exec_command")
}

fn resolve_exec_session_id(value: &str) -> Result<ExecSessionId> {
    let session_id = value.trim();
    if session_id.is_empty() {
        Err(ToolError::invalid(
            "write_stdin requires a non-empty `session_id`",
        ))
    } else {
        Ok(ExecSessionId::from(session_id))
    }
}

pub(crate) fn resolve_exec_cwd(
    workdir: Option<&str>,
    ctx: &ToolExecutionContext,
) -> Result<PathBuf> {
    let cwd = resolve_tool_path_against_workspace_root(
        workdir.unwrap_or("."),
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_read_allowed(&cwd)?;
    Ok(cwd)
}

pub(crate) fn runtime_scope_from_context(ctx: &ToolExecutionContext) -> RuntimeScope {
    RuntimeScope {
        session_id: ctx.session_id.clone(),
        agent_session_id: ctx.agent_session_id.clone(),
        turn_id: ctx.turn_id.clone(),
        tool_name: ctx.tool_name.clone().map(|name| name.to_string()),
        tool_call_id: ctx.tool_call_id.clone(),
    }
}

pub(crate) fn exec_args(login: bool, command: &str) -> Vec<String> {
    if login {
        vec!["-lc".to_string(), command.to_string()]
    } else {
        vec!["-c".to_string(), command.to_string()]
    }
}

fn exec_stdin_error(
    session: &ExecSession,
    action: &str,
    kind: ErrorKind,
    error: std::io::Error,
) -> ToolError {
    if kind == ErrorKind::BrokenPipe || !session.is_running() {
        ToolError::invalid_state(format!("exec session `{}` has already exited", session.id))
    } else {
        ToolError::invalid_state(format!(
            "failed to {action} stdin for exec session `{}`: {error}",
            session.id
        ))
    }
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

fn truncate_text_with_limit(value: &str, limit: usize) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= limit {
        return (value.to_string(), false);
    }
    (value.chars().take(limit).collect::<String>(), true)
}

fn render_session_output(
    tool_name: &str,
    session: &ExecSession,
    status: &SessionStatusSnapshot,
    stdout: &OutputWindow,
    stderr: &OutputWindow,
    wrote_chars: usize,
    closed_stdin: bool,
) -> String {
    let mut sections = vec![
        format!(
            "[{tool_name} session_id={} state={}]",
            session.id, status.state
        ),
        format!("command> {}", session.command),
        format!("cwd> {}", session.cwd.display()),
    ];
    if wrote_chars > 0 {
        sections.push(format!("stdin_chars> {wrote_chars}"));
    }
    if closed_stdin {
        sections.push("stdin_closed> true".to_string());
    }
    if let Some(exit_code) = status.exit_code {
        sections.push(format!("exit_code> {exit_code}"));
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
    sections.join("\n\n")
}

pub(crate) fn unix_timestamp_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn resolve_shell_command(command: &str, tool_name: &str) -> Result<String> {
    let command = command.trim();
    if command.is_empty() {
        Err(ToolError::invalid(format!(
            "{tool_name} requires a non-empty `cmd`"
        )))
    } else {
        Ok(command.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecCommandTool, ExecCommandToolInput, WriteStdinTool, WriteStdinToolInput};
    use crate::{ExecRequest, HostProcessExecutor, ProcessExecutor, Tool, ToolExecutionContext};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tokio::process::Command;
    use types::{ToolCallId, ToolResult};

    #[derive(Clone)]
    struct RecordingExecutor {
        inner: Arc<dyn ProcessExecutor>,
        requests: Arc<Mutex<Vec<ExecRequest>>>,
    }

    impl ProcessExecutor for RecordingExecutor {
        fn prepare(&self, request: ExecRequest) -> sandbox::Result<Command> {
            self.requests.lock().unwrap().push(request.clone());
            self.inner.prepare(request)
        }
    }

    fn session_id_from(result: &ToolResult) -> String {
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("session_id"))
            .and_then(Value::as_str)
            .expect("exec output should include session_id")
            .to_string()
    }

    #[tokio::test]
    async fn exec_command_routes_shell_launch_through_process_executor() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let tool = ExecCommandTool::with_process_executor(Arc::new(RecordingExecutor {
            inner: Arc::new(HostProcessExecutor),
            requests: requests.clone(),
        }));

        let _ = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ExecCommandToolInput {
                    cmd: "printf ready".to_string(),
                    workdir: None,
                    shell: Some("/bin/sh".to_string()),
                    login: Some(false),
                    tty: Some(false),
                    yield_time_ms: Some(10),
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let logged = requests.lock().unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].stdin, crate::ProcessStdio::Piped);
        assert_eq!(logged[0].stdout, crate::ProcessStdio::Piped);
        assert_eq!(logged[0].stderr, crate::ProcessStdio::Piped);
        assert_eq!(
            logged[0].args,
            vec!["-c".to_string(), "printf ready".to_string()]
        );
    }

    #[tokio::test]
    async fn exec_command_supports_interactive_stdin_sessions() {
        let exec = ExecCommandTool::new();
        let write = WriteStdinTool::new();

        let started = exec
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ExecCommandToolInput {
                    cmd: "cat".to_string(),
                    workdir: None,
                    shell: Some("/bin/sh".to_string()),
                    login: Some(false),
                    tty: Some(false),
                    yield_time_ms: Some(20),
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let session_id = session_id_from(&started);

        let echoed = write
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WriteStdinToolInput {
                    session_id: session_id.clone(),
                    chars: Some("hello\n".to_string()),
                    yield_time_ms: Some(200),
                    max_output_chars: None,
                    close_stdin: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let echoed_stdout = echoed
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/stdout/text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(echoed_stdout.contains("hello"));

        let closed = write
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WriteStdinToolInput {
                    session_id,
                    chars: None,
                    yield_time_ms: Some(200),
                    max_output_chars: None,
                    close_stdin: Some(true),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            closed
                .structured_content
                .as_ref()
                .and_then(|value| value.get("state"))
                .and_then(Value::as_str),
            Some("completed")
        );
    }

    #[tokio::test]
    async fn write_stdin_rejects_input_after_process_exit() {
        let exec = ExecCommandTool::new();
        let write = WriteStdinTool::new();

        let finished = exec
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ExecCommandToolInput {
                    cmd: "printf done".to_string(),
                    workdir: None,
                    shell: Some("/bin/sh".to_string()),
                    login: Some(false),
                    tty: Some(false),
                    yield_time_ms: Some(200),
                    max_output_chars: None,
                    env: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let session_id = session_id_from(&finished);

        let error = write
            .execute(
                ToolCallId::new(),
                serde_json::to_value(WriteStdinToolInput {
                    session_id,
                    chars: Some("more".to_string()),
                    yield_time_ms: None,
                    max_output_chars: None,
                    close_stdin: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("completed process should reject more stdin");
        assert!(error.to_string().contains("already exited"));
    }

    #[test]
    fn write_stdin_spec_does_not_request_follow_up_approval() {
        let spec = WriteStdinTool::new().spec();
        assert!(!spec.approval.mutates_state);
        assert!(!spec.approval.open_world);
        assert_eq!(spec.approval.idempotent, Some(false));
    }
}
