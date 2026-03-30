use crate::ToolExecutionContext;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use types::{
    AgentHandle, AgentId, AgentResultEnvelope, AgentSessionId, AgentStatus, AgentTaskSpec,
    AgentWaitMode, AgentWaitRequest, AgentWaitResponse, CallId, MessagePart, SessionId, ToolCallId,
    ToolName, ToolOutputMode, ToolResult, ToolSpec, TurnId,
};

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
pub struct TaskToolInput {
    #[serde(flatten)]
    pub task: AgentTaskInput,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TaskToolOutput {
    agent: AgentHandle,
    result: AgentResultEnvelope,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskBatchToolInput {
    pub tasks: Vec<AgentTaskInput>,
    #[serde(default = "default_wait_mode")]
    pub mode: AgentWaitMode,
    #[serde(default)]
    pub stop_on_error: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TaskBatchToolOutput {
    completed: Vec<AgentHandle>,
    pending: Vec<AgentHandle>,
    results: Vec<AgentResultEnvelope>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentSpawnToolInput {
    pub tasks: Vec<AgentTaskInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentSendToolInput {
    pub agent_id: AgentId,
    #[serde(default = "default_message_channel")]
    pub channel: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct AgentCancelToolInput {
    pub agent_id: AgentId,
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_wait_mode() -> AgentWaitMode {
    AgentWaitMode::All
}

fn default_message_channel() -> String {
    "steer".to_string()
}

#[async_trait]
pub trait SubagentExecutor: Send + Sync {
    async fn spawn(
        &self,
        parent: SubagentParentContext,
        tasks: Vec<AgentTaskSpec>,
    ) -> Result<Vec<AgentHandle>>;

    async fn send(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        channel: String,
        payload: Value,
    ) -> Result<AgentHandle>;

    async fn wait(
        &self,
        parent: SubagentParentContext,
        request: AgentWaitRequest,
    ) -> Result<AgentWaitResponse>;

    async fn list(&self, parent: SubagentParentContext) -> Result<Vec<AgentHandle>>;

    async fn cancel(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        reason: Option<String>,
    ) -> Result<AgentHandle>;
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

define_executor_tool!(TaskTool);
define_executor_tool!(TaskBatchTool);
define_executor_tool!(AgentSpawnTool);
define_executor_tool!(AgentSendTool);
define_executor_tool!(AgentWaitTool);
define_executor_tool!(AgentListTool);
define_executor_tool!(AgentCancelTool);

#[async_trait]
impl Tool for TaskTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "task",
            "Spawn one child agent, wait for completion, and return its structured result.",
            serde_json::to_value(schema_for!(TaskToolInput)).expect("task schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(TaskToolOutput)).expect("task output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskToolInput = serde_json::from_value(arguments)?;
        let parent = SubagentParentContext::from(ctx);
        let task = normalize_task_input(input.task, 1)?;
        let mut handles = self.executor.spawn(parent.clone(), vec![task]).await?;
        let agent = handles
            .pop()
            .ok_or_else(|| ToolError::invalid_state("task spawn returned no agent"))?;
        let wait = self
            .executor
            .wait(
                parent,
                AgentWaitRequest {
                    agent_ids: vec![agent.agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await?;
        let result = wait
            .results
            .into_iter()
            .find(|result| result.agent_id == agent.agent_id)
            .ok_or_else(|| ToolError::invalid_state("missing child result"))?;
        build_tool_result(
            call_id,
            "task",
            format!(
                "[task {} status={}]\nsummary> {}\n\n{}",
                agent.role, result.status, result.summary, result.text
            ),
            TaskToolOutput { agent, result },
        )
    }
}

#[async_trait]
impl Tool for TaskBatchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "task_batch",
            "Spawn multiple child agents with dependency-aware scheduling, wait for completion, and return structured results.",
            serde_json::to_value(schema_for!(TaskBatchToolInput))
                .expect("task_batch schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(TaskBatchToolOutput))
                .expect("task_batch output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: TaskBatchToolInput = serde_json::from_value(arguments)?;
        if input.tasks.is_empty() {
            return Err(ToolError::invalid("task_batch requires at least one task"));
        }
        let parent = SubagentParentContext::from(ctx);
        let tasks = input
            .tasks
            .into_iter()
            .enumerate()
            .map(|(index, task)| normalize_task_input(task, index + 1))
            .collect::<Result<Vec<_>>>()?;
        let handles = self.executor.spawn(parent.clone(), tasks).await?;
        let wait = wait_for_batch(
            self.executor.as_ref(),
            parent,
            handles,
            input.mode,
            input.stop_on_error,
        )
        .await?;
        build_tool_result(
            call_id,
            "task_batch",
            render_wait_summary("task_batch", &wait),
            TaskBatchToolOutput {
                completed: wait.completed,
                pending: wait.pending,
                results: wait.results,
            },
        )
    }
}

#[async_trait]
impl Tool for AgentSpawnTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "agent_spawn",
            "Spawn one or more child agents without waiting, honoring in-batch dependencies before they start.",
            serde_json::to_value(schema_for!(AgentSpawnToolInput))
                .expect("agent_spawn schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(Vec<AgentHandle>))
                .expect("agent_spawn output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentSpawnToolInput = serde_json::from_value(arguments)?;
        let tasks = input
            .tasks
            .into_iter()
            .enumerate()
            .map(|(index, task)| normalize_task_input(task, index + 1))
            .collect::<Result<Vec<_>>>()?;
        let handles = self
            .executor
            .spawn(SubagentParentContext::from(ctx), tasks)
            .await?;
        build_tool_result(
            call_id,
            "agent_spawn",
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
impl Tool for AgentSendTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "agent_send",
            "Send a message or steering payload to a child agent.",
            serde_json::to_value(schema_for!(AgentSendToolInput)).expect("agent_send schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentHandle)).expect("agent_send output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentSendToolInput = serde_json::from_value(arguments)?;
        let handle = self
            .executor
            .send(
                SubagentParentContext::from(ctx),
                input.agent_id,
                input.channel,
                input.payload,
            )
            .await?;
        build_tool_result(call_id, "agent_send", render_handle_line(&handle), handle)
    }
}

#[async_trait]
impl Tool for AgentWaitTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "agent_wait",
            "Wait for one or more child agents to reach a terminal state.",
            serde_json::to_value(schema_for!(AgentWaitRequest)).expect("agent_wait schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentWaitResponse)).expect("agent_wait output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let request: AgentWaitRequest = serde_json::from_value(arguments)?;
        let wait = self
            .executor
            .wait(SubagentParentContext::from(ctx), request)
            .await?;
        build_tool_result(
            call_id,
            "agent_wait",
            render_wait_summary("agent_wait", &wait),
            wait,
        )
    }
}

#[async_trait]
impl Tool for AgentListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "agent_list",
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
            serde_json::to_value(schema_for!(Vec<AgentHandle>)).expect("agent_list output schema"),
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
            "agent_list",
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
impl Tool for AgentCancelTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "agent_cancel",
            "Cancel a running child agent.",
            serde_json::to_value(schema_for!(AgentCancelToolInput)).expect("agent_cancel schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(AgentHandle)).expect("agent_cancel output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: AgentCancelToolInput = serde_json::from_value(arguments)?;
        let handle = self
            .executor
            .cancel(
                SubagentParentContext::from(ctx),
                input.agent_id,
                input.reason,
            )
            .await?;
        build_tool_result(call_id, "agent_cancel", render_handle_line(&handle), handle)
    }
}

fn normalize_task_input(input: AgentTaskInput, ordinal: usize) -> Result<AgentTaskSpec> {
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
        .unwrap_or_else(|| format!("task_{ordinal}"));
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

fn normalize_paths(paths: Vec<String>) -> Vec<String> {
    let mut unique = BTreeSet::new();
    paths
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .filter(|path| unique.insert(path.clone()))
        .collect()
}

fn normalize_dependency_ids(dependency_ids: Vec<String>) -> Vec<String> {
    let mut unique = BTreeSet::new();
    dependency_ids
        .into_iter()
        .map(|dependency_id| dependency_id.trim().to_string())
        .filter(|dependency_id| !dependency_id.is_empty())
        .filter(|dependency_id| unique.insert(dependency_id.clone()))
        .collect()
}

async fn wait_for_batch(
    executor: &dyn SubagentExecutor,
    parent: SubagentParentContext,
    handles: Vec<AgentHandle>,
    mode: AgentWaitMode,
    stop_on_error: bool,
) -> Result<AgentWaitResponse> {
    let agent_ids = handles
        .iter()
        .map(|handle| handle.agent_id.clone())
        .collect::<Vec<_>>();
    if !stop_on_error {
        return executor
            .wait(parent, AgentWaitRequest { agent_ids, mode })
            .await;
    }

    let mut remaining = agent_ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut completed = BTreeMap::new();
    let mut results = BTreeMap::new();
    while !remaining.is_empty() {
        let wave = executor
            .wait(
                parent.clone(),
                AgentWaitRequest {
                    agent_ids: remaining.iter().cloned().collect(),
                    mode: AgentWaitMode::Any,
                },
            )
            .await?;
        if wave.completed.is_empty() {
            return Err(ToolError::invalid_state(
                "agent_wait(any) returned no terminal agent",
            ));
        }
        let saw_failure = wave
            .results
            .iter()
            .any(|result| matches!(result.status, AgentStatus::Failed | AgentStatus::Cancelled));
        for handle in wave.completed {
            remaining.remove(&handle.agent_id);
            completed.insert(handle.agent_id.clone(), handle);
        }
        for result in wave.results {
            results.insert(result.agent_id.clone(), result);
        }
        if saw_failure {
            for agent_id in remaining.iter().cloned().collect::<Vec<_>>() {
                let _ = executor
                    .cancel(
                        parent.clone(),
                        agent_id.clone(),
                        Some("task_batch stop_on_error".to_string()),
                    )
                    .await;
            }
            let tail_ids = remaining.iter().cloned().collect::<Vec<_>>();
            if !tail_ids.is_empty() {
                let tail = executor
                    .wait(
                        parent.clone(),
                        AgentWaitRequest {
                            agent_ids: tail_ids,
                            mode: AgentWaitMode::All,
                        },
                    )
                    .await?;
                for handle in tail.completed {
                    completed.insert(handle.agent_id.clone(), handle);
                }
                for result in tail.results {
                    results.insert(result.agent_id.clone(), result);
                }
            }
            remaining.clear();
        }
    }

    let completed_vec = agent_ids
        .iter()
        .filter_map(|agent_id| completed.remove(agent_id))
        .collect::<Vec<_>>();
    let results_vec = agent_ids
        .iter()
        .filter_map(|agent_id| results.remove(agent_id))
        .collect::<Vec<_>>();
    Ok(match mode {
        AgentWaitMode::Any => AgentWaitResponse {
            completed: completed_vec.into_iter().take(1).collect(),
            pending: Vec::new(),
            results: results_vec.into_iter().take(1).collect(),
        },
        AgentWaitMode::All => AgentWaitResponse {
            completed: completed_vec,
            pending: Vec::new(),
            results: results_vec,
        },
    })
}

fn render_wait_summary(tool_name: &str, wait: &AgentWaitResponse) -> String {
    let mut lines = vec![format!(
        "[{tool_name} completed={} pending={} results={}]",
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

fn render_result_line(result: &AgentResultEnvelope) -> String {
    format!(
        "result {} status={} summary={}",
        result.agent_id, result.status, result.summary
    )
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
        structured_content: Some(structured.clone()),
        metadata: Some(structured),
        is_error: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AgentCancelTool, AgentListTool, AgentSendTool, AgentSpawnTool, AgentTaskInput,
        AgentWaitTool, SubagentExecutor, SubagentParentContext, TaskBatchTool, TaskBatchToolInput,
        TaskTool, TaskToolInput,
    };
    use crate::{Result, Tool, ToolExecutionContext};
    use async_trait::async_trait;
    use serde_json::Value;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Arc, Mutex};
    use types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentSessionId, AgentStatus, AgentTaskSpec,
        AgentWaitMode, AgentWaitRequest, AgentWaitResponse, SessionId, ToolCallId, ToolName,
    };

    #[derive(Default)]
    struct FakeExecutor {
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        handles: BTreeMap<AgentId, AgentHandle>,
        results: BTreeMap<AgentId, AgentResultEnvelope>,
        wait_any_queue: Vec<AgentId>,
        sent: Vec<(AgentId, String, serde_json::Value)>,
        cancelled: Vec<AgentId>,
        spawned_tasks: Vec<AgentTaskSpec>,
    }

    #[async_trait]
    impl SubagentExecutor for FakeExecutor {
        async fn spawn(
            &self,
            _parent: SubagentParentContext,
            tasks: Vec<AgentTaskSpec>,
        ) -> Result<Vec<AgentHandle>> {
            let mut state = self.state.lock().unwrap();
            let mut handles = Vec::new();
            for task in tasks {
                state.spawned_tasks.push(task.clone());
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
            channel: String,
            payload: Value,
        ) -> Result<AgentHandle> {
            let mut state = self.state.lock().unwrap();
            state.sent.push((agent_id.clone(), channel, payload));
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

    #[tokio::test]
    async fn task_tool_spawns_and_waits_for_single_agent() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = TaskTool::new(executor);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskToolInput {
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
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["agent"]["task_id"], "inspect");
        assert_eq!(structured["result"]["status"], "completed");
    }

    #[tokio::test]
    async fn task_batch_fans_out_and_joins() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = TaskBatchTool::new(executor);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "tasks": [
                        {"task_id":"a","role":"explorer","prompt":"inspect a"},
                        {"task_id":"b","role":"reviewer","prompt":"inspect b"}
                    ],
                    "mode": "all",
                    "stop_on_error": false
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["completed"].as_array().unwrap().len(), 2);
        assert_eq!(structured["results"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn task_batch_preserves_dependency_ids_for_executor_scheduling() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = TaskBatchTool::new(executor.clone());
        tool.execute(
            ToolCallId::new(),
            json!({
                "tasks": [
                    {"task_id":"inspect","role":"explorer","prompt":"inspect"},
                    {"task_id":"review","role":"reviewer","prompt":"review","dependency_ids":["inspect"," inspect "]}
                ],
                "mode": "all",
                "stop_on_error": false
            }),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let state = executor.state.lock().unwrap();
        let review = state
            .spawned_tasks
            .iter()
            .find(|task| task.task_id == "review")
            .expect("review task should be forwarded to the executor");
        assert_eq!(review.dependency_ids, vec!["inspect"]);
    }

    #[tokio::test]
    async fn task_batch_stop_on_error_cancels_remaining_agents() {
        let executor = Arc::new(FakeExecutor::default());
        let tool = TaskBatchTool::new(executor.clone());
        let _ = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskBatchToolInput {
                    tasks: vec![
                        AgentTaskInput {
                            task_id: Some("fail".to_string()),
                            role: Some("failing".to_string()),
                            prompt: "fail".to_string(),
                            steer: None,
                            allowed_tools: Vec::new(),
                            requested_write_set: Vec::new(),
                            dependency_ids: Vec::new(),
                            timeout_seconds: None,
                        },
                        AgentTaskInput {
                            task_id: Some("other".to_string()),
                            role: Some("worker".to_string()),
                            prompt: "other".to_string(),
                            steer: None,
                            allowed_tools: Vec::new(),
                            requested_write_set: Vec::new(),
                            dependency_ids: Vec::new(),
                            timeout_seconds: None,
                        },
                    ],
                    mode: AgentWaitMode::All,
                    stop_on_error: true,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(
            executor
                .state
                .lock()
                .unwrap()
                .cancelled
                .contains(&AgentId::from("agent_other"))
        );
    }

    #[tokio::test]
    async fn agent_control_tools_forward_to_executor() {
        let executor = Arc::new(FakeExecutor::default());
        let spawn = AgentSpawnTool::new(executor.clone());
        let send = AgentSendTool::new(executor.clone());
        let wait = AgentWaitTool::new(executor.clone());
        let list = AgentListTool::new(executor.clone());
        let cancel = AgentCancelTool::new(executor.clone());

        let spawned = spawn
            .execute(
                ToolCallId::new(),
                json!({"tasks":[{"task_id":"agent_a","prompt":"inspect","role":"explorer"}]}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let agent_id = AgentId::from(
            spawned.structured_content.unwrap()[0]["agent_id"]
                .as_str()
                .unwrap(),
        );

        send.execute(
            ToolCallId::new(),
            json!({
                "agent_id": agent_id,
                "channel": "steer",
                "payload": {"message":"focus tests"}
            }),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let waited = wait
            .execute(
                ToolCallId::new(),
                json!({"agent_ids":[agent_id],"mode":"all"}),
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

        let cancelled = cancel
            .execute(
                ToolCallId::new(),
                json!({"agent_id":"agent_agent_a"}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(cancelled.structured_content.unwrap()["status"], "cancelled");
        assert_eq!(executor.state.lock().unwrap().sent.len(), 1);
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
            1,
        )
        .unwrap();

        assert_eq!(task.dependency_ids, vec!["inspect", "plan"]);
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
            1,
        )
        .unwrap_err();

        assert!(error.to_string().contains("cannot depend on itself"));
    }
}
