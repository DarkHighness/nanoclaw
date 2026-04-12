use crate::backend::SessionEventStream;
use crate::ui::SessionEvent;
use agent::tools::{
    MonitorLaunchRequest, MonitorManager, MonitorRuntimeContext, Result as ToolResult, ToolError,
};
use agent::types::{
    MonitorEventKind, MonitorEventRecord, MonitorId, MonitorStatus, MonitorStream,
    MonitorSummaryRecord, SessionEventEnvelope, SessionEventKind, SessionId, new_opaque_id,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use store::SessionStore;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Child;
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tracing::warn;

struct SessionMonitor {
    runtime: MonitorRuntimeContext,
    summary: Mutex<MonitorSummaryRecord>,
    child: AsyncMutex<Child>,
    cancel_reason: Mutex<Option<String>>,
    completion: Notify,
}

impl SessionMonitor {
    fn summary(&self) -> MonitorSummaryRecord {
        self.summary.lock().expect("monitor summary lock").clone()
    }

    fn update_summary(
        &self,
        status: MonitorStatus,
        finished_at_unix_s: Option<u64>,
    ) -> MonitorSummaryRecord {
        let mut summary = self.summary.lock().expect("monitor summary lock");
        summary.status = status;
        summary.finished_at_unix_s = finished_at_unix_s;
        summary.clone()
    }

    fn request_cancel(&self, reason: Option<String>) {
        let mut cancel_reason = self
            .cancel_reason
            .lock()
            .expect("monitor cancel reason lock");
        if cancel_reason.is_none() {
            *cancel_reason = reason;
        }
    }

    fn cancel_reason(&self) -> Option<String> {
        self.cancel_reason
            .lock()
            .expect("monitor cancel reason lock")
            .clone()
    }
}

#[derive(Clone)]
pub struct SessionMonitorManager {
    store: Arc<dyn SessionStore>,
    events: SessionEventStream,
    process_executor: Arc<dyn agent::tools::ProcessExecutor>,
    monitors: Arc<Mutex<BTreeMap<MonitorId, Arc<SessionMonitor>>>>,
}

impl SessionMonitorManager {
    #[must_use]
    pub fn new(
        store: Arc<dyn SessionStore>,
        events: SessionEventStream,
        process_executor: Arc<dyn agent::tools::ProcessExecutor>,
    ) -> Self {
        Self {
            store,
            events,
            process_executor,
            monitors: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn require_attached_runtime(
        runtime: &MonitorRuntimeContext,
    ) -> ToolResult<(SessionId, agent::types::AgentSessionId)> {
        let session_id = runtime.session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("monitor tools require an attached runtime session")
        })?;
        let agent_session_id = runtime.agent_session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("monitor tools require an attached runtime agent session")
        })?;
        Ok((session_id, agent_session_id))
    }

    async fn append_session_event(
        &self,
        runtime: &MonitorRuntimeContext,
        event: SessionEventKind,
    ) -> ToolResult<()> {
        let (session_id, agent_session_id) = Self::require_attached_runtime(runtime)?;
        self.store
            .append(SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                runtime.turn_id.clone(),
                None,
                event,
            ))
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))
    }

    async fn publish_started(
        &self,
        runtime: &MonitorRuntimeContext,
        summary: MonitorSummaryRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::MonitorStarted {
                summary: summary.clone(),
            },
        )
        .await?;
        self.events
            .publish(SessionEvent::MonitorStarted { summary });
        Ok(())
    }

    async fn publish_event(
        &self,
        runtime: &MonitorRuntimeContext,
        event: MonitorEventRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::MonitorEvent {
                event: event.clone(),
            },
        )
        .await?;
        self.events.publish(SessionEvent::MonitorEvent { event });
        Ok(())
    }

    async fn publish_updated(
        &self,
        runtime: &MonitorRuntimeContext,
        summary: MonitorSummaryRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::MonitorUpdated {
                summary: summary.clone(),
            },
        )
        .await?;
        self.events
            .publish(SessionEvent::MonitorUpdated { summary });
        Ok(())
    }

    fn monitor_state(&self, monitor_id: &MonitorId) -> Option<Arc<SessionMonitor>> {
        self.monitors
            .lock()
            .expect("monitor registry lock")
            .get(monitor_id)
            .cloned()
    }

    fn insert_monitor(&self, state: Arc<SessionMonitor>) {
        self.monitors
            .lock()
            .expect("monitor registry lock")
            .insert(state.summary().monitor_id.clone(), state);
    }
}

#[async_trait]
impl MonitorManager for SessionMonitorManager {
    async fn start_monitor(
        &self,
        runtime: MonitorRuntimeContext,
        request: MonitorLaunchRequest,
    ) -> ToolResult<MonitorSummaryRecord> {
        let _ = Self::require_attached_runtime(&runtime)?;
        let summary = MonitorSummaryRecord {
            monitor_id: MonitorId::from(format!("monitor_{}", new_opaque_id())),
            session_id: runtime
                .session_id
                .clone()
                .expect("monitor summary requires session_id"),
            agent_session_id: runtime
                .agent_session_id
                .clone()
                .expect("monitor summary requires agent_session_id"),
            parent_agent_id: runtime.parent_agent_id.clone(),
            task_id: runtime.task_id.clone(),
            command: request.command.clone(),
            cwd: request.cwd.display().to_string(),
            shell: request.shell.clone(),
            login: request.login,
            status: MonitorStatus::Running,
            started_at_unix_s: unix_timestamp_s(),
            finished_at_unix_s: None,
        };
        let mut child = self
            .process_executor
            .prepare(agent::tools::ExecRequest {
                program: request.shell.clone(),
                args: exec_args(request.login, &request.command),
                cwd: Some(request.cwd.clone()),
                env: request.env.clone(),
                stdin: agent::tools::ProcessStdio::Null,
                stdout: agent::tools::ProcessStdio::Piped,
                stderr: agent::tools::ProcessStdio::Piped,
                kill_on_drop: true,
                origin: agent::tools::ExecutionOrigin::HostUtility {
                    name: "monitor_start".to_string(),
                },
                runtime_scope: request.runtime_scope.clone(),
                sandbox_policy: request.sandbox_policy.clone(),
            })?
            .spawn()
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to start monitor process: {error}"))
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::invalid_state("monitor process did not expose stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::invalid_state("monitor process did not expose stderr"))?;
        let state = Arc::new(SessionMonitor {
            runtime: runtime.clone(),
            summary: Mutex::new(summary.clone()),
            child: AsyncMutex::new(child),
            cancel_reason: Mutex::new(None),
            completion: Notify::new(),
        });
        self.insert_monitor(state.clone());
        self.publish_started(&runtime, summary.clone()).await?;

        let manager = self.clone();
        let stdout_state = state.clone();
        tokio::spawn(async move {
            forward_monitor_lines(manager, stdout_state, stdout, MonitorStream::Stdout).await;
        });

        let manager = self.clone();
        let stderr_state = state.clone();
        tokio::spawn(async move {
            forward_monitor_lines(manager, stderr_state, stderr, MonitorStream::Stderr).await;
        });

        let manager = self.clone();
        tokio::spawn(async move {
            finalize_monitor(manager, state).await;
        });

        Ok(summary)
    }

    async fn list_monitors(
        &self,
        _runtime: MonitorRuntimeContext,
        include_closed: bool,
    ) -> ToolResult<Vec<MonitorSummaryRecord>> {
        let mut monitors = self
            .monitors
            .lock()
            .expect("monitor registry lock")
            .values()
            .map(|state| state.summary())
            .filter(|summary| include_closed || !summary.status.is_terminal())
            .collect::<Vec<_>>();
        monitors.sort_by(|left, right| {
            right
                .started_at_unix_s
                .cmp(&left.started_at_unix_s)
                .then_with(|| left.monitor_id.cmp(&right.monitor_id))
        });
        Ok(monitors)
    }

    async fn stop_monitor(
        &self,
        _runtime: MonitorRuntimeContext,
        monitor_id: MonitorId,
        reason: Option<String>,
    ) -> ToolResult<MonitorSummaryRecord> {
        let state = self
            .monitor_state(&monitor_id)
            .ok_or_else(|| ToolError::invalid(format!("unknown monitor id: {}", monitor_id)))?;
        let summary = state.summary();
        if summary.status.is_terminal() {
            return Ok(summary);
        }

        state.request_cancel(reason);
        state.child.lock().await.start_kill().map_err(|error| {
            ToolError::invalid_state(format!("failed to stop monitor: {error}"))
        })?;
        state.completion.notified().await;
        Ok(state.summary())
    }
}

async fn forward_monitor_lines<R>(
    manager: SessionMonitorManager,
    state: Arc<SessionMonitor>,
    stream: R,
    stream_kind: MonitorStream,
) where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream).lines();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                if line.is_empty() {
                    continue;
                }
                let event = MonitorEventRecord {
                    monitor_id: state.summary().monitor_id,
                    timestamp_unix_s: unix_timestamp_s(),
                    kind: MonitorEventKind::Line {
                        stream: stream_kind,
                        text: line,
                    },
                };
                if let Err(error) = manager.publish_event(&state.runtime, event).await {
                    warn!(error = %error, "failed to persist monitor output line");
                }
            }
            Ok(None) => break,
            Err(error) => {
                warn!(error = %error, "failed to read monitor output");
                break;
            }
        }
    }
}

async fn finalize_monitor(manager: SessionMonitorManager, state: Arc<SessionMonitor>) {
    let wait_result = state.child.lock().await.wait().await;
    let cancel_reason = state.cancel_reason();
    let (status, terminal_kind) = if let Some(reason) = cancel_reason {
        (
            MonitorStatus::Cancelled,
            MonitorEventKind::Cancelled {
                reason: Some(reason),
            },
        )
    } else {
        match wait_result {
            Ok(wait_status) if wait_status.success() => (
                MonitorStatus::Completed,
                MonitorEventKind::Completed {
                    exit_code: wait_status.code().unwrap_or(0),
                },
            ),
            Ok(wait_status) => (
                MonitorStatus::Failed,
                MonitorEventKind::Failed {
                    exit_code: wait_status.code(),
                    error: None,
                },
            ),
            Err(error) => (
                MonitorStatus::Failed,
                MonitorEventKind::Failed {
                    exit_code: None,
                    error: Some(error.to_string()),
                },
            ),
        }
    };

    let state_changed = MonitorEventRecord {
        monitor_id: state.summary().monitor_id.clone(),
        timestamp_unix_s: unix_timestamp_s(),
        kind: MonitorEventKind::StateChanged { status },
    };
    if let Err(error) = manager.publish_event(&state.runtime, state_changed).await {
        warn!(error = %error, "failed to persist monitor state change");
    }

    let terminal = MonitorEventRecord {
        monitor_id: state.summary().monitor_id.clone(),
        timestamp_unix_s: unix_timestamp_s(),
        kind: terminal_kind,
    };
    if let Err(error) = manager.publish_event(&state.runtime, terminal).await {
        warn!(error = %error, "failed to persist monitor terminal event");
    }

    let updated = state.update_summary(status, Some(unix_timestamp_s()));
    if let Err(error) = manager.publish_updated(&state.runtime, updated).await {
        warn!(error = %error, "failed to persist monitor summary update");
    }
    state.completion.notify_waiters();
}

fn unix_timestamp_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn exec_args(login: bool, command: &str) -> Vec<String> {
    if login {
        vec!["-lc".to_string(), command.to_string()]
    } else {
        vec!["-c".to_string(), command.to_string()]
    }
}
