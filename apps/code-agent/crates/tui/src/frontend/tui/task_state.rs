use super::state::TrackedTaskSummary;
use agent::types::{
    AgentId, AgentResultEnvelope, AgentTaskSpec, SessionEventEnvelope, SessionEventKind, TaskId,
    TaskStatus,
};
use std::collections::BTreeMap;

pub(crate) fn restore_tracked_tasks(events: &[SessionEventEnvelope]) -> Vec<TrackedTaskSummary> {
    let mut tasks = BTreeMap::<TaskId, TrackedTaskSummary>::new();
    for event in events {
        match &event.event {
            SessionEventKind::TaskCreated {
                task,
                parent_agent_id,
                status,
                summary,
            } => apply_task_created_map(
                &mut tasks,
                task,
                parent_agent_id.as_ref(),
                *status,
                summary.clone(),
            ),
            SessionEventKind::TaskUpdated {
                task_id,
                status,
                summary,
            } => apply_task_updated_map(&mut tasks, task_id, *status, summary.clone()),
            SessionEventKind::TaskCompleted {
                task_id,
                agent_id,
                status,
            } => apply_task_completed_map(&mut tasks, task_id, agent_id, *status),
            SessionEventKind::SubagentStart { handle, task } => {
                let entry =
                    tasks
                        .entry(task.task_id.clone())
                        .or_insert_with(|| TrackedTaskSummary {
                            task_id: task.task_id.clone(),
                            role: task.role.clone(),
                            origin: task.origin,
                            status: TaskStatus::from(&handle.status),
                            summary: Some(task.prompt.clone()),
                            parent_agent_id: None,
                            child_agent_id: Some(handle.agent_id.to_string()),
                        });
                entry.role = task.role.clone();
                entry.origin = task.origin;
                entry.status = TaskStatus::from(&handle.status);
                entry.child_agent_id = Some(handle.agent_id.to_string());
                if entry.summary.is_none() {
                    entry.summary = Some(task.prompt.clone());
                }
            }
            SessionEventKind::SubagentStop {
                handle,
                result,
                error,
            } => {
                let status = result
                    .as_ref()
                    .map(|result| TaskStatus::from(&result.status))
                    .unwrap_or_else(|| {
                        if error.is_some() {
                            TaskStatus::Failed
                        } else {
                            TaskStatus::Cancelled
                        }
                    });
                let entry =
                    tasks
                        .entry(handle.task_id.clone())
                        .or_insert_with(|| TrackedTaskSummary {
                            task_id: handle.task_id.clone(),
                            role: "task".to_string(),
                            origin: agent::types::TaskOrigin::ChildAgentBacked,
                            status,
                            summary: None,
                            parent_agent_id: None,
                            child_agent_id: Some(handle.agent_id.to_string()),
                        });
                entry.status = status;
                entry.child_agent_id = Some(handle.agent_id.to_string());
                if let Some(result) = result.as_ref() {
                    entry.summary = Some(result.summary.clone());
                } else if let Some(error) = error.as_deref() {
                    entry.summary = Some(error.to_string());
                }
            }
            _ => {}
        }
    }
    sort_task_values(tasks.into_values().collect())
}

pub(crate) fn apply_task_created(
    tasks: &mut Vec<TrackedTaskSummary>,
    task: &AgentTaskSpec,
    parent_agent_id: Option<&AgentId>,
    status: TaskStatus,
    summary: Option<String>,
) {
    let mut by_id = index_tasks(tasks);
    apply_task_created_map(&mut by_id, task, parent_agent_id, status, summary);
    *tasks = sort_task_values(by_id.into_values().collect());
}

pub(crate) fn apply_task_updated(
    tasks: &mut Vec<TrackedTaskSummary>,
    task_id: &TaskId,
    status: TaskStatus,
    summary: Option<String>,
) {
    let mut by_id = index_tasks(tasks);
    apply_task_updated_map(&mut by_id, task_id, status, summary);
    *tasks = sort_task_values(by_id.into_values().collect());
}

pub(crate) fn apply_task_completed(
    tasks: &mut Vec<TrackedTaskSummary>,
    task_id: &TaskId,
    agent_id: &AgentId,
    status: TaskStatus,
) {
    let mut by_id = index_tasks(tasks);
    apply_task_completed_map(&mut by_id, task_id, agent_id, status);
    *tasks = sort_task_values(by_id.into_values().collect());
}

pub(crate) fn apply_subagent_started(
    tasks: &mut Vec<TrackedTaskSummary>,
    agent_id: &str,
    task: &AgentTaskSpec,
    status: &agent::types::AgentStatus,
) {
    let mut by_id = index_tasks(tasks);
    let entry = by_id
        .entry(task.task_id.clone())
        .or_insert_with(|| TrackedTaskSummary {
            task_id: task.task_id.clone(),
            role: task.role.clone(),
            origin: task.origin,
            status: TaskStatus::from(status),
            summary: Some(task.prompt.clone()),
            parent_agent_id: None,
            child_agent_id: Some(agent_id.to_string()),
        });
    entry.role = task.role.clone();
    entry.origin = task.origin;
    entry.status = TaskStatus::from(status);
    entry.child_agent_id = Some(agent_id.to_string());
    if entry.summary.is_none() {
        entry.summary = Some(task.prompt.clone());
    }
    *tasks = sort_task_values(by_id.into_values().collect());
}

pub(crate) fn apply_subagent_stopped(
    tasks: &mut Vec<TrackedTaskSummary>,
    task_id: &TaskId,
    agent_id: &str,
    result: Option<&AgentResultEnvelope>,
    error: Option<&str>,
) {
    let mut by_id = index_tasks(tasks);
    let status = result
        .map(|result| TaskStatus::from(&result.status))
        .unwrap_or_else(|| {
            if error.is_some() {
                TaskStatus::Failed
            } else {
                TaskStatus::Cancelled
            }
        });
    let entry = by_id
        .entry(task_id.clone())
        .or_insert_with(|| TrackedTaskSummary {
            task_id: task_id.clone(),
            role: "task".to_string(),
            origin: agent::types::TaskOrigin::ChildAgentBacked,
            status,
            summary: None,
            parent_agent_id: None,
            child_agent_id: Some(agent_id.to_string()),
        });
    entry.status = status;
    entry.child_agent_id = Some(agent_id.to_string());
    if let Some(result) = result {
        entry.summary = Some(result.summary.clone());
    } else if let Some(error) = error {
        entry.summary = Some(error.to_string());
    }
    *tasks = sort_task_values(by_id.into_values().collect());
}

fn index_tasks(tasks: &[TrackedTaskSummary]) -> BTreeMap<TaskId, TrackedTaskSummary> {
    tasks
        .iter()
        .cloned()
        .map(|task| (task.task_id.clone(), task))
        .collect()
}

fn apply_task_created_map(
    tasks: &mut BTreeMap<TaskId, TrackedTaskSummary>,
    task: &AgentTaskSpec,
    parent_agent_id: Option<&AgentId>,
    status: TaskStatus,
    summary: Option<String>,
) {
    tasks.insert(
        task.task_id.clone(),
        TrackedTaskSummary {
            task_id: task.task_id.clone(),
            role: task.role.clone(),
            origin: task.origin,
            status,
            summary,
            parent_agent_id: parent_agent_id.map(ToString::to_string),
            child_agent_id: None,
        },
    );
}

fn apply_task_updated_map(
    tasks: &mut BTreeMap<TaskId, TrackedTaskSummary>,
    task_id: &TaskId,
    status: TaskStatus,
    summary: Option<String>,
) {
    if let Some(entry) = tasks.get_mut(task_id) {
        entry.status = status;
        if summary.is_some() {
            entry.summary = summary;
        }
        return;
    }
    tasks.insert(
        task_id.clone(),
        TrackedTaskSummary {
            task_id: task_id.clone(),
            role: "task".to_string(),
            origin: agent::types::TaskOrigin::AgentCreated,
            status,
            summary,
            parent_agent_id: None,
            child_agent_id: None,
        },
    );
}

fn apply_task_completed_map(
    tasks: &mut BTreeMap<TaskId, TrackedTaskSummary>,
    task_id: &TaskId,
    agent_id: &AgentId,
    status: TaskStatus,
) {
    let entry = tasks
        .entry(task_id.clone())
        .or_insert_with(|| TrackedTaskSummary {
            task_id: task_id.clone(),
            role: "task".to_string(),
            origin: agent::types::TaskOrigin::ChildAgentBacked,
            status,
            summary: None,
            parent_agent_id: None,
            child_agent_id: Some(agent_id.to_string()),
        });
    entry.status = status;
    entry.child_agent_id = Some(agent_id.to_string());
}

fn sort_task_values(mut tasks: Vec<TrackedTaskSummary>) -> Vec<TrackedTaskSummary> {
    tasks.sort_by(|left, right| {
        left.status
            .is_terminal()
            .cmp(&right.status.is_terminal())
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    tasks
}
