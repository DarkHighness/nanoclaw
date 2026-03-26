use crate::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use types::{CallId, RunId, SessionId, TurnId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    Off,
    AllowDomains(Vec<String>),
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEscapePolicy {
    Deny,
    HostManaged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub filesystem: FilesystemPolicy,
    pub network: NetworkPolicy,
    pub host_escape: HostEscapePolicy,
    pub fail_if_unavailable: bool,
}

impl SandboxPolicy {
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            mode: SandboxMode::DangerFullAccess,
            filesystem: FilesystemPolicy::default(),
            network: NetworkPolicy::Full,
            host_escape: HostEscapePolicy::HostManaged,
            fail_if_unavailable: false,
        }
    }
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        // The current substrate still launches local processes on the host by
        // default. Keeping that posture explicit avoids pretending there is a
        // hard boundary before an enforcing backend is wired in.
        Self::permissive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOrigin {
    BashTool,
    HookCommand,
    McpStdioServer { server_name: String },
    HostUtility { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RuntimeScope {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<CallId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStdio {
    Inherit,
    Null,
    Piped,
}

impl ProcessStdio {
    fn into_stdio(self) -> Stdio {
        match self {
            Self::Inherit => Stdio::inherit(),
            Self::Null => Stdio::null(),
            Self::Piped => Stdio::piped(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub stdin: ProcessStdio,
    pub stdout: ProcessStdio,
    pub stderr: ProcessStdio,
    pub kill_on_drop: bool,
    pub origin: ExecutionOrigin,
    pub runtime_scope: RuntimeScope,
    pub sandbox_policy: SandboxPolicy,
}

pub trait ProcessExecutor: Send + Sync {
    fn prepare(&self, request: ExecRequest) -> Result<Command>;
}

#[derive(Default)]
pub struct HostProcessExecutor;

impl ProcessExecutor for HostProcessExecutor {
    fn prepare(&self, request: ExecRequest) -> Result<Command> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(request.stdin.into_stdio())
            .stdout(request.stdout.into_stdio())
            .stderr(request.stderr.into_stdio())
            .kill_on_drop(request.kill_on_drop);
        if let Some(cwd) = request.cwd {
            command.current_dir(cwd);
        }
        if !request.env.is_empty() {
            command.envs(request.env);
        }

        // Phase 1 centralizes child-process construction under one boundary
        // without changing behavior. Enforcing backends will interpret
        // `sandbox_policy` here later instead of at every call site.
        let _ = request.origin;
        let _ = request.runtime_scope;
        let _ = request.sandbox_policy;

        Ok(command)
    }
}
