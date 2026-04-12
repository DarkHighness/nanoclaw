use crate::HOST_FEATURE_HOST_PROCESS_SURFACES;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::process::{RuntimeScope, SandboxPolicy};
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use types::{
    AgentId, AgentSessionId, MonitorId, MonitorSummaryRecord, SessionId, TaskId, ToolAvailability,
    ToolCallId, ToolOutputMode, ToolResult, ToolSpec, TurnId,
};

use super::unified_exec::{resolve_exec_cwd, resolve_shell_command, runtime_scope_from_context};

const MONITOR_START_TOOL_NAME: &str = "monitor_start";
const MONITOR_LIST_TOOL_NAME: &str = "monitor_list";
const MONITOR_STOP_TOOL_NAME: &str = "monitor_stop";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonitorRuntimeContext {
    pub session_id: Option<SessionId>,
    pub agent_session_id: Option<AgentSessionId>,
    pub turn_id: Option<TurnId>,
    pub parent_agent_id: Option<AgentId>,
    pub task_id: Option<TaskId>,
}

impl From<&ToolExecutionContext> for MonitorRuntimeContext {
    fn from(ctx: &ToolExecutionContext) -> Self {
        Self {
            session_id: ctx.session_id.clone(),
            agent_session_id: ctx.agent_session_id.clone(),
            turn_id: ctx.turn_id.clone(),
            parent_agent_id: ctx.agent_id.clone(),
            task_id: ctx.task_id.as_deref().map(TaskId::from),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorLaunchRequest {
    pub command: String,
    pub cwd: PathBuf,
    pub shell: String,
    pub login: bool,
    pub env: BTreeMap<String, String>,
    pub sandbox_policy: SandboxPolicy,
    pub runtime_scope: RuntimeScope,
}

#[async_trait]
pub trait MonitorManager: Send + Sync {
    async fn start_monitor(
        &self,
        runtime: MonitorRuntimeContext,
        request: MonitorLaunchRequest,
    ) -> Result<MonitorSummaryRecord>;

    async fn list_monitors(
        &self,
        runtime: MonitorRuntimeContext,
        include_closed: bool,
    ) -> Result<Vec<MonitorSummaryRecord>>;

    async fn stop_monitor(
        &self,
        runtime: MonitorRuntimeContext,
        monitor_id: MonitorId,
        reason: Option<String>,
    ) -> Result<MonitorSummaryRecord>;
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MonitorStartToolInput {
    pub cmd: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub login: Option<bool>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct MonitorListToolInput {
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MonitorStopToolInput {
    pub monitor_id: MonitorId,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct MonitorStartToolOutput {
    monitor: MonitorSummaryRecord,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct MonitorListToolOutput {
    monitors: Vec<MonitorSummaryRecord>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct MonitorStopToolOutput {
    monitor: MonitorSummaryRecord,
}

#[derive(Clone)]
pub struct MonitorStartTool {
    manager: Arc<dyn MonitorManager>,
}

impl MonitorStartTool {
    #[must_use]
    pub fn new(manager: Arc<dyn MonitorManager>) -> Self {
        Self { manager }
    }
}

#[derive(Clone)]
pub struct MonitorListTool {
    manager: Arc<dyn MonitorManager>,
}

impl MonitorListTool {
    #[must_use]
    pub fn new(manager: Arc<dyn MonitorManager>) -> Self {
        Self { manager }
    }
}

#[derive(Clone)]
pub struct MonitorStopTool {
    manager: Arc<dyn MonitorManager>,
}

impl MonitorStopTool {
    #[must_use]
    pub fn new(manager: Arc<dyn MonitorManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for MonitorStartTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            MONITOR_START_TOOL_NAME,
            "Start a background shell monitor that continues streaming stdout/stderr events into the active session while other work proceeds.",
            serde_json::to_value(schema_for!(MonitorStartToolInput))
                .expect("monitor_start schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(MonitorStartToolOutput))
                .expect("monitor_start output schema"),
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
        let external_call_id = types::CallId::from(&call_id);
        let input: MonitorStartToolInput = serde_json::from_value(arguments)?;
        let command = resolve_shell_command(&input.cmd, MONITOR_START_TOOL_NAME)?;
        let shell = input
            .shell
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| agent_env::shell_or_default("/bin/sh"));
        let login = input.login.unwrap_or(true);
        let request = MonitorLaunchRequest {
            command,
            cwd: resolve_exec_cwd(input.workdir.as_deref(), ctx)?,
            shell,
            login,
            env: input.env.unwrap_or_default(),
            sandbox_policy: ctx.sandbox_policy(),
            runtime_scope: runtime_scope_from_context(ctx),
        };
        let monitor = self
            .manager
            .start_monitor(MonitorRuntimeContext::from(ctx), request)
            .await?;
        Ok(ToolResult::text(
            call_id,
            MONITOR_START_TOOL_NAME,
            render_monitor_summary("monitor_start", &monitor),
        )
        .with_structured_content(json!(MonitorStartToolOutput { monitor }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for MonitorListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            MONITOR_LIST_TOOL_NAME,
            "List background monitors attached to the active session.",
            serde_json::to_value(schema_for!(MonitorListToolInput)).expect("monitor_list schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(MonitorListToolOutput))
                .expect("monitor_list output schema"),
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
        let external_call_id = types::CallId::from(&call_id);
        let input: MonitorListToolInput = serde_json::from_value(arguments)?;
        let monitors = self
            .manager
            .list_monitors(MonitorRuntimeContext::from(ctx), input.include_closed)
            .await?;
        Ok(ToolResult::text(
            call_id,
            MONITOR_LIST_TOOL_NAME,
            render_monitor_list(&monitors),
        )
        .with_structured_content(json!(MonitorListToolOutput { monitors }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for MonitorStopTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            MONITOR_STOP_TOOL_NAME,
            "Stop a background monitor by id and return its final summary.",
            serde_json::to_value(schema_for!(MonitorStopToolInput)).expect("monitor_stop schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(MonitorStopToolOutput))
                .expect("monitor_stop output schema"),
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
        let external_call_id = types::CallId::from(&call_id);
        let input: MonitorStopToolInput = serde_json::from_value(arguments)?;
        let monitor = self
            .manager
            .stop_monitor(
                MonitorRuntimeContext::from(ctx),
                input.monitor_id,
                input.reason,
            )
            .await?;
        Ok(ToolResult::text(
            call_id,
            MONITOR_STOP_TOOL_NAME,
            render_monitor_summary("monitor_stop", &monitor),
        )
        .with_structured_content(json!(MonitorStopToolOutput { monitor }))
        .with_call_id(external_call_id))
    }
}

pub(crate) fn render_monitor_summary(tool_name: &str, summary: &MonitorSummaryRecord) -> String {
    let mut lines = vec![
        format!(
            "[{tool_name} monitor_id={} status={}]",
            summary.monitor_id, summary.status
        ),
        format!("command> {}", summary.command),
        format!("cwd> {}", summary.cwd),
        format!("shell> {}", render_shell_summary(summary)),
        format!("started_at_unix_s> {}", summary.started_at_unix_s),
    ];
    if let Some(task_id) = summary.task_id.as_ref() {
        lines.push(format!("task_id> {task_id}"));
    }
    if let Some(finished_at_unix_s) = summary.finished_at_unix_s {
        lines.push(format!("finished_at_unix_s> {finished_at_unix_s}"));
    }
    lines.join("\n")
}

fn render_monitor_list(monitors: &[MonitorSummaryRecord]) -> String {
    if monitors.is_empty() {
        return "[monitor_list]\nmonitors> 0".to_string();
    }

    let mut lines = vec![
        "[monitor_list]".to_string(),
        format!("monitors> {}", monitors.len()),
    ];
    lines.extend(monitors.iter().map(|monitor| {
        format!(
            "{} {} @ {} ({})",
            monitor.monitor_id,
            monitor.status,
            monitor.cwd,
            truncate_command(&monitor.command)
        )
    }));
    lines.join("\n")
}

fn render_shell_summary(summary: &MonitorSummaryRecord) -> String {
    if summary.login {
        format!("{} -lc", summary.shell)
    } else {
        format!("{} -c", summary.shell)
    }
}

fn truncate_command(command: &str) -> String {
    const MAX_CHARS: usize = 72;
    let mut chars = command.chars();
    let mut rendered = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        rendered.push_str("...");
    }
    rendered
}
