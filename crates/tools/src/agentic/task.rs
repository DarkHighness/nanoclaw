use crate::ToolExecutionContext;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::fs::{load_tool_image, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use base64::Engine;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use types::{
    AgentHandle, AgentId, AgentInputDelivery, AgentResultEnvelope, AgentSessionId, AgentTaskSpec,
    AgentWaitMode, AgentWaitRequest, AgentWaitResponse, CallId, Message, MessagePart, MessageRole,
    SessionId, TaskId, TaskOrigin, TaskRecord, TaskStatus, TaskSummaryRecord, ToolCallId, ToolName,
    ToolOutputMode, ToolResult, ToolSpec, TurnId,
};

const TASK_CREATE_TOOL_NAME: &str = "task_create";
const TASK_GET_TOOL_NAME: &str = "task_get";
const TASK_LIST_TOOL_NAME: &str = "task_list";
const TASK_UPDATE_TOOL_NAME: &str = "task_update";
const TASK_STOP_TOOL_NAME: &str = "task_stop";
const SPAWN_AGENT_TOOL_NAME: &str = "spawn_agent";
const SEND_INPUT_TOOL_NAME: &str = "send_input";
const WAIT_AGENT_TOOL_NAME: &str = "wait_agent";
const RESUME_AGENT_TOOL_NAME: &str = "resume_agent";
const LIST_AGENTS_TOOL_NAME: &str = "list_agents";
const CLOSE_AGENT_TOOL_NAME: &str = "close_agent";

#[derive(Clone, Debug, Default)]
pub struct SubagentParentContext {
    pub session_id: Option<SessionId>,
    pub agent_session_id: Option<AgentSessionId>,
    pub turn_id: Option<TurnId>,
    pub parent_agent_id: Option<AgentId>,
}

impl From<&ToolExecutionContext> for SubagentParentContext {
    fn from(ctx: &ToolExecutionContext) -> Self {
        Self {
            session_id: ctx.session_id.clone(),
            agent_session_id: ctx.agent_session_id.clone(),
            turn_id: ctx.turn_id.clone(),
            parent_agent_id: ctx.agent_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentTaskInput {
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub steer: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<ToolName>,
    #[serde(default)]
    pub requested_write_set: Vec<String>,
    #[serde(default)]
    pub dependency_ids: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskCreateToolInput {
    #[serde(flatten)]
    pub task: AgentTaskInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskGetToolInput {
    pub task_id: TaskId,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskListToolInput {
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskUpdateToolInput {
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskStopToolInput {
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentSpawnToolInput {
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub fork_context: bool,
    #[serde(default)]
    pub items: Vec<AgentInputItem>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentInputItem {
    #[serde(rename = "type", default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentSendToolInput {
    pub target: AgentId,
    #[serde(default)]
    pub interrupt: bool,
    #[serde(default)]
    pub items: Vec<AgentInputItem>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentWaitToolInput {
    pub targets: Vec<AgentId>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentCloseToolInput {
    pub target: AgentId,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentResumeToolInput {
    pub id: AgentId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubagentLaunchSpec {
    pub task: AgentTaskSpec,
    pub initial_input: Message,
    pub fork_context: bool,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

pub type SubagentInputDelivery = AgentInputDelivery;

impl SubagentLaunchSpec {
    #[must_use]
    pub fn from_task(task: AgentTaskSpec) -> Self {
        let initial_input = Message::user(task.prompt.clone());
        Self {
            task,
            initial_input,
            fork_context: false,
            model: None,
            reasoning_effort: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadedAgentInputFile {
    requested_path: String,
    file_name: Option<String>,
    mime_type: Option<String>,
    data_base64: String,
}

#[async_trait]
pub trait SubagentExecutor: Send + Sync {
    async fn spawn(
        &self,
        parent: SubagentParentContext,
        tasks: Vec<SubagentLaunchSpec>,
    ) -> Result<Vec<AgentHandle>>;

    async fn send(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        message: Message,
        delivery: SubagentInputDelivery,
    ) -> Result<AgentHandle>;

    async fn wait(
        &self,
        parent: SubagentParentContext,
        request: AgentWaitRequest,
    ) -> Result<AgentWaitResponse>;

    async fn resume(&self, parent: SubagentParentContext, agent_id: AgentId)
    -> Result<AgentHandle>;

    async fn list(&self, parent: SubagentParentContext) -> Result<Vec<AgentHandle>>;

    async fn cancel(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        reason: Option<String>,
    ) -> Result<AgentHandle>;
}

#[async_trait]
pub trait TaskManager: Send + Sync {
    async fn create_task(
        &self,
        parent: SubagentParentContext,
        task: AgentTaskSpec,
        status: TaskStatus,
    ) -> Result<TaskRecord>;

    async fn get_task(&self, parent: SubagentParentContext, task_id: &TaskId)
    -> Result<TaskRecord>;

    async fn list_tasks(
        &self,
        parent: SubagentParentContext,
        include_closed: bool,
    ) -> Result<Vec<TaskSummaryRecord>>;

    async fn update_task(
        &self,
        parent: SubagentParentContext,
        task_id: TaskId,
        status: Option<TaskStatus>,
        summary: Option<String>,
    ) -> Result<TaskRecord>;

    async fn stop_task(
        &self,
        parent: SubagentParentContext,
        task_id: TaskId,
        reason: Option<String>,
    ) -> Result<TaskRecord>;
}

macro_rules! define_executor_tool {
    ($name:ident) => {
        #[derive(Clone)]
        pub struct $name {
            executor: Arc<dyn SubagentExecutor>,
        }

        impl $name {
            #[must_use]
            pub fn new(executor: Arc<dyn SubagentExecutor>) -> Self {
                Self { executor }
            }
        }
    };
}

define_executor_tool!(AgentSpawnTool);
define_executor_tool!(AgentSendTool);
define_executor_tool!(AgentWaitTool);
define_executor_tool!(AgentResumeTool);
define_executor_tool!(AgentListTool);
define_executor_tool!(AgentCancelTool);

#[derive(Clone)]
pub struct TaskCreateTool {
    manager: Arc<dyn TaskManager>,
}

#[derive(Clone)]
pub struct TaskGetTool {
    manager: Arc<dyn TaskManager>,
}

#[derive(Clone)]
pub struct TaskListTool {
    manager: Arc<dyn TaskManager>,
}

#[derive(Clone)]
pub struct TaskUpdateTool {
    manager: Arc<dyn TaskManager>,
}

#[derive(Clone)]
pub struct TaskStopTool {
    manager: Arc<dyn TaskManager>,
}

macro_rules! define_task_tool {
    ($name:ident) => {
        impl $name {
            #[must_use]
            pub fn new(manager: Arc<dyn TaskManager>) -> Self {
                Self { manager }
            }
        }
    };
}

define_task_tool!(TaskCreateTool);
define_task_tool!(TaskGetTool);
define_task_tool!(TaskListTool);
define_task_tool!(TaskUpdateTool);
define_task_tool!(TaskStopTool);

#[async_trait]
impl Tool for TaskCreateTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            TASK_CREATE_TOOL_NAME,
            "Create a first-class task record without launching a child agent. Use spawn_agent when you need a new child runtime; use task_create for tracked work items and TODOs.",
            serde_json::to_value(schema_for!(TaskCreateToolInput)).expect("task_create schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(TaskRecord)).expect("task_create output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskCreateToolInput = serde_json::from_value(arguments)?;
        let task = normalize_task_input(
            input.task,
            TaskOrigin::AgentCreated,
            Some(TaskId::from(format!("task_{call_id}"))),
        )?;
        let record = self
            .manager
            .create_task(
                SubagentParentContext::from(ctx),
                task,
                input.status.unwrap_or(TaskStatus::Open),
            )
            .await?;
        build_tool_result(
            call_id,
            TASK_CREATE_TOOL_NAME,
            render_task_record_line("Created", &record),
            record,
        )
    }
}

#[async_trait]
impl Tool for TaskGetTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            TASK_GET_TOOL_NAME,
            "Load one task record by task id.",
            serde_json::to_value(schema_for!(TaskGetToolInput)).expect("task_get schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(TaskRecord)).expect("task_get output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskGetToolInput = serde_json::from_value(arguments)?;
        let record = self
            .manager
            .get_task(SubagentParentContext::from(ctx), &input.task_id)
            .await?;
        build_tool_result(
            call_id,
            TASK_GET_TOOL_NAME,
            render_task_record_line("Loaded", &record),
            record,
        )
    }
}

#[async_trait]
impl Tool for TaskListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            TASK_LIST_TOOL_NAME,
            "List task records visible from the current parent scope.",
            serde_json::to_value(schema_for!(TaskListToolInput)).expect("task_list schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(Vec<TaskSummaryRecord>))
                .expect("task_list output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskListToolInput = serde_json::from_value(arguments)?;
        let tasks = self
            .manager
            .list_tasks(SubagentParentContext::from(ctx), input.include_closed)
            .await?;
        build_tool_result(
            call_id,
            TASK_LIST_TOOL_NAME,
            if tasks.is_empty() {
                "No tracked tasks.".to_string()
            } else {
                tasks
                    .iter()
                    .map(render_task_summary_line)
                    .collect::<Vec<_>>()
                    .join("\n")
            },
            tasks,
        )
    }
}

#[async_trait]
impl Tool for TaskUpdateTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            TASK_UPDATE_TOOL_NAME,
            "Update task status or summary without changing the shared plan surface.",
            serde_json::to_value(schema_for!(TaskUpdateToolInput)).expect("task_update schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(TaskRecord)).expect("task_update output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskUpdateToolInput = serde_json::from_value(arguments)?;
        if input.status.is_none()
            && input
                .summary
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ToolError::invalid(
                "task_update requires at least one of status or summary",
            ));
        }
        let record = self
            .manager
            .update_task(
                SubagentParentContext::from(ctx),
                input.task_id,
                input.status,
                normalize_optional_non_empty(input.summary),
            )
            .await?;
        build_tool_result(
            call_id,
            TASK_UPDATE_TOOL_NAME,
            render_task_record_line("Updated", &record),
            record,
        )
    }
}

#[async_trait]
impl Tool for TaskStopTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            TASK_STOP_TOOL_NAME,
            "Stop a task by cancelling its live child agent when present, or by marking the record cancelled otherwise.",
            serde_json::to_value(schema_for!(TaskStopToolInput)).expect("task_stop schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(TaskRecord)).expect("task_stop output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskStopToolInput = serde_json::from_value(arguments)?;
        let record = self
            .manager
            .stop_task(
                SubagentParentContext::from(ctx),
                input.task_id,
                normalize_optional_non_empty(input.reason),
            )
            .await?;
        build_tool_result(
            call_id,
            TASK_STOP_TOOL_NAME,
            render_task_record_line("Stopped", &record),
            record,
        )
    }
}

#[async_trait]
impl Tool for AgentSpawnTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            SPAWN_AGENT_TOOL_NAME,
            "Spawn one child agent without waiting so it can receive follow-up input later.",
            serde_json::to_value(schema_for!(AgentSpawnToolInput)).expect("spawn_agent schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentHandle)).expect("spawn_agent output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentSpawnToolInput = serde_json::from_value(arguments)?;
        let launch = normalize_spawn_input(input, &call_id, ctx).await?;
        let mut handles = self
            .executor
            .spawn(SubagentParentContext::from(ctx), vec![launch])
            .await?;
        let handle = handles
            .pop()
            .ok_or_else(|| ToolError::invalid_state("spawn_agent returned no agent"))?;
        build_tool_result(
            call_id,
            SPAWN_AGENT_TOOL_NAME,
            render_handle_line(&handle),
            handle,
        )
    }
}

#[async_trait]
impl Tool for AgentSendTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            SEND_INPUT_TOOL_NAME,
            "Send a message or steering payload to a child agent.",
            serde_json::to_value(schema_for!(AgentSendToolInput)).expect("send_input schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentHandle)).expect("send_input output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentSendToolInput = serde_json::from_value(arguments)?;
        let (target, message, delivery) = normalize_send_input(input, ctx).await?;
        let handle = self
            .executor
            .send(SubagentParentContext::from(ctx), target, message, delivery)
            .await?;
        build_tool_result(
            call_id,
            SEND_INPUT_TOOL_NAME,
            render_send_input_line(&handle, delivery),
            handle,
        )
    }
}

#[async_trait]
impl Tool for AgentWaitTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            WAIT_AGENT_TOOL_NAME,
            "Wait for one or more child agents to reach a terminal state.",
            serde_json::to_value(schema_for!(AgentWaitToolInput)).expect("wait_agent schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentWaitResponse)).expect("wait_agent output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentWaitToolInput = serde_json::from_value(arguments)?;
        let parent = SubagentParentContext::from(ctx);
        let (wait, timed_out) = wait_for_targets(self.executor.as_ref(), parent, input).await?;
        build_tool_result(
            call_id,
            WAIT_AGENT_TOOL_NAME,
            render_wait_summary(WAIT_AGENT_TOOL_NAME, &wait, timed_out),
            wait,
        )
    }
}

#[async_trait]
impl Tool for AgentListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            LIST_AGENTS_TOOL_NAME,
            "List current child agents and their session metadata.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(Vec<AgentHandle>)).expect("list_agents output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        _arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let handles = self.executor.list(SubagentParentContext::from(ctx)).await?;
        build_tool_result(
            call_id,
            LIST_AGENTS_TOOL_NAME,
            handles
                .iter()
                .map(render_handle_line)
                .collect::<Vec<_>>()
                .join("\n"),
            handles,
        )
    }
}

#[async_trait]
impl Tool for AgentResumeTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            RESUME_AGENT_TOOL_NAME,
            "Resume a previously closed child agent so it can receive more input.",
            serde_json::to_value(schema_for!(AgentResumeToolInput)).expect("resume_agent schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentHandle)).expect("resume_agent output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentResumeToolInput = serde_json::from_value(arguments)?;
        let handle = self
            .executor
            .resume(SubagentParentContext::from(ctx), input.id)
            .await?;
        build_tool_result(
            call_id,
            RESUME_AGENT_TOOL_NAME,
            render_handle_line(&handle),
            handle,
        )
    }
}

#[async_trait]
impl Tool for AgentCancelTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            CLOSE_AGENT_TOOL_NAME,
            "Close a child agent by cancelling it if it is still running.",
            serde_json::to_value(schema_for!(AgentCloseToolInput)).expect("close_agent schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentHandle)).expect("close_agent output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentCloseToolInput = serde_json::from_value(arguments)?;
        let handle = self
            .executor
            .cancel(SubagentParentContext::from(ctx), input.target, None)
            .await?;
        build_tool_result(
            call_id,
            CLOSE_AGENT_TOOL_NAME,
            render_handle_line(&handle),
            handle,
        )
    }
}

fn normalize_task_input(
    input: AgentTaskInput,
    origin: TaskOrigin,
    fallback_task_id: Option<TaskId>,
) -> Result<AgentTaskSpec> {
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(ToolError::invalid("agent task prompt must not be empty"));
    }
    let role = input
        .role
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "general-purpose".to_string());
    let task_id = input
        .task_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(TaskId::from)
        .or(fallback_task_id)
        .unwrap_or_else(|| TaskId::from(format!("task_{}", types::new_opaque_id())));
    let dependency_ids = normalize_dependency_ids(input.dependency_ids);
    if dependency_ids
        .iter()
        .any(|dependency_id| dependency_id == &task_id)
    {
        return Err(ToolError::invalid(format!(
            "agent task {task_id} cannot depend on itself"
        )));
    }
    Ok(AgentTaskSpec {
        task_id,
        role,
        prompt,
        origin,
        steer: input
            .steer
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        allowed_tools: input.allowed_tools,
        requested_write_set: normalize_paths(input.requested_write_set),
        dependency_ids,
        timeout_seconds: input.timeout_seconds,
    })
}

async fn normalize_spawn_input(
    input: AgentSpawnToolInput,
    call_id: &ToolCallId,
    ctx: &ToolExecutionContext,
) -> Result<SubagentLaunchSpec> {
    let role = normalize_optional_non_empty(input.agent_type)
        .unwrap_or_else(|| "general-purpose".to_string());
    let normalized = normalize_agent_input(
        input.message,
        &input.items,
        "spawn_agent requires a message or at least one input item",
        ctx,
    )
    .await?;
    Ok(SubagentLaunchSpec {
        task: AgentTaskSpec {
            task_id: TaskId::from(format!("spawn_{}", call_id)),
            role,
            prompt: normalized.preview_text,
            origin: TaskOrigin::ChildAgentBacked,
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        },
        initial_input: normalized.message,
        fork_context: input.fork_context,
        model: normalize_optional_non_empty(input.model),
        reasoning_effort: normalize_optional_non_empty(input.reasoning_effort),
    })
}

async fn normalize_send_input(
    input: AgentSendToolInput,
    ctx: &ToolExecutionContext,
) -> Result<(AgentId, Message, SubagentInputDelivery)> {
    let target = input.target;
    let normalized = normalize_agent_input(
        input.message,
        &input.items,
        "send_input requires a message or at least one input item",
        ctx,
    )
    .await?;
    Ok((
        target,
        normalized.message,
        if input.interrupt {
            SubagentInputDelivery::Interrupt
        } else {
            SubagentInputDelivery::Queue
        },
    ))
}

struct NormalizedAgentInput {
    preview_text: String,
    message: Message,
}

async fn normalize_agent_input(
    message: Option<String>,
    items: &[AgentInputItem],
    empty_error: &str,
    ctx: &ToolExecutionContext,
) -> Result<NormalizedAgentInput> {
    let normalized_message = normalize_optional_non_empty(message);
    let mut preview_parts = Vec::new();
    let mut message_parts = Vec::new();

    if let Some(message) = normalized_message.as_ref() {
        preview_parts.push(message.clone());
        message_parts.push(MessagePart::text(message.clone()));
    }

    let mut preview_items = Vec::new();
    for item in items {
        if let Some(line) = render_agent_input_item_summary(item) {
            preview_items.push(line);
        }
        message_parts.extend(normalize_agent_input_item_parts(item, ctx).await?);
    }

    if !preview_items.is_empty() {
        preview_parts.push(preview_items.join("\n"));
    }

    if message_parts.is_empty() {
        return Err(ToolError::invalid(empty_error));
    }
    Ok(NormalizedAgentInput {
        preview_text: preview_parts.join("\n\n"),
        message: Message::new(MessageRole::User, message_parts),
    })
}

fn normalize_optional_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn normalize_agent_input_item_parts(
    item: &AgentInputItem,
    ctx: &ToolExecutionContext,
) -> Result<Vec<MessagePart>> {
    let item_type = normalized_agent_input_item_type(item);
    let text = trim_optional_field(item.text.as_deref());
    let path = trim_optional_field(item.path.as_deref());
    let name = trim_optional_field(item.name.as_deref());
    let image_url = trim_optional_field(item.image_url.as_deref());

    if item_type == "text" {
        let text = text.ok_or_else(|| {
            ToolError::invalid("agent input items with type=text require a non-empty text field")
        })?;
        return Ok(vec![MessagePart::text(text)]);
    }

    if path.is_none() && name.is_none() && image_url.is_none() && text.is_none() {
        return Err(ToolError::invalid(format!(
            "agent input item `{item_type}` must include at least one of text, path, name, or image_url"
        )));
    }

    if item_type == "local_image" {
        let path = path.ok_or_else(|| {
            ToolError::invalid("agent input items with type=local_image require a non-empty path")
        })?;
        let image = load_tool_image(path, ctx).await?;
        let mut parts = vec![image.message_part()];
        if let Some(caption) = compose_item_caption(name, text) {
            parts.push(MessagePart::text(caption));
        }
        return Ok(parts);
    }

    if item_type == "local_file" && path.is_some_and(is_remote_url) {
        return Err(ToolError::invalid(
            "agent input items with type=local_file require a workspace path; use type=file for remote URLs",
        ));
    }

    if is_local_file_item(item_type, path) {
        let path = path.ok_or_else(|| {
            ToolError::invalid(
                "agent input items with type=file or type=local_file require a non-empty path",
            )
        })?;
        let file = load_agent_input_file(path, ctx).await?;
        let mut parts = vec![MessagePart::File {
            file_name: file.file_name.clone(),
            mime_type: file.mime_type.clone(),
            data_base64: Some(file.data_base64),
            // Keep the original workspace-relative path attached for transcript
            // replay and provider fallbacks that cannot consume the binary part.
            // Provider adapters must not blindly treat non-URL paths as remote
            // fetch targets.
            uri: Some(file.requested_path.clone()),
        }];
        if let Some(caption) = compose_item_caption(name, text) {
            parts.push(MessagePart::text(caption));
        }
        return Ok(parts);
    }

    if is_remote_file_item(item_type, path) {
        let path = path.ok_or_else(|| {
            ToolError::invalid("agent input items with type=file require a non-empty path")
        })?;
        let mut parts = vec![remote_agent_input_file(path, name)];
        if let Some(caption) = compose_item_caption(name, text) {
            parts.push(MessagePart::text(caption));
        }
        return Ok(parts);
    }

    if is_remote_image_item(item_type, image_url, path) {
        let url = image_url.ok_or_else(|| {
            ToolError::invalid(
                "agent input items with type=image_url require a non-empty image_url",
            )
        })?;
        // Remote images should stay first-class image parts so providers can use
        // their native multimodal transport instead of degrading them into text.
        let mut parts = vec![MessagePart::image_url(url)];
        if let Some(caption) = compose_item_caption(name, text) {
            parts.push(MessagePart::text(caption));
        }
        return Ok(parts);
    }

    Ok(vec![MessagePart::reference(
        item_type,
        name.map(str::to_string),
        path.or(image_url).map(str::to_string),
        text.map(str::to_string),
    )])
}

fn compose_item_caption(name: Option<&str>, text: Option<&str>) -> Option<String> {
    match (name, text) {
        (Some(name), Some(text)) => Some(format!("{name}\n{text}")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(text)) => Some(text.to_string()),
        (None, None) => None,
    }
}

fn is_remote_image_item(item_type: &str, image_url: Option<&str>, path: Option<&str>) -> bool {
    image_url.is_some() && path.is_none() && matches!(item_type, "image_url" | "image" | "item")
}

fn is_local_file_item(item_type: &str, path: Option<&str>) -> bool {
    path.is_some_and(|path| !is_remote_url(path)) && matches!(item_type, "local_file" | "file")
}

fn is_remote_file_item(item_type: &str, path: Option<&str>) -> bool {
    item_type == "file" && path.is_some_and(is_remote_url)
}

async fn load_agent_input_file(
    requested_path: &str,
    ctx: &ToolExecutionContext,
) -> Result<LoadedAgentInputFile> {
    let resolved_path = resolve_tool_path_against_workspace_root(
        requested_path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_read_allowed(&resolved_path)?;
    let bytes = fs::read(&resolved_path).await?;
    Ok(LoadedAgentInputFile {
        requested_path: requested_path.to_string(),
        file_name: resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string),
        mime_type: sniff_agent_input_file_mime(&bytes, &resolved_path).map(str::to_string),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

fn sniff_agent_input_file_mime(bytes: &[u8], path: &Path) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

fn remote_agent_input_file(path: &str, name: Option<&str>) -> MessagePart {
    MessagePart::File {
        file_name: name
            .map(str::to_string)
            .or_else(|| derive_file_name(path))
            .filter(|value| !value.is_empty()),
        mime_type: sniff_remote_agent_input_file_mime(path, name).map(str::to_string),
        data_base64: None,
        uri: Some(path.to_string()),
    }
}

fn sniff_remote_agent_input_file_mime(path: &str, name: Option<&str>) -> Option<&'static str> {
    if name.is_some_and(|value| value.to_ascii_lowercase().ends_with(".pdf"))
        || path.to_ascii_lowercase().ends_with(".pdf")
    {
        return Some("application/pdf");
    }
    None
}

fn derive_file_name(path: &str) -> Option<String> {
    let path = path.split('#').next().unwrap_or(path);
    let path = path.split('?').next().unwrap_or(path);
    path.rsplit('/').next().and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn is_remote_url(path: &str) -> bool {
    path.starts_with("http://") || path.starts_with("https://")
}

fn render_agent_input_item_summary(item: &AgentInputItem) -> Option<String> {
    let item_type = normalized_agent_input_item_type(item);
    if item_type == "text" {
        return trim_optional_field(item.text.as_deref()).map(ToString::to_string);
    }

    let mut fields = Vec::new();
    if let Some(name) = trim_optional_field(item.name.as_deref()) {
        fields.push(format!("name={name}"));
    }
    if let Some(path) = trim_optional_field(item.path.as_deref()) {
        fields.push(format!("path={path}"));
    }
    if let Some(url) = trim_optional_field(item.image_url.as_deref()) {
        fields.push(format!("image_url={url}"));
    }
    if let Some(text) = trim_optional_field(item.text.as_deref()) {
        fields.push(format!("text={}", text.replace('\n', " ")));
    }
    if fields.is_empty() {
        None
    } else {
        Some(format!("[{item_type}] {}", fields.join(" ")))
    }
}

fn normalized_agent_input_item_type(item: &AgentInputItem) -> &str {
    item.item_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if item
                .text
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                "text"
            } else {
                "item"
            }
        })
}

fn trim_optional_field(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn normalize_paths(paths: Vec<String>) -> Vec<String> {
    let mut unique = BTreeSet::new();
    paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .filter(|path| unique.insert(path.clone()))
        .collect()
}

fn normalize_dependency_ids(dependency_ids: Vec<String>) -> Vec<TaskId> {
    let mut unique = BTreeSet::new();
    dependency_ids
        .into_iter()
        .map(|dependency_id| dependency_id.trim().to_string())
        .filter(|dependency_id| !dependency_id.is_empty())
        .map(TaskId::from)
        .filter(|dependency_id| unique.insert(dependency_id.clone()))
        .collect()
}

async fn wait_for_targets(
    executor: &dyn SubagentExecutor,
    parent: SubagentParentContext,
    input: AgentWaitToolInput,
) -> Result<(AgentWaitResponse, bool)> {
    if input.targets.is_empty() {
        return Err(ToolError::invalid(
            "wait_agent requires at least one target",
        ));
    }
    let request = AgentWaitRequest {
        agent_ids: input.targets,
        mode: AgentWaitMode::All,
    };
    match input.timeout_ms {
        Some(timeout_ms) => match tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            executor.wait(parent.clone(), request.clone()),
        )
        .await
        {
            Ok(wait) => Ok((wait?, false)),
            Err(_) => Ok((
                snapshot_wait_response(executor, parent, request.agent_ids).await?,
                true,
            )),
        },
        None => Ok((executor.wait(parent, request).await?, false)),
    }
}

async fn snapshot_wait_response(
    executor: &dyn SubagentExecutor,
    parent: SubagentParentContext,
    agent_ids: Vec<AgentId>,
) -> Result<AgentWaitResponse> {
    // `wait_agent(timeout_ms=...)` still needs a coherent snapshot of terminal
    // results and non-terminal handles after the timeout fires. Use `list` as
    // the cheap status snapshot, then fetch results only for the agents that
    // are already terminal so the timeout path never blocks again on runners
    // that are still active.
    let handles_by_id = executor
        .list(parent.clone())
        .await?
        .into_iter()
        .map(|handle| (handle.agent_id.clone(), handle))
        .collect::<BTreeMap<_, _>>();
    let mut completed = Vec::new();
    let mut pending = Vec::new();
    for agent_id in agent_ids {
        let handle = handles_by_id
            .get(&agent_id)
            .cloned()
            .ok_or_else(|| ToolError::invalid_state(format!("unknown child agent: {agent_id}")))?;
        if handle.status.is_terminal() {
            completed.push(handle);
        } else {
            pending.push(handle);
        }
    }
    let results = if completed.is_empty() {
        Vec::new()
    } else {
        executor
            .wait(
                parent,
                AgentWaitRequest {
                    agent_ids: completed
                        .iter()
                        .map(|handle| handle.agent_id.clone())
                        .collect(),
                    mode: AgentWaitMode::All,
                },
            )
            .await?
            .results
    };
    Ok(AgentWaitResponse {
        completed,
        pending,
        results,
    })
}

fn render_wait_summary(tool_name: &str, wait: &AgentWaitResponse, timed_out: bool) -> String {
    let status = if timed_out { "timed_out" } else { "completed" };
    let mut lines = vec![format!(
        "[{tool_name} {status} completed={} pending={} results={}]",
        wait.completed.len(),
        wait.pending.len(),
        wait.results.len()
    )];
    lines.extend(wait.completed.iter().map(render_handle_line));
    lines.extend(wait.results.iter().map(render_result_line));
    lines.join("\n")
}

fn render_handle_line(handle: &AgentHandle) -> String {
    format!(
        "{} status={} task={} session={} agent_session={}",
        handle.agent_id, handle.status, handle.task_id, handle.session_id, handle.agent_session_id
    )
}

fn render_send_input_line(handle: &AgentHandle, delivery: SubagentInputDelivery) -> String {
    format!(
        "{} delivery={} status={} task={} session={} agent_session={}",
        handle.agent_id,
        delivery,
        handle.status,
        handle.task_id,
        handle.session_id,
        handle.agent_session_id
    )
}

fn render_result_line(result: &AgentResultEnvelope) -> String {
    format!(
        "result {} status={} summary={}",
        result.agent_id, result.status, result.summary
    )
}

fn render_task_summary_line(summary: &TaskSummaryRecord) -> String {
    let mut fields = vec![format!(
        "{} status={} origin={}",
        summary.task_id, summary.status, summary.origin
    )];
    if let Some(child_agent_id) = &summary.child_agent_id {
        fields.push(format!("agent={child_agent_id}"));
    }
    if let Some(summary_text) = &summary.summary {
        fields.push(format!("summary={summary_text}"));
    }
    fields.join(" ")
}

fn render_task_record_line(verb: &str, record: &TaskRecord) -> String {
    let mut lines = vec![format!(
        "{verb} task {} status={} origin={}",
        record.summary.task_id, record.summary.status, record.summary.origin
    )];
    lines.push(format!("role={}", record.spec.role));
    lines.push(format!("prompt={}", record.spec.prompt));
    if let Some(summary) = &record.summary.summary {
        lines.push(format!("summary={summary}"));
    }
    if let Some(child_agent_id) = &record.summary.child_agent_id {
        lines.push(format!("agent={child_agent_id}"));
    }
    if !record.claimed_files.is_empty() {
        lines.push(format!("claimed_files={}", record.claimed_files.join(",")));
    }
    lines.join("\n")
}

fn build_tool_result<T>(
    call_id: ToolCallId,
    tool_name: &str,
    text: String,
    content: T,
) -> Result<ToolResult>
where
    T: Serialize,
{
    let structured = serde_json::to_value(&content)
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
    Ok(ToolResult {
        id: call_id.clone(),
        call_id: CallId::from(&call_id),
        tool_name: ToolName::from(tool_name),
        parts: vec![MessagePart::text(text)],
        attachments: Vec::new(),
        structured_content: Some(structured.clone()),
        continuation: None,
        metadata: Some(structured),
        is_error: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AgentCancelTool, AgentListTool, AgentResumeTool, AgentSendTool, AgentSpawnTool,
        AgentTaskInput, AgentWaitTool, SubagentExecutor, SubagentInputDelivery, SubagentLaunchSpec,
        SubagentParentContext, TaskCreateTool, TaskCreateToolInput, TaskGetTool, TaskGetToolInput,
        TaskListTool, TaskListToolInput, TaskManager, TaskStopTool, TaskStopToolInput,
        TaskUpdateTool, TaskUpdateToolInput,
    };
    use crate::{Result, Tool, ToolError, ToolExecutionContext, ToolRegistry};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Notify;
    use types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentSessionId, AgentStatus, AgentWaitMode,
        AgentWaitRequest, AgentWaitResponse, Message, MessagePart, MessageRole, SessionId, TaskId,
        TaskOrigin, TaskRecord, TaskStatus, TaskSummaryRecord, ToolCallId, ToolName,
    };

    fn workspace_context(root: &Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct FakeExecutor {
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        handles: BTreeMap<AgentId, AgentHandle>,
        results: BTreeMap<AgentId, AgentResultEnvelope>,
        wait_any_queue: Vec<AgentId>,
        sent: Vec<(AgentId, SubagentInputDelivery, Message)>,
        resumed: Vec<AgentId>,
        cancelled: Vec<AgentId>,
        spawned_launches: Vec<SubagentLaunchSpec>,
    }

    #[derive(Default)]
    struct FakeTaskManager {
        state: Mutex<BTreeMap<TaskId, TaskRecord>>,
    }

    impl FakeTaskManager {
        fn insert(&self, record: TaskRecord) {
            self.state
                .lock()
                .unwrap()
                .insert(record.summary.task_id.clone(), record);
        }
    }

    #[async_trait]
    impl TaskManager for FakeTaskManager {
        async fn create_task(
            &self,
            parent: SubagentParentContext,
            task: types::AgentTaskSpec,
            status: TaskStatus,
        ) -> Result<TaskRecord> {
            let record = TaskRecord {
                summary: TaskSummaryRecord {
                    task_id: task.task_id.clone(),
                    session_id: parent
                        .session_id
                        .unwrap_or_else(|| SessionId::from("session_root")),
                    agent_session_id: parent
                        .agent_session_id
                        .unwrap_or_else(|| AgentSessionId::from("agent_session_root")),
                    role: task.role.clone(),
                    origin: task.origin,
                    status,
                    parent_agent_id: parent.parent_agent_id,
                    child_agent_id: None,
                    summary: Some(task.prompt.clone()),
                },
                spec: task,
                claimed_files: Vec::new(),
                result: None,
                error: None,
            };
            self.insert(record.clone());
            Ok(record)
        }

        async fn get_task(
            &self,
            _parent: SubagentParentContext,
            task_id: &TaskId,
        ) -> Result<TaskRecord> {
            self.state
                .lock()
                .unwrap()
                .get(task_id)
                .cloned()
                .ok_or_else(|| ToolError::invalid(format!("unknown task id: {task_id}")))
        }

        async fn list_tasks(
            &self,
            _parent: SubagentParentContext,
            include_closed: bool,
        ) -> Result<Vec<TaskSummaryRecord>> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .values()
                .filter(|record| include_closed || !record.summary.status.is_terminal())
                .map(|record| record.summary.clone())
                .collect())
        }

        async fn update_task(
            &self,
            _parent: SubagentParentContext,
            task_id: TaskId,
            status: Option<TaskStatus>,
            summary: Option<String>,
        ) -> Result<TaskRecord> {
            let mut state = self.state.lock().unwrap();
            let record = state
                .get_mut(&task_id)
                .ok_or_else(|| ToolError::invalid(format!("unknown task id: {task_id}")))?;
            if let Some(status) = status {
                record.summary.status = status;
            }
            if let Some(summary) = summary {
                record.summary.summary = Some(summary);
            }
            Ok(record.clone())
        }

        async fn stop_task(
            &self,
            _parent: SubagentParentContext,
            task_id: TaskId,
            reason: Option<String>,
        ) -> Result<TaskRecord> {
            let mut state = self.state.lock().unwrap();
            let record = state
                .get_mut(&task_id)
                .ok_or_else(|| ToolError::invalid(format!("unknown task id: {task_id}")))?;
            record.summary.status = TaskStatus::Cancelled;
            record.error = reason;
            Ok(record.clone())
        }
    }

    struct BlockingWaitExecutor {
        handles: Vec<AgentHandle>,
        release: Arc<Notify>,
    }

    fn sample_task_record(task_id: &str, status: TaskStatus, summary: Option<&str>) -> TaskRecord {
        let task_id = TaskId::from(task_id);
        TaskRecord {
            summary: TaskSummaryRecord {
                task_id: task_id.clone(),
                session_id: SessionId::from("session_root"),
                agent_session_id: AgentSessionId::from("agent_session_root"),
                role: "reviewer".to_string(),
                origin: TaskOrigin::AgentCreated,
                status,
                parent_agent_id: Some(AgentId::from("agent_parent")),
                child_agent_id: None,
                summary: summary.map(ToString::to_string),
            },
            spec: types::AgentTaskSpec {
                task_id,
                role: "reviewer".to_string(),
                prompt: "inspect workspace".to_string(),
                origin: TaskOrigin::AgentCreated,
                steer: None,
                allowed_tools: vec![ToolName::from("read")],
                requested_write_set: vec!["src/lib.rs".to_string()],
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            },
            claimed_files: Vec::new(),
            result: None,
            error: None,
        }
    }

    #[async_trait]
    impl SubagentExecutor for FakeExecutor {
        async fn spawn(
            &self,
            _parent: SubagentParentContext,
            tasks: Vec<SubagentLaunchSpec>,
        ) -> Result<Vec<AgentHandle>> {
            let mut state = self.state.lock().unwrap();
            let mut handles = Vec::new();
            for launch in tasks {
                state.spawned_launches.push(launch.clone());
                let task = launch.task;
                let agent_id = AgentId::from(format!("agent_{}", task.task_id));
                let handle = AgentHandle {
                    agent_id: agent_id.clone(),
                    parent_agent_id: Some(AgentId::from("agent_parent")),
                    session_id: SessionId::from(format!("run_{}", task.task_id)),
                    agent_session_id: AgentSessionId::from(format!("session_{}", task.task_id)),
                    task_id: task.task_id.clone(),
                    role: task.role.clone(),
                    status: AgentStatus::Running,
                };
                state.handles.insert(agent_id.clone(), handle.clone());
                state.results.insert(
                    agent_id.clone(),
                    AgentResultEnvelope {
                        agent_id: agent_id.clone(),
                        task_id: task.task_id.clone(),
                        status: if task.role == "failing" {
                            AgentStatus::Failed
                        } else {
                            AgentStatus::Completed
                        },
                        summary: format!("summary {}", task.task_id),
                        text: format!("text {}", task.task_id),
                        artifacts: Vec::new(),
                        claimed_files: task.requested_write_set.clone(),
                        structured_payload: Some(json!({"role": task.role})),
                    },
                );
                state.wait_any_queue.push(agent_id.clone());
                handles.push(handle);
            }
            Ok(handles)
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
            message: Message,
            delivery: SubagentInputDelivery,
        ) -> Result<AgentHandle> {
            let mut state = self.state.lock().unwrap();
            state.sent.push((agent_id.clone(), delivery, message));
            Ok(state.handles.get(&agent_id).cloned().unwrap())
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            request: AgentWaitRequest,
        ) -> Result<AgentWaitResponse> {
            let mut state = self.state.lock().unwrap();
            let requested = request.agent_ids.into_iter().collect::<BTreeSet<_>>();
            let completed_ids = match request.mode {
                AgentWaitMode::All => requested.iter().cloned().collect::<Vec<_>>(),
                AgentWaitMode::Any => state
                    .wait_any_queue
                    .iter()
                    .find(|agent_id| requested.contains(*agent_id))
                    .cloned()
                    .into_iter()
                    .collect(),
            };
            state
                .wait_any_queue
                .retain(|agent_id| !completed_ids.contains(agent_id));
            Ok(AgentWaitResponse {
                completed: completed_ids
                    .iter()
                    .filter_map(|agent_id| state.handles.get(agent_id).cloned())
                    .map(|mut handle| {
                        handle.status = state
                            .results
                            .get(&handle.agent_id)
                            .map(|result| result.status.clone())
                            .unwrap_or(handle.status.clone());
                        handle
                    })
                    .collect(),
                pending: requested
                    .iter()
                    .filter(|agent_id| !completed_ids.contains(agent_id))
                    .filter_map(|agent_id| state.handles.get(agent_id).cloned())
                    .collect(),
                results: completed_ids
                    .iter()
                    .filter_map(|agent_id| state.results.get(agent_id).cloned())
                    .collect(),
            })
        }

        async fn resume(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
        ) -> Result<AgentHandle> {
            let mut state = self.state.lock().unwrap();
            state.resumed.push(agent_id.clone());
            let handle = state
                .handles
                .get_mut(&agent_id)
                .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
            handle.status = AgentStatus::Queued;
            Ok(handle.clone())
        }

        async fn list(&self, _parent: SubagentParentContext) -> Result<Vec<AgentHandle>> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .handles
                .values()
                .cloned()
                .collect())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
            _reason: Option<String>,
        ) -> Result<AgentHandle> {
            let mut state = self.state.lock().unwrap();
            state.cancelled.push(agent_id.clone());
            if let Some(result) = state.results.get_mut(&agent_id) {
                result.status = AgentStatus::Cancelled;
            }
            let handle = state.handles.get_mut(&agent_id).unwrap();
            handle.status = AgentStatus::Cancelled;
            Ok(handle.clone())
        }
    }

    #[async_trait]
    impl SubagentExecutor for BlockingWaitExecutor {
        async fn spawn(
            &self,
            _parent: SubagentParentContext,
            _tasks: Vec<SubagentLaunchSpec>,
        ) -> Result<Vec<AgentHandle>> {
            unreachable!("blocking wait executor does not spawn agents")
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _message: Message,
            _delivery: SubagentInputDelivery,
        ) -> Result<AgentHandle> {
            unreachable!("blocking wait executor does not send messages")
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            request: AgentWaitRequest,
        ) -> Result<AgentWaitResponse> {
            self.release.notified().await;
            Ok(AgentWaitResponse {
                completed: self
                    .handles
                    .iter()
                    .filter(|handle| request.agent_ids.contains(&handle.agent_id))
                    .cloned()
                    .collect(),
                pending: Vec::new(),
                results: Vec::new(),
            })
        }

        async fn resume(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
        ) -> Result<AgentHandle> {
            unreachable!("blocking wait executor does not resume agents")
        }

        async fn list(&self, _parent: SubagentParentContext) -> Result<Vec<AgentHandle>> {
            Ok(self.handles.clone())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _reason: Option<String>,
        ) -> Result<AgentHandle> {
            unreachable!("blocking wait executor does not cancel agents")
        }
    }

    #[tokio::test]
    async fn task_create_creates_standalone_record() {
        let manager = Arc::new(FakeTaskManager::default());
        let tool = TaskCreateTool::new(manager);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskCreateToolInput {
                    task: AgentTaskInput {
                        task_id: Some("inspect".to_string()),
                        role: Some("explorer".to_string()),
                        prompt: "inspect workspace".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: vec!["src/lib.rs".to_string()],
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                    status: Some(TaskStatus::Open),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["summary"]["task_id"], "inspect");
        assert_eq!(structured["summary"]["status"], "open");
        assert_eq!(structured["summary"]["origin"], "agent_created");
    }

    #[tokio::test]
    async fn task_list_and_get_round_trip_records() {
        let manager = Arc::new(FakeTaskManager::default());
        manager.insert(sample_task_record(
            "task_open",
            TaskStatus::Open,
            Some("inspect workspace"),
        ));
        manager.insert(sample_task_record(
            "task_done",
            TaskStatus::Completed,
            Some("done"),
        ));
        let list = TaskListTool::new(manager.clone())
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskListToolInput {
                    include_closed: false,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let list_items = list.structured_content.unwrap().as_array().unwrap().clone();
        assert_eq!(list_items.len(), 1);
        assert_eq!(list_items[0]["task_id"], "task_open");

        let get = TaskGetTool::new(manager)
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskGetToolInput {
                    task_id: TaskId::from("task_done"),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = get.structured_content.unwrap();
        assert_eq!(structured["summary"]["task_id"], "task_done");
        assert_eq!(structured["summary"]["status"], "completed");
    }

    #[tokio::test]
    async fn task_update_requires_status_or_summary() {
        let manager = Arc::new(FakeTaskManager::default());
        let tool = TaskUpdateTool::new(manager);
        let error = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskUpdateToolInput {
                    task_id: TaskId::from("task_1"),
                    status: None,
                    summary: Some("   ".to_string()),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires at least one of status or summary")
        );
    }

    #[tokio::test]
    async fn task_update_mutates_existing_record() {
        let manager = Arc::new(FakeTaskManager::default());
        manager.insert(sample_task_record(
            "task_1",
            TaskStatus::Open,
            Some("before"),
        ));
        let tool = TaskUpdateTool::new(manager);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskUpdateToolInput {
                    task_id: TaskId::from("task_1"),
                    status: Some(TaskStatus::Running),
                    summary: Some("after".to_string()),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["summary"]["status"], "running");
        assert_eq!(structured["summary"]["summary"], "after");
    }

    #[tokio::test]
    async fn task_stop_marks_manual_record_cancelled() {
        let manager = Arc::new(FakeTaskManager::default());
        manager.insert(sample_task_record(
            "task_1",
            TaskStatus::Running,
            Some("before"),
        ));
        let tool = TaskStopTool::new(manager);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskStopToolInput {
                    task_id: TaskId::from("task_1"),
                    reason: Some("operator stop".to_string()),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["summary"]["status"], "cancelled");
        assert_eq!(structured["error"], "operator stop");
    }

    #[tokio::test]
    async fn agent_control_tools_forward_to_executor() {
        let executor = Arc::new(FakeExecutor::default());
        let spawn = AgentSpawnTool::new(executor.clone());
        let send = AgentSendTool::new(executor.clone());
        let wait = AgentWaitTool::new(executor.clone());
        let resume = AgentResumeTool::new(executor.clone());
        let list = AgentListTool::new(executor.clone());
        let cancel = AgentCancelTool::new(executor.clone());

        let spawned = spawn
            .execute(
                ToolCallId::new(),
                json!({"agent_type":"explorer","message":"inspect"}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let agent_id = AgentId::from(
            spawned.structured_content.unwrap()["agent_id"]
                .as_str()
                .unwrap(),
        );

        send.execute(
            ToolCallId::new(),
            json!({
                "target": agent_id,
                "message": "focus tests"
            }),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let waited = wait
            .execute(
                ToolCallId::new(),
                json!({"targets":[agent_id]}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            waited.structured_content.unwrap()["completed"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let listed = list
            .execute(
                ToolCallId::new(),
                json!({}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            listed.structured_content.unwrap().as_array().unwrap().len(),
            1
        );

        let resumed = resume
            .execute(
                ToolCallId::new(),
                json!({"id": agent_id}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.structured_content.unwrap()["status"], "queued");

        let cancelled = cancel
            .execute(
                ToolCallId::new(),
                json!({"target": agent_id}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(cancelled.structured_content.unwrap()["status"], "cancelled");
        let state = executor.state.lock().unwrap();
        assert_eq!(state.sent.len(), 1);
        assert_eq!(state.sent[0].1, SubagentInputDelivery::Queue);
        assert_eq!(state.resumed.len(), 1);
    }

    #[tokio::test]
    async fn spawn_agent_uses_codex_style_input_and_records_launch_overrides() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());

        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "agent_type": "reviewer",
                    "message": "Review the current patch.",
                    "items": [
                        {"type": "text", "text": "Focus on regressions."},
                        {"type": "mention", "name": "connector", "path": "app://tool-registry"}
                    ],
                    "model": "gpt-5.4",
                    "reasoning_effort": "high"
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["role"], "reviewer");

        let state = executor.state.lock().unwrap();
        assert_eq!(state.spawned_launches.len(), 1);
        let launch = &state.spawned_launches[0];
        assert_eq!(launch.task.role, "reviewer");
        assert!(!launch.fork_context);
        assert_eq!(launch.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(launch.reasoning_effort.as_deref(), Some("high"));
        assert!(launch.task.task_id.as_str().starts_with("spawn_"));
        assert_eq!(launch.task.origin, TaskOrigin::ChildAgentBacked);
        assert!(launch.task.prompt.contains("Review the current patch."));
        assert!(launch.task.prompt.contains("Focus on regressions."));
        assert!(launch.task.prompt.contains("[mention]"));
        assert_eq!(launch.initial_input.role, MessageRole::User);
        assert_eq!(launch.initial_input.parts.len(), 3);
        assert!(matches!(
            launch.initial_input.parts.get(2),
            Some(MessagePart::Reference {
                kind,
                name,
                uri,
                text,
            }) if kind == "mention"
                && name.as_deref() == Some("connector")
                && uri.as_deref() == Some("app://tool-registry")
                && text.is_none()
        ));
        assert_eq!(
            launch.initial_input.text_content(),
            "Review the current patch.\nFocus on regressions.\nconnector"
        );
    }

    #[tokio::test]
    async fn spawn_agent_plain_item_without_uri_becomes_reference_part() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());

        tool.execute(
            ToolCallId::new(),
            json!({
                "agent_type": "reviewer",
                "items": [
                    {
                        "type": "skill",
                        "name": "openai-docs",
                        "text": "Use the official docs only"
                    }
                ]
            }),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        let launch = &state.spawned_launches[0];
        assert!(matches!(
            launch.initial_input.parts.first(),
            Some(MessagePart::Reference {
                kind,
                name,
                uri,
                text,
            }) if kind == "skill"
                && name.as_deref() == Some("openai-docs")
                && uri.is_none()
                && text.as_deref() == Some("Use the official docs only")
        ));
    }

    #[tokio::test]
    async fn spawn_agent_local_image_items_become_image_parts() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sample.png"), b"\x89PNG\r\n\x1a\npayload").unwrap();

        tool.execute(
            ToolCallId::new(),
            json!({
                "agent_type": "reviewer",
                "items": [
                    {"type": "local_image", "path": "sample.png", "text": "latest failure screenshot"}
                ]
            }),
            &workspace_context(dir.path()),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        let launch = &state.spawned_launches[0];
        assert!(matches!(
            launch.initial_input.parts.first(),
            Some(MessagePart::Image { mime_type, .. }) if mime_type == "image/png"
        ));
        assert_eq!(
            launch.initial_input.text_content(),
            "[image:image/png]\nlatest failure screenshot"
        );
    }

    #[tokio::test]
    async fn spawn_agent_image_url_items_become_remote_image_parts() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());

        tool.execute(
            ToolCallId::new(),
            json!({
                "agent_type": "reviewer",
                "items": [
                    {
                        "type": "image_url",
                        "image_url": "https://example.com/failure.png",
                        "text": "latest CI screenshot"
                    }
                ]
            }),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        let launch = &state.spawned_launches[0];
        assert!(matches!(
            launch.initial_input.parts.first(),
            Some(MessagePart::ImageUrl { url, .. }) if url == "https://example.com/failure.png"
        ));
        assert_eq!(
            launch.initial_input.text_content(),
            "[image_url:https://example.com/failure.png]\nlatest CI screenshot"
        );
    }

    #[tokio::test]
    async fn spawn_agent_local_file_items_become_file_parts() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf"), b"%PDF-1.7\npayload").unwrap();

        tool.execute(
            ToolCallId::new(),
            json!({
                "agent_type": "reviewer",
                "items": [
                    {
                        "type": "local_file",
                        "path": "report.pdf",
                        "text": "Summarize the findings"
                    }
                ]
            }),
            &workspace_context(dir.path()),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        let launch = &state.spawned_launches[0];
        assert!(matches!(
            launch.initial_input.parts.first(),
            Some(MessagePart::File {
                file_name,
                mime_type,
                data_base64: Some(_),
                uri: Some(uri),
            }) if file_name.as_deref() == Some("report.pdf")
                && mime_type.as_deref() == Some("application/pdf")
                && uri == "report.pdf"
        ));
        assert_eq!(
            launch.initial_input.text_content(),
            "[file:report.pdf application/pdf report.pdf]\nSummarize the findings"
        );
    }

    #[tokio::test]
    async fn spawn_agent_remote_file_items_become_file_url_parts() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());

        tool.execute(
            ToolCallId::new(),
            json!({
                "agent_type": "reviewer",
                "items": [
                    {
                        "type": "file",
                        "path": "https://example.com/reports/monthly.pdf",
                        "text": "Summarize the findings"
                    }
                ]
            }),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        let launch = &state.spawned_launches[0];
        assert!(matches!(
            launch.initial_input.parts.first(),
            Some(MessagePart::File {
                file_name,
                mime_type,
                data_base64: None,
                uri: Some(uri),
            }) if file_name.as_deref() == Some("monthly.pdf")
                && mime_type.as_deref() == Some("application/pdf")
                && uri == "https://example.com/reports/monthly.pdf"
        ));
        assert_eq!(
            launch.initial_input.text_content(),
            "[file:monthly.pdf application/pdf https://example.com/reports/monthly.pdf]\nSummarize the findings"
        );
    }

    #[tokio::test]
    async fn spawn_agent_rejects_remote_urls_for_local_file_items() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor);

        let error = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "agent_type": "reviewer",
                    "items": [
                        {
                            "type": "local_file",
                            "path": "https://example.com/reports/monthly.pdf"
                        }
                    ]
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("use type=file for remote URLs"));
    }

    #[tokio::test]
    async fn spawn_agent_forwards_fork_context_to_executor() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSpawnTool::new(executor.clone());
        tool.execute(
            ToolCallId::new(),
            json!({"fork_context": true, "message": "continue"}),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        assert_eq!(state.spawned_launches.len(), 1);
        assert!(state.spawned_launches[0].fork_context);
    }

    #[tokio::test]
    async fn send_input_forwards_interrupt_delivery_to_executor() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = AgentSendTool::new(executor.clone());
        executor.state.lock().unwrap().handles.insert(
            AgentId::from("agent_a"),
            AgentHandle {
                agent_id: AgentId::from("agent_a"),
                parent_agent_id: Some(AgentId::from("agent_parent")),
                session_id: SessionId::from("run_agent_a"),
                agent_session_id: AgentSessionId::from("session_agent_a"),
                task_id: TaskId::from("task_a"),
                role: "worker".to_string(),
                status: AgentStatus::Running,
            },
        );
        tool.execute(
            ToolCallId::new(),
            json!({"target":"agent_a","interrupt":true,"message":"focus"}),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        assert_eq!(state.sent.len(), 1);
        assert_eq!(state.sent[0].0, AgentId::from("agent_a"));
        assert_eq!(state.sent[0].1, SubagentInputDelivery::Interrupt);
        assert_eq!(state.sent[0].2.text_content(), "focus");
    }

    #[tokio::test]
    async fn wait_agent_timeout_returns_pending_handles_without_blocking_forever() {
        let tool = AgentWaitTool::new(Arc::new(BlockingWaitExecutor {
            handles: vec![AgentHandle {
                agent_id: AgentId::from("agent_pending"),
                parent_agent_id: Some(AgentId::from("agent_parent")),
                session_id: SessionId::from("run_pending"),
                agent_session_id: AgentSessionId::from("session_pending"),
                task_id: TaskId::from("pending"),
                role: "worker".to_string(),
                status: AgentStatus::Running,
            }],
            release: Arc::new(Notify::new()),
        }));

        let result = tool
            .execute(
                ToolCallId::new(),
                json!({"targets":["agent_pending"],"timeout_ms":1}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["completed"].as_array().unwrap().len(), 0);
        assert_eq!(structured["pending"].as_array().unwrap().len(), 1);
        let text = match &result.parts[0] {
            types::MessagePart::Text { text } => text,
            other => panic!("unexpected tool result part: {other:?}"),
        };
        assert!(text.contains("timed_out"));
    }

    #[test]
    fn registry_registers_codex_style_agent_controls_without_legacy_aliases() {
        let executor = Arc::new(FakeExecutor::default());
        let mut registry = ToolRegistry::new();
        registry.register(AgentSpawnTool::new(executor.clone()));
        registry.register(AgentSendTool::new(executor.clone()));
        registry.register(AgentWaitTool::new(executor.clone()));
        registry.register(AgentResumeTool::new(executor.clone()));
        registry.register(AgentListTool::new(executor.clone()));
        registry.register(AgentCancelTool::new(executor));

        assert_eq!(registry.specs()[0].name.as_str(), "close_agent");
        assert!(registry.get("agent_spawn").is_none());
        assert!(registry.get("agent_send").is_none());
        assert!(registry.get("agent_wait").is_none());
        assert!(registry.get("agent_list").is_none());
        assert!(registry.get("agent_cancel").is_none());
        assert!(registry.get("spawn_agent").is_some());
        assert!(registry.get("send_input").is_some());
        assert!(registry.get("wait_agent").is_some());
        assert!(registry.get("resume_agent").is_some());
        assert!(registry.get("list_agents").is_some());
        assert!(registry.get("close_agent").is_some());
    }

    #[test]
    fn normalize_task_input_deduplicates_dependency_ids() {
        let task = super::normalize_task_input(
            AgentTaskInput {
                task_id: Some("review".to_string()),
                role: Some("reviewer".to_string()),
                prompt: "review".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: vec![
                    " inspect ".to_string(),
                    "inspect".to_string(),
                    "plan".to_string(),
                ],
                timeout_seconds: None,
            },
            TaskOrigin::AgentCreated,
            None,
        )
        .unwrap();

        assert_eq!(
            task.dependency_ids,
            vec![TaskId::from("inspect"), TaskId::from("plan")]
        );
    }

    #[test]
    fn normalize_task_input_rejects_self_dependency() {
        let error = super::normalize_task_input(
            AgentTaskInput {
                task_id: Some("review".to_string()),
                role: Some("reviewer".to_string()),
                prompt: "review".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: vec!["review".to_string()],
                timeout_seconds: None,
            },
            TaskOrigin::AgentCreated,
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("cannot depend on itself"));
    }
}
