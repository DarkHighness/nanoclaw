use crate::backend::task_history;
use crate::ui::LoadedTask;
use agent::tools::{Result as ToolResult, SubagentExecutor, SubagentParentContext, TaskManager};
use agent::types::{
    AgentSessionId, SessionEventEnvelope, SessionEventKind, SessionId, TaskId, TaskRecord,
    TaskStatus, TaskSummaryRecord,
};
use async_trait::async_trait;
use std::sync::Arc;
use store::SessionStore;

#[derive(Clone)]
pub struct SessionTaskManager {
    store: Arc<dyn SessionStore>,
    subagent_executor: Arc<dyn SubagentExecutor>,
}

impl SessionTaskManager {
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, subagent_executor: Arc<dyn SubagentExecutor>) -> Self {
        Self {
            store,
            subagent_executor,
        }
    }

    fn require_parent_session(
        parent: &SubagentParentContext,
    ) -> ToolResult<(SessionId, AgentSessionId)> {
        let session_id = parent.session_id.clone().ok_or_else(|| {
            agent::tools::ToolError::invalid_state("task tools require an attached runtime session")
        })?;
        let agent_session_id = parent.agent_session_id.clone().ok_or_else(|| {
            agent::tools::ToolError::invalid_state(
                "task tools require an attached runtime agent session",
            )
        })?;
        Ok((session_id, agent_session_id))
    }

    async fn append_task_event(
        &self,
        parent: &SubagentParentContext,
        event: SessionEventKind,
    ) -> ToolResult<()> {
        let (session_id, agent_session_id) = Self::require_parent_session(parent)?;
        self.store
            .append(SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                parent.turn_id.clone(),
                None,
                event,
            ))
            .await
            .map_err(|error| agent::tools::ToolError::invalid_state(error.to_string()))
    }

    async fn load_current_session_task(
        &self,
        parent: &SubagentParentContext,
        task_id: &TaskId,
    ) -> ToolResult<TaskRecord> {
        let (session_id, _) = Self::require_parent_session(parent)?;
        let tasks = task_history::list_tasks(&self.store, Some(session_id.as_str()))
            .await
            .map_err(map_anyhow)?;
        let summary = tasks
            .into_iter()
            .find(|summary| &summary.task_id == task_id)
            .ok_or_else(|| {
                agent::tools::ToolError::invalid(format!("unknown task id: {}", task_id))
            })?;
        let loaded = task_history::load_task(&self.store, summary)
            .await
            .map_err(map_anyhow)?;
        Ok(task_record_from_loaded_task(loaded))
    }
}

#[async_trait]
impl TaskManager for SessionTaskManager {
    async fn create_task(
        &self,
        parent: SubagentParentContext,
        task: agent::types::AgentTaskSpec,
        status: TaskStatus,
    ) -> ToolResult<TaskRecord> {
        let (session_id, agent_session_id) = Self::require_parent_session(&parent)?;
        let initial_summary = Some(task.prompt.clone());
        self.append_task_event(
            &parent,
            SessionEventKind::TaskCreated {
                task: task.clone(),
                parent_agent_id: parent.parent_agent_id.clone(),
                status,
                summary: initial_summary.clone(),
            },
        )
        .await?;
        Ok(TaskRecord {
            summary: TaskSummaryRecord {
                task_id: task.task_id.clone(),
                session_id,
                agent_session_id,
                role: task.role.clone(),
                origin: task.origin,
                status,
                parent_agent_id: parent.parent_agent_id,
                child_agent_id: None,
                summary: initial_summary,
            },
            spec: task,
            claimed_files: Vec::new(),
            result: None,
            error: None,
        })
    }

    async fn get_task(
        &self,
        parent: SubagentParentContext,
        task_id: &TaskId,
    ) -> ToolResult<TaskRecord> {
        self.load_current_session_task(&parent, task_id).await
    }

    async fn list_tasks(
        &self,
        parent: SubagentParentContext,
        include_closed: bool,
    ) -> ToolResult<Vec<TaskSummaryRecord>> {
        let (session_id, _) = Self::require_parent_session(&parent)?;
        let tasks = task_history::list_tasks(&self.store, Some(session_id.as_str()))
            .await
            .map_err(map_anyhow)?;
        Ok(tasks
            .into_iter()
            .filter(|summary| include_closed || !summary.status.is_terminal())
            .map(task_summary_record_from_summary)
            .collect())
    }

    async fn update_task(
        &self,
        parent: SubagentParentContext,
        task_id: TaskId,
        status: Option<TaskStatus>,
        summary: Option<String>,
    ) -> ToolResult<TaskRecord> {
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(map_tool_error)?;
        if let Some(handle) = handles.iter().find(|handle| handle.task_id == task_id) {
            if status == Some(TaskStatus::Cancelled) {
                return self
                    .stop_task(
                        parent,
                        handle.task_id.clone(),
                        summary.or_else(|| Some("cancelled".to_string())),
                    )
                    .await;
            }
            if status.is_some() {
                return Err(agent::tools::ToolError::invalid(
                    "task_update cannot override the runtime-owned status of a live child task",
                ));
            }
        }

        self.append_task_event(
            &parent,
            SessionEventKind::TaskUpdated {
                task_id: task_id.clone(),
                status: status.unwrap_or(TaskStatus::Open),
                summary,
            },
        )
        .await?;
        self.load_current_session_task(&parent, &task_id).await
    }

    async fn stop_task(
        &self,
        parent: SubagentParentContext,
        task_id: TaskId,
        reason: Option<String>,
    ) -> ToolResult<TaskRecord> {
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(map_tool_error)?;
        if let Some(handle) = handles.iter().find(|handle| handle.task_id == task_id) {
            self.subagent_executor
                .cancel(parent.clone(), handle.agent_id.clone(), reason.clone())
                .await
                .map_err(map_tool_error)?;
            return self.load_current_session_task(&parent, &task_id).await;
        }

        self.append_task_event(
            &parent,
            SessionEventKind::TaskUpdated {
                task_id: task_id.clone(),
                status: TaskStatus::Cancelled,
                summary: reason,
            },
        )
        .await?;
        self.load_current_session_task(&parent, &task_id).await
    }
}

fn task_summary_record_from_summary(summary: crate::ui::PersistedTaskSummary) -> TaskSummaryRecord {
    TaskSummaryRecord {
        task_id: summary.task_id,
        session_id: SessionId::from(summary.session_ref),
        agent_session_id: AgentSessionId::from(summary.parent_agent_session_ref),
        role: summary.role,
        origin: summary.origin,
        status: summary.status,
        parent_agent_id: None,
        child_agent_id: None,
        summary: Some(summary.summary),
    }
}

fn task_record_from_loaded_task(loaded: LoadedTask) -> TaskRecord {
    let child_agent_id = loaded.result.as_ref().map(|result| result.agent_id.clone());
    let claimed_files = loaded
        .result
        .as_ref()
        .map(|result| result.claimed_files.clone())
        .unwrap_or_default();
    let summary_text =
        (!loaded.summary.summary.trim().is_empty()).then(|| loaded.summary.summary.clone());
    TaskRecord {
        summary: TaskSummaryRecord {
            task_id: loaded.summary.task_id.clone(),
            session_id: SessionId::from(loaded.summary.session_ref.clone()),
            agent_session_id: AgentSessionId::from(loaded.summary.parent_agent_session_ref.clone()),
            role: loaded.summary.role.clone(),
            origin: loaded.summary.origin,
            status: loaded.summary.status,
            parent_agent_id: None,
            child_agent_id,
            summary: summary_text,
        },
        spec: loaded.spec,
        claimed_files,
        result: loaded.result,
        error: loaded.error,
    }
}

fn map_tool_error(error: agent::tools::ToolError) -> agent::tools::ToolError {
    error
}

fn map_anyhow(error: anyhow::Error) -> agent::tools::ToolError {
    agent::tools::ToolError::invalid_state(error.to_string())
}
