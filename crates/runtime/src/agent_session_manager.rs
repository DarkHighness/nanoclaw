use crate::agent_mailbox::AgentMailbox;
use crate::{Result, RuntimeError};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use types::{
    AgentHandle, AgentId, AgentResultEnvelope, AgentStatus, AgentTaskSpec, AgentWaitMode,
    AgentWaitRequest, AgentWaitResponse,
};

struct AgentRecord {
    handle: AgentHandle,
    task: AgentTaskSpec,
    mailbox: AgentMailbox,
    result: Option<AgentResultEnvelope>,
    error: Option<String>,
    join_handle: Option<JoinHandle<()>>,
}

#[derive(Clone, Default)]
pub struct AgentSessionManager {
    records: Arc<Mutex<BTreeMap<AgentId, AgentRecord>>>,
    updates: Arc<Notify>,
}

impl AgentSessionManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, handle: AgentHandle, task: AgentTaskSpec, mailbox: AgentMailbox) {
        self.records.lock().unwrap().insert(
            handle.agent_id.clone(),
            AgentRecord {
                handle,
                task,
                mailbox,
                result: None,
                error: None,
                join_handle: None,
            },
        );
        self.updates.notify_waiters();
    }

    pub fn attach_join_handle(
        &self,
        agent_id: &AgentId,
        join_handle: JoinHandle<()>,
    ) -> Result<()> {
        let mut guard = self.records.lock().unwrap();
        let record = guard.get_mut(agent_id).ok_or_else(|| {
            RuntimeError::invalid_state(format!("unknown child agent: {agent_id}"))
        })?;
        record.join_handle = Some(join_handle);
        Ok(())
    }

    pub fn mailbox(&self, agent_id: &AgentId) -> Result<AgentMailbox> {
        let guard = self.records.lock().unwrap();
        guard
            .get(agent_id)
            .map(|record| record.mailbox.clone())
            .ok_or_else(|| RuntimeError::invalid_state(format!("unknown child agent: {agent_id}")))
    }

    pub fn handle(&self, agent_id: &AgentId) -> Result<AgentHandle> {
        let guard = self.records.lock().unwrap();
        guard
            .get(agent_id)
            .map(|record| record.handle.clone())
            .ok_or_else(|| RuntimeError::invalid_state(format!("unknown child agent: {agent_id}")))
    }

    pub fn task(&self, agent_id: &AgentId) -> Result<AgentTaskSpec> {
        let guard = self.records.lock().unwrap();
        guard
            .get(agent_id)
            .map(|record| record.task.clone())
            .ok_or_else(|| RuntimeError::invalid_state(format!("unknown child agent: {agent_id}")))
    }

    pub fn update_status(&self, agent_id: &AgentId, status: AgentStatus) -> Result<AgentHandle> {
        let mut guard = self.records.lock().unwrap();
        let record = guard.get_mut(agent_id).ok_or_else(|| {
            RuntimeError::invalid_state(format!("unknown child agent: {agent_id}"))
        })?;
        record.handle.status = status;
        let handle = record.handle.clone();
        self.updates.notify_waiters();
        Ok(handle)
    }

    pub fn finish(
        &self,
        agent_id: &AgentId,
        status: AgentStatus,
        result: Option<AgentResultEnvelope>,
        error: Option<String>,
    ) -> Result<AgentHandle> {
        if !status.is_terminal() {
            return Err(RuntimeError::invalid_state(format!(
                "finish requires terminal status, got {status}"
            )));
        }
        let mut guard = self.records.lock().unwrap();
        let record = guard.get_mut(agent_id).ok_or_else(|| {
            RuntimeError::invalid_state(format!("unknown child agent: {agent_id}"))
        })?;
        record.handle.status = status;
        record.result = result;
        record.error = error;
        let handle = record.handle.clone();
        self.updates.notify_waiters();
        Ok(handle)
    }

    pub fn list(&self) -> Vec<AgentHandle> {
        self.records
            .lock()
            .unwrap()
            .values()
            .map(|record| record.handle.clone())
            .collect()
    }

    pub async fn wait(&self, request: AgentWaitRequest) -> Result<AgentWaitResponse> {
        loop {
            // Register the waiter before snapshotting state so status transitions
            // that race with the snapshot cannot be lost between the read and
            // the subsequent wait.
            let notified = self.updates.notified();
            let response = self.snapshot_wait(&request)?;
            let done = match request.mode {
                AgentWaitMode::Any => !response.completed.is_empty(),
                AgentWaitMode::All => response.pending.is_empty(),
            };
            if done {
                return Ok(response);
            }
            notified.await;
        }
    }

    fn snapshot_wait(&self, request: &AgentWaitRequest) -> Result<AgentWaitResponse> {
        let guard = self.records.lock().unwrap();
        let mut completed = Vec::new();
        let mut pending = Vec::new();
        let mut results = Vec::new();
        for agent_id in &request.agent_ids {
            let record = guard.get(agent_id).ok_or_else(|| {
                RuntimeError::invalid_state(format!("unknown child agent: {agent_id}"))
            })?;
            if record.handle.status.is_terminal() {
                completed.push(record.handle.clone());
                if let Some(result) = &record.result {
                    results.push(result.clone());
                }
            } else {
                pending.push(record.handle.clone());
            }
        }
        if matches!(request.mode, AgentWaitMode::Any)
            && let Some(first_completed) = completed.first().cloned()
        {
            let completed_id = first_completed.agent_id.clone();
            let result = guard
                .get(&completed_id)
                .and_then(|record| record.result.clone());
            completed = vec![first_completed];
            pending = request
                .agent_ids
                .iter()
                .filter(|agent_id| **agent_id != completed_id)
                .filter_map(|agent_id| guard.get(agent_id).map(|record| record.handle.clone()))
                .collect();
            results = result.into_iter().collect();
        }
        Ok(AgentWaitResponse {
            completed,
            pending,
            results,
        })
    }

    pub fn cancel(
        &self,
        agent_id: &AgentId,
        reason: Option<String>,
        claimed_files: Vec<String>,
    ) -> Result<(AgentHandle, AgentResultEnvelope)> {
        let mut guard = self.records.lock().unwrap();
        let record = guard.get_mut(agent_id).ok_or_else(|| {
            RuntimeError::invalid_state(format!("unknown child agent: {agent_id}"))
        })?;
        if record.handle.status.is_terminal() {
            let result = record
                .result
                .clone()
                .unwrap_or_else(|| AgentResultEnvelope {
                    agent_id: record.handle.agent_id.clone(),
                    task_id: record.task.task_id.clone(),
                    status: record.handle.status.clone(),
                    summary: reason
                        .clone()
                        .unwrap_or_else(|| "agent already terminal".to_string()),
                    text: String::new(),
                    artifacts: Vec::new(),
                    claimed_files,
                    structured_payload: None,
                });
            return Ok((record.handle.clone(), result));
        }
        if let Some(join_handle) = record.join_handle.take() {
            join_handle.abort();
        }
        let result = AgentResultEnvelope {
            agent_id: record.handle.agent_id.clone(),
            task_id: record.task.task_id.clone(),
            status: AgentStatus::Cancelled,
            summary: reason
                .clone()
                .unwrap_or_else(|| "child agent cancelled".to_string()),
            text: String::new(),
            artifacts: Vec::new(),
            claimed_files,
            structured_payload: None,
        };
        record.handle.status = AgentStatus::Cancelled;
        record.result = Some(result.clone());
        record.error = reason;
        let handle = record.handle.clone();
        self.updates.notify_waiters();
        Ok((handle, result))
    }
}

#[cfg(test)]
mod tests {
    use super::AgentSessionManager;
    use crate::agent_mailbox::agent_mailbox_channel;
    use tokio::time::{Duration, timeout};
    use types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentStatus, AgentTaskSpec, AgentWaitMode,
        AgentWaitRequest,
    };

    #[tokio::test]
    async fn wait_any_returns_after_first_terminal_child() {
        let manager = AgentSessionManager::new();
        let (mailbox, _) = agent_mailbox_channel();
        manager.insert(
            AgentHandle {
                agent_id: "agent_1".into(),
                parent_agent_id: None,
                run_id: "run_1".into(),
                session_id: "session_1".into(),
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                status: AgentStatus::Running,
            },
            AgentTaskSpec {
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                prompt: "inspect".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            },
            mailbox,
        );

        let waiter = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .wait(AgentWaitRequest {
                        agent_ids: vec![AgentId::from("agent_1")],
                        mode: AgentWaitMode::Any,
                    })
                    .await
                    .unwrap()
            })
        };

        manager
            .finish(
                &AgentId::from("agent_1"),
                AgentStatus::Completed,
                Some(AgentResultEnvelope {
                    agent_id: "agent_1".into(),
                    task_id: "task_1".to_string(),
                    status: AgentStatus::Completed,
                    summary: "done".to_string(),
                    text: "ok".to_string(),
                    artifacts: Vec::new(),
                    claimed_files: Vec::new(),
                    structured_payload: None,
                }),
                None,
            )
            .unwrap();

        let response = timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.completed.len(), 1);
        assert_eq!(response.completed[0].agent_id, AgentId::from("agent_1"));
    }

    #[test]
    fn cancel_marks_child_as_cancelled() {
        let manager = AgentSessionManager::new();
        let (mailbox, _) = agent_mailbox_channel();
        manager.insert(
            AgentHandle {
                agent_id: "agent_1".into(),
                parent_agent_id: None,
                run_id: "run_1".into(),
                session_id: "session_1".into(),
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                status: AgentStatus::Running,
            },
            AgentTaskSpec {
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                prompt: "inspect".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            },
            mailbox,
        );

        let (handle, result) = manager
            .cancel(
                &AgentId::from("agent_1"),
                Some("stop".to_string()),
                vec!["src/lib.rs".to_string()],
            )
            .unwrap();
        assert_eq!(handle.status, AgentStatus::Cancelled);
        assert_eq!(result.status, AgentStatus::Cancelled);
        assert_eq!(result.claimed_files, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn finish_rejects_non_terminal_status() {
        let manager = AgentSessionManager::new();
        let (mailbox, _) = agent_mailbox_channel();
        manager.insert(
            AgentHandle {
                agent_id: "agent_1".into(),
                parent_agent_id: None,
                run_id: "run_1".into(),
                session_id: "session_1".into(),
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                status: AgentStatus::Running,
            },
            AgentTaskSpec {
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                prompt: "inspect".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            },
            mailbox,
        );

        let error = manager
            .finish(&AgentId::from("agent_1"), AgentStatus::Running, None, None)
            .unwrap_err();
        assert!(error.to_string().contains("terminal status"));
    }

    #[test]
    fn wait_any_keeps_result_aligned_with_completed_agent() {
        let manager = AgentSessionManager::new();
        for agent_id in ["agent_1", "agent_2"] {
            let (mailbox, _) = agent_mailbox_channel();
            manager.insert(
                AgentHandle {
                    agent_id: agent_id.into(),
                    parent_agent_id: None,
                    run_id: "run_1".into(),
                    session_id: "session_1".into(),
                    task_id: format!("task_{agent_id}"),
                    role: "explorer".to_string(),
                    status: AgentStatus::Running,
                },
                AgentTaskSpec {
                    task_id: format!("task_{agent_id}"),
                    role: "explorer".to_string(),
                    prompt: "inspect".to_string(),
                    steer: None,
                    allowed_tools: Vec::new(),
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                },
                mailbox,
            );
        }
        manager
            .finish(
                &AgentId::from("agent_1"),
                AgentStatus::Completed,
                Some(AgentResultEnvelope {
                    agent_id: "agent_1".into(),
                    task_id: "task_agent_1".to_string(),
                    status: AgentStatus::Completed,
                    summary: "done".to_string(),
                    text: "ok".to_string(),
                    artifacts: Vec::new(),
                    claimed_files: Vec::new(),
                    structured_payload: None,
                }),
                None,
            )
            .unwrap();
        manager
            .finish(
                &AgentId::from("agent_2"),
                AgentStatus::Completed,
                Some(AgentResultEnvelope {
                    agent_id: "agent_2".into(),
                    task_id: "task_agent_2".to_string(),
                    status: AgentStatus::Completed,
                    summary: "done".to_string(),
                    text: "ok".to_string(),
                    artifacts: Vec::new(),
                    claimed_files: Vec::new(),
                    structured_payload: None,
                }),
                None,
            )
            .unwrap();

        let response = manager
            .snapshot_wait(&AgentWaitRequest {
                agent_ids: vec![AgentId::from("agent_1"), AgentId::from("agent_2")],
                mode: AgentWaitMode::Any,
            })
            .unwrap();

        assert_eq!(response.completed.len(), 1);
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.completed[0].agent_id, response.results[0].agent_id);
    }
}
