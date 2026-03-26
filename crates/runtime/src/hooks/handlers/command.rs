use crate::{Result, RuntimeError};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use tools::{
    ExecRequest, ExecutionOrigin, HostProcessExecutor, ProcessExecutor, ProcessStdio, RuntimeScope,
    SandboxPolicy,
};
use types::{HookContext, HookOutput};

#[async_trait]
pub trait CommandHookExecutor: Send + Sync {
    async fn execute(&self, command: &str, context: HookContext) -> Result<HookOutput>;
}

#[derive(Clone)]
pub struct DefaultCommandHookExecutor {
    extra_env: BTreeMap<String, String>,
    process_executor: Arc<dyn ProcessExecutor>,
    sandbox_policy: SandboxPolicy,
}

impl fmt::Debug for DefaultCommandHookExecutor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultCommandHookExecutor")
            .field("extra_env_keys", &self.extra_env.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for DefaultCommandHookExecutor {
    fn default() -> Self {
        Self::new(BTreeMap::new())
    }
}

impl DefaultCommandHookExecutor {
    #[must_use]
    pub fn new(extra_env: BTreeMap<String, String>) -> Self {
        Self {
            extra_env,
            process_executor: Arc::new(HostProcessExecutor),
            sandbox_policy: SandboxPolicy::default(),
        }
    }

    #[must_use]
    pub fn with_process_executor(
        extra_env: BTreeMap<String, String>,
        process_executor: Arc<dyn ProcessExecutor>,
    ) -> Self {
        Self {
            extra_env,
            process_executor,
            sandbox_policy: SandboxPolicy::default(),
        }
    }
}

#[async_trait]
impl CommandHookExecutor for DefaultCommandHookExecutor {
    async fn execute(&self, command: &str, context: HookContext) -> Result<HookOutput> {
        let mut env = self.extra_env.clone();
        env.insert(
            "AGENT_CORE_HOOK_PAYLOAD".to_string(),
            serde_json::to_string(&context).unwrap_or_default(),
        );
        let mut process = self
            .process_executor
            .prepare(ExecRequest {
                program: "/bin/sh".to_string(),
                args: vec!["-lc".to_string(), command.to_string()],
                cwd: None,
                env,
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Piped,
                stderr: ProcessStdio::Piped,
                kill_on_drop: true,
                origin: ExecutionOrigin::HookCommand,
                runtime_scope: RuntimeScope {
                    run_id: Some(context.run_id.clone()),
                    session_id: Some(context.session_id.clone()),
                    turn_id: context.turn_id.clone(),
                    tool_name: None,
                    tool_call_id: None,
                },
                sandbox_policy: self.sandbox_policy.clone(),
            })
            .map_err(|error| RuntimeError::hook(error.to_string()))?;
        let output = process.output().await?;
        if !output.status.success() {
            return Err(RuntimeError::hook(format!(
                "hook command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Ok(HookOutput::default());
        }
        match serde_json::from_str::<HookOutput>(&stdout) {
            Ok(value) => Ok(value),
            Err(_) => Ok(HookOutput {
                system_message: Some(stdout),
                ..HookOutput::default()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandHookExecutor, DefaultCommandHookExecutor};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::process::Command;
    use tools::{
        ExecRequest, HostProcessExecutor, ProcessExecutor, Result as ToolResult,
        RuntimeScope as ExecRuntimeScope,
    };
    use types::{HookContext, HookEvent, RunId, SessionId};

    #[derive(Clone)]
    struct RecordingExecutor {
        inner: Arc<dyn ProcessExecutor>,
        requests: Arc<Mutex<Vec<ExecRequest>>>,
    }

    impl ProcessExecutor for RecordingExecutor {
        fn prepare(&self, request: ExecRequest) -> ToolResult<Command> {
            self.requests.lock().unwrap().push(request.clone());
            self.inner.prepare(request)
        }
    }

    #[tokio::test]
    async fn command_hook_executor_routes_process_launch_through_executor() {
        let run_id = RunId::from("run_1");
        let session_id = SessionId::from("session_1");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let process_executor = Arc::new(RecordingExecutor {
            inner: Arc::new(HostProcessExecutor),
            requests: requests.clone(),
        });
        let executor = DefaultCommandHookExecutor::with_process_executor(
            BTreeMap::from([("HOOK_FLAG".to_string(), "flag".to_string())]),
            process_executor,
        );

        let output = executor
            .execute(
                "printf '{\"system_message\":\"ok\"}'",
                HookContext {
                    event: HookEvent::Notification,
                    run_id: run_id.clone(),
                    session_id: session_id.clone(),
                    turn_id: None,
                    fields: BTreeMap::new(),
                    payload: serde_json::json!({"hello":"world"}),
                },
            )
            .await
            .unwrap();

        assert_eq!(output.system_message.as_deref(), Some("ok"));
        let logged = requests.lock().unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].program, "/bin/sh");
        assert_eq!(
            logged[0].args,
            vec!["-lc", "printf '{\"system_message\":\"ok\"}'"]
        );
        assert!(logged[0].env.contains_key("AGENT_CORE_HOOK_PAYLOAD"));
        assert_eq!(logged[0].runtime_scope.run_id, Some(run_id.clone()));
        assert_eq!(
            logged[0].runtime_scope,
            ExecRuntimeScope {
                run_id: Some(run_id),
                session_id: Some(session_id),
                turn_id: None,
                tool_name: None,
                tool_call_id: None,
            }
        );
    }
}
