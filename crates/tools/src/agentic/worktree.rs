use crate::HOST_FEATURE_HOST_PROCESS_SURFACES;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use types::{
    AgentId, AgentSessionId, SessionId, TaskId, ToolAvailability, ToolCallId, ToolOutputMode,
    ToolResult, ToolSpec, TurnId, WorktreeId, WorktreeSummaryRecord,
};

pub const PRIMARY_WORKTREE_ID: &str = "worktree_primary";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeRuntimeContext {
    pub session_id: Option<SessionId>,
    pub agent_session_id: Option<AgentSessionId>,
    pub turn_id: Option<TurnId>,
    pub parent_agent_id: Option<AgentId>,
    pub task_id: Option<TaskId>,
    pub active_worktree_id: Option<WorktreeId>,
}

impl From<&ToolExecutionContext> for WorktreeRuntimeContext {
    fn from(ctx: &ToolExecutionContext) -> Self {
        Self {
            session_id: ctx.session_id.clone(),
            agent_session_id: ctx.agent_session_id.clone(),
            turn_id: ctx.turn_id.clone(),
            parent_agent_id: ctx.agent_id.clone(),
            task_id: ctx.task_id.as_deref().map(TaskId::from),
            active_worktree_id: ctx.active_worktree_id.clone(),
        }
    }
}

#[async_trait]
pub trait WorktreeManager: Send + Sync {
    async fn enter_worktree(
        &self,
        runtime: WorktreeRuntimeContext,
        request: WorktreeEnterRequest,
    ) -> Result<WorktreeSummaryRecord>;

    async fn list_worktrees(
        &self,
        runtime: WorktreeRuntimeContext,
        include_inactive: bool,
    ) -> Result<Vec<WorktreeSummaryRecord>>;

    async fn exit_worktree(
        &self,
        runtime: WorktreeRuntimeContext,
        worktree_id: Option<WorktreeId>,
    ) -> Result<WorktreeSummaryRecord>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeEnterRequest {
    pub label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct WorktreeEnterToolInput {
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct WorktreeListToolInput {
    #[serde(default)]
    pub include_inactive: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct WorktreeExitToolInput {
    #[serde(default)]
    pub worktree_id: Option<WorktreeId>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WorktreeEnterToolOutput {
    worktree: WorktreeSummaryRecord,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WorktreeListToolOutput {
    worktrees: Vec<WorktreeSummaryRecord>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct WorktreeExitToolOutput {
    worktree: WorktreeSummaryRecord,
}

#[derive(Clone)]
pub struct WorktreeEnterTool {
    manager: Arc<dyn WorktreeManager>,
}

#[derive(Clone)]
pub struct WorktreeListTool {
    manager: Arc<dyn WorktreeManager>,
}

#[derive(Clone)]
pub struct WorktreeExitTool {
    manager: Arc<dyn WorktreeManager>,
}

impl WorktreeEnterTool {
    #[must_use]
    pub fn new(manager: Arc<dyn WorktreeManager>) -> Self {
        Self { manager }
    }
}

impl WorktreeListTool {
    #[must_use]
    pub fn new(manager: Arc<dyn WorktreeManager>) -> Self {
        Self { manager }
    }
}

impl WorktreeExitTool {
    #[must_use]
    pub fn new(manager: Arc<dyn WorktreeManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for WorktreeEnterTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "worktree_enter",
            "Create and enter a dedicated session worktree. Later file and shell tools inherit the new worktree root until worktree_exit returns to the primary workspace.",
            serde_json::to_value(schema_for!(WorktreeEnterToolInput))
                .expect("worktree_enter schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(WorktreeEnterToolOutput))
                .expect("worktree_enter output schema"),
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
        let input: WorktreeEnterToolInput = serde_json::from_value(arguments)?;
        let worktree = self
            .manager
            .enter_worktree(
                WorktreeRuntimeContext::from(ctx),
                WorktreeEnterRequest { label: input.label },
            )
            .await?;
        Ok(ToolResult::text(
            call_id.clone(),
            "worktree_enter",
            render_worktree_summary("Entered", &worktree),
        )
        .with_structured_content(json!(WorktreeEnterToolOutput { worktree })))
    }
}

#[async_trait]
impl Tool for WorktreeListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "worktree_list",
            "List worktrees visible from the current session. Use include_inactive when you need detached or removed entries too.",
            serde_json::to_value(schema_for!(WorktreeListToolInput)).expect("worktree_list schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(WorktreeListToolOutput))
                .expect("worktree_list output schema"),
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
        let input: WorktreeListToolInput = serde_json::from_value(arguments)?;
        let worktrees = self
            .manager
            .list_worktrees(WorktreeRuntimeContext::from(ctx), input.include_inactive)
            .await?;
        Ok(ToolResult::text(
            call_id.clone(),
            "worktree_list",
            render_worktree_list(&worktrees),
        )
        .with_structured_content(json!(WorktreeListToolOutput { worktrees })))
    }
}

#[async_trait]
impl Tool for WorktreeExitTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "worktree_exit",
            "Exit the current session worktree and return to the primary workspace. When worktree_id is omitted, the active worktree is used.",
            serde_json::to_value(schema_for!(WorktreeExitToolInput)).expect("worktree_exit schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(WorktreeExitToolOutput))
                .expect("worktree_exit output schema"),
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
        let input: WorktreeExitToolInput = serde_json::from_value(arguments)?;
        let worktree = self
            .manager
            .exit_worktree(WorktreeRuntimeContext::from(ctx), input.worktree_id)
            .await?;
        Ok(ToolResult::text(
            call_id.clone(),
            "worktree_exit",
            render_worktree_summary("Exited", &worktree),
        )
        .with_structured_content(json!(WorktreeExitToolOutput { worktree })))
    }
}

fn render_worktree_summary(verb: &str, summary: &WorktreeSummaryRecord) -> String {
    let mut lines = vec![format!("{verb} {} {}", summary.scope, summary.worktree_id)];
    lines.push(format!("status {}", summary.status));
    lines.push(format!("root {}", summary.root.display()));
    if let Some(label) = summary
        .label
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("label {}", label.trim()));
    }
    lines.join("\n")
}

fn render_worktree_list(worktrees: &[WorktreeSummaryRecord]) -> String {
    if worktrees.is_empty() {
        return "No worktrees".to_string();
    }
    worktrees
        .iter()
        .map(|summary| {
            format!(
                "{} [{}] {}",
                summary.worktree_id,
                summary.status,
                summary.root.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
