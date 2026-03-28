use super::execution::{
    HookAuditAction, HookExecutionObserver, TracingHookExecutionObserver, authorize_execute_path,
    record_completion, record_failure, tighten_hook_sandbox_policy,
};
use crate::{Result, RuntimeError};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tools::{
    ExecRequest, ExecutionOrigin, FilesystemPolicy, HostEscapePolicy, ManagedPolicyProcessExecutor,
    ProcessExecutor, ProcessStdio, RuntimeScope, SandboxMode, SandboxPolicy,
};
use types::{HookContext, HookHandler, HookRegistration, HookResult};

#[async_trait]
pub trait CommandHookExecutor: Send + Sync {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult>;
}

#[derive(Clone)]
pub struct DefaultCommandHookExecutor {
    extra_env: BTreeMap<String, String>,
    process_executor: Arc<dyn ProcessExecutor>,
    sandbox_policy: SandboxPolicy,
    observer: Arc<dyn HookExecutionObserver>,
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
            process_executor: Arc::new(ManagedPolicyProcessExecutor::new()),
            sandbox_policy: default_hook_command_sandbox_policy(),
            observer: Arc::new(TracingHookExecutionObserver),
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
            sandbox_policy: default_hook_command_sandbox_policy(),
            observer: Arc::new(TracingHookExecutionObserver),
        }
    }

    #[must_use]
    pub fn with_process_executor_and_policy(
        extra_env: BTreeMap<String, String>,
        process_executor: Arc<dyn ProcessExecutor>,
        sandbox_policy: SandboxPolicy,
    ) -> Self {
        Self {
            extra_env,
            process_executor,
            sandbox_policy,
            observer: Arc::new(TracingHookExecutionObserver),
        }
    }

    #[cfg(test)]
    fn with_process_executor_policy_and_observer(
        extra_env: BTreeMap<String, String>,
        process_executor: Arc<dyn ProcessExecutor>,
        sandbox_policy: SandboxPolicy,
        observer: Arc<dyn HookExecutionObserver>,
    ) -> Self {
        Self {
            extra_env,
            process_executor,
            sandbox_policy,
            observer,
        }
    }
}

#[async_trait]
impl CommandHookExecutor for DefaultCommandHookExecutor {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult> {
        let HookHandler::Command(command) = &registration.handler else {
            return Err(RuntimeError::hook(format!(
                "hook `{}` is not a command hook",
                registration.name
            )));
        };
        let command_path = PathBuf::from(&command.command);
        let authorized = authorize_execute_path(
            registration,
            "command",
            &command_path,
            self.observer.as_ref(),
        )?;

        let mut env = self.extra_env.clone();
        env.insert(
            "NANOCLAW_CORE_HOOK_PAYLOAD".to_string(),
            serde_json::to_string(&context).unwrap_or_default(),
        );
        let mut process = self
            .process_executor
            .prepare(ExecRequest {
                program: command.command.clone(),
                args: Vec::new(),
                cwd: command_path.parent().map(Path::to_path_buf),
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
                sandbox_policy: tighten_hook_sandbox_policy(
                    &self.sandbox_policy,
                    &authorized.tool_context,
                ),
            })
            .map_err(|error| {
                let error = RuntimeError::hook(error.to_string());
                record_failure(
                    self.observer.as_ref(),
                    registration,
                    "command",
                    HookAuditAction::ExecutePath,
                    command_path.display().to_string(),
                    &error,
                );
                error
            })?;
        let output = process.output().await.map_err(|error| {
            let error = RuntimeError::hook(error.to_string());
            record_failure(
                self.observer.as_ref(),
                registration,
                "command",
                HookAuditAction::ExecutePath,
                command_path.display().to_string(),
                &error,
            );
            error
        })?;
        if !output.status.success() {
            let error = RuntimeError::hook(format!(
                "hook command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
            record_failure(
                self.observer.as_ref(),
                registration,
                "command",
                HookAuditAction::ExecutePath,
                command_path.display().to_string(),
                &error,
            );
            return Err(error);
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            record_completion(
                self.observer.as_ref(),
                registration,
                "command",
                HookAuditAction::ExecutePath,
                command_path.display().to_string(),
            );
            return Ok(HookResult::default());
        }
        let result = match serde_json::from_str::<HookResult>(&stdout) {
            Ok(value) => value,
            Err(_) => HookResult {
                effects: vec![types::HookEffect::AddContext { text: stdout }],
            },
        };
        record_completion(
            self.observer.as_ref(),
            registration,
            "command",
            HookAuditAction::ExecutePath,
            command_path.display().to_string(),
        );
        Ok(result)
    }
}

fn default_hook_command_sandbox_policy() -> SandboxPolicy {
    SandboxPolicy {
        mode: SandboxMode::WorkspaceWrite,
        filesystem: FilesystemPolicy::default(),
        network: tools::NetworkPolicy::Off,
        host_escape: HostEscapePolicy::Deny,
        fail_if_unavailable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandHookExecutor, DefaultCommandHookExecutor};
    use crate::hooks::handlers::execution::{
        HookAuditAction, HookAuditEvent, HookAuditOutcome, HookExecutionObserver,
    };
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::process::Command;
    use tools::{
        ExecRequest, HostProcessExecutor, NetworkPolicy, ProcessExecutor,
        RuntimeScope as ExecRuntimeScope, SandboxError, SandboxMode,
    };
    use types::{
        HookContext, HookEffect, HookEvent, HookExecutionPolicy, HookHandler, HookRegistration,
        HookResult, MessagePart, MessageRole, RunId, SessionId,
    };

    #[derive(Clone)]
    struct RecordingExecutor {
        inner: Arc<dyn ProcessExecutor>,
        requests: Arc<Mutex<Vec<ExecRequest>>>,
    }

    impl ProcessExecutor for RecordingExecutor {
        fn prepare(&self, request: ExecRequest) -> std::result::Result<Command, SandboxError> {
            self.requests.lock().unwrap().push(request.clone());
            self.inner.prepare(request)
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<HookAuditEvent>>,
    }

    impl HookExecutionObserver for RecordingObserver {
        fn record(&self, event: HookAuditEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn command_hook_executor_routes_process_launch_through_executor() {
        let dir = tempfile::tempdir().unwrap();
        let command_path = dir.path().join("hook.sh");
        std::fs::write(
            &command_path,
            "#!/bin/sh\nprintf '{\"effects\":[{\"kind\":\"append_message\",\"role\":\"system\",\"parts\":[{\"type\":\"text\",\"text\":\"ok\"}]}]}'",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&command_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&command_path, permissions).unwrap();
        }

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
                &HookRegistration {
                    name: "hook".to_string(),
                    event: HookEvent::Notification,
                    matcher: None,
                    handler: HookHandler::Command(types::CommandHookHandler {
                        command: command_path.to_string_lossy().to_string(),
                        asynchronous: false,
                    }),
                    timeout_ms: None,
                    execution: Some(HookExecutionPolicy {
                        exec_roots: vec![dir.path().to_path_buf()],
                        ..HookExecutionPolicy::default()
                    }),
                },
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

        assert_eq!(
            output,
            HookResult {
                effects: vec![HookEffect::AppendMessage {
                    role: MessageRole::System,
                    parts: vec![MessagePart::text("ok")],
                }],
            }
        );
        let logged = requests.lock().unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(
            logged[0].program,
            command_path.to_string_lossy().to_string()
        );
        assert!(logged[0].args.is_empty());
        assert!(logged[0].env.contains_key("NANOCLAW_CORE_HOOK_PAYLOAD"));
        assert_eq!(logged[0].runtime_scope.run_id, Some(run_id.clone()));
        assert_eq!(logged[0].sandbox_policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(logged[0].sandbox_policy.network, NetworkPolicy::Off);
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

    #[tokio::test]
    async fn default_command_executor_requires_explicit_execution_grants() {
        let dir = tempfile::tempdir().unwrap();
        let command_path = dir.path().join("hook.sh");
        std::fs::write(&command_path, "#!/bin/sh\nprintf ''").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&command_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&command_path, permissions).unwrap();
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let process_executor = Arc::new(RecordingExecutor {
            inner: Arc::new(HostProcessExecutor),
            requests: requests.clone(),
        });
        let executor =
            DefaultCommandHookExecutor::with_process_executor(BTreeMap::new(), process_executor);
        let error = executor
            .execute(
                &HookRegistration {
                    name: "hook".to_string(),
                    event: HookEvent::Notification,
                    matcher: None,
                    handler: HookHandler::Command(types::CommandHookHandler {
                        command: command_path.to_string_lossy().to_string(),
                        asynchronous: false,
                    }),
                    timeout_ms: None,
                    execution: None,
                },
                HookContext {
                    event: HookEvent::Notification,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
                    turn_id: None,
                    fields: BTreeMap::new(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("requires execution policy grants")
        );
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn command_hook_uses_shared_audit_plane() {
        let dir = tempfile::tempdir().unwrap();
        let command_path = dir.path().join("hook.sh");
        std::fs::write(&command_path, "#!/bin/sh\nprintf ''").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&command_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&command_path, permissions).unwrap();
        }

        let observer = Arc::new(RecordingObserver::default());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let executor = DefaultCommandHookExecutor::with_process_executor_policy_and_observer(
            BTreeMap::new(),
            Arc::new(RecordingExecutor {
                inner: Arc::new(HostProcessExecutor),
                requests,
            }),
            super::default_hook_command_sandbox_policy(),
            observer.clone(),
        );

        executor
            .execute(
                &HookRegistration {
                    name: "hook".to_string(),
                    event: HookEvent::Notification,
                    matcher: None,
                    handler: HookHandler::Command(types::CommandHookHandler {
                        command: command_path.to_string_lossy().to_string(),
                        asynchronous: false,
                    }),
                    timeout_ms: None,
                    execution: Some(HookExecutionPolicy {
                        plugin_id: Some("plugin".to_string()),
                        exec_roots: vec![dir.path().to_path_buf()],
                        ..HookExecutionPolicy::default()
                    }),
                },
                HookContext {
                    event: HookEvent::Notification,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
                    turn_id: None,
                    fields: BTreeMap::new(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, HookAuditAction::ExecutePath);
        assert_eq!(events[0].outcome, HookAuditOutcome::Allowed);
        assert_eq!(events[1].outcome, HookAuditOutcome::Completed);
    }
}
