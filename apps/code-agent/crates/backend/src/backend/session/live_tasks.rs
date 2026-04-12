use super::*;

impl CodeAgentSession {
    pub async fn list_live_tasks(&self) -> Result<Vec<LiveTaskSummary>> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(live_task_summaries(&handles))
    }

    pub async fn spawn_live_task(&self, role: &str, prompt: &str) -> Result<LiveTaskSpawnOutcome> {
        let role = role.trim();
        if role.is_empty() {
            return Err(anyhow::anyhow!("live task role cannot be empty"));
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(anyhow::anyhow!("live task prompt cannot be empty"));
        }

        let parent = self.live_task_parent_context();
        let task = AgentTaskSpec {
            task_id: new_live_task_id(),
            role: role.to_string(),
            prompt: prompt.to_string(),
            origin: agent::types::TaskOrigin::UserCreated,
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let mut handles = self
            .subagent_executor
            .spawn(parent, vec![SubagentLaunchSpec::from_task(task)])
            .await
            .map_err(anyhow::Error::from)?;
        let handle = handles
            .pop()
            .ok_or_else(|| anyhow::anyhow!("live task spawn returned no child handle"))?;
        Ok(LiveTaskSpawnOutcome {
            task: live_task_summary(&handle),
            prompt: prompt.to_string(),
        })
    }

    pub async fn send_live_task(
        &self,
        task_or_agent_ref: &str,
        message: &str,
    ) -> Result<LiveTaskMessageOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(anyhow::Error::from)?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let updated = self
            .subagent_executor
            .send(
                parent,
                handle.agent_id.clone(),
                Message::user(message),
                SubagentInputDelivery::Queue,
            )
            .await
            .map_err(anyhow::Error::from)?;
        Ok(LiveTaskMessageOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: updated.agent_id.to_string(),
            task_id: updated.task_id,
            status: updated.status.clone().into(),
            action: if handle.status.is_terminal() {
                LiveTaskMessageAction::AlreadyTerminal
            } else {
                LiveTaskMessageAction::Sent
            },
            message: message.to_string(),
        })
    }

    pub async fn wait_live_task(&self, task_or_agent_ref: &str) -> Result<LiveTaskWaitOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(anyhow::Error::from)?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let response = self
            .subagent_executor
            .wait(
                parent.clone(),
                AgentWaitRequest {
                    agent_ids: vec![handle.agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .map_err(anyhow::Error::from)?;
        let completed = response
            .completed
            .into_iter()
            .find(|candidate| candidate.agent_id == handle.agent_id)
            .unwrap_or(handle);
        let result = response
            .results
            .into_iter()
            .find(|candidate| candidate.agent_id.as_str() == completed.agent_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing live task result for {}", completed.task_id))?;
        let refreshed_handles = self
            .subagent_executor
            .list(parent)
            .await
            .map_err(anyhow::Error::from)?;
        let remaining_live_tasks = live_task_summaries(&refreshed_handles)
            .into_iter()
            .filter(|task| task.agent_id != completed.agent_id.as_str())
            .filter(|task| !task.status.is_terminal())
            .collect();
        Ok(LiveTaskWaitOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: completed.agent_id.to_string(),
            task_id: completed.task_id,
            status: completed.status.into(),
            summary: result.summary,
            claimed_files: result.claimed_files,
            remaining_live_tasks,
        })
    }

    pub fn schedule_live_task_attention(
        &self,
        outcome: &LiveTaskWaitOutcome,
        turn_running: bool,
    ) -> Result<LiveTaskAttentionOutcome> {
        let preview = render_live_task_attention_message(outcome);
        if turn_running {
            let control_id = self.schedule_runtime_steer(
                preview.clone(),
                Some(format!("live_task_wait_complete:{}", outcome.task_id)),
            )?;
            return Ok(LiveTaskAttentionOutcome {
                action: LiveTaskAttentionAction::ScheduledSteer,
                control_id,
                preview,
            });
        }

        let queued = self
            .control_plane
            .push_prompt(Message::user(preview.clone()));
        Ok(LiveTaskAttentionOutcome {
            action: LiveTaskAttentionAction::QueuedPrompt,
            control_id: queued.id.to_string(),
            preview,
        })
    }

    pub async fn cancel_live_task(
        &self,
        task_or_agent_ref: &str,
        reason: Option<String>,
    ) -> Result<LiveTaskControlOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(anyhow::Error::from)?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let updated = self
            .subagent_executor
            .cancel(parent, handle.agent_id.clone(), reason)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(LiveTaskControlOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: updated.agent_id.to_string(),
            task_id: updated.task_id,
            status: updated.status.clone().into(),
            action: if handle.status.is_terminal() {
                LiveTaskControlAction::AlreadyTerminal
            } else {
                LiveTaskControlAction::Cancelled
            },
        })
    }

    // Host-initiated live task operations should still append their lifecycle
    // into the active top-level session, otherwise operator-side spawn/send/
    // cancel actions disappear from durable task history.
    fn live_task_parent_context(&self) -> SubagentParentContext {
        let startup = self.startup_snapshot();
        SubagentParentContext {
            session_id: Some(SessionId::from(startup.active_session_ref)),
            agent_session_id: Some(AgentSessionId::from(startup.root_agent_session_id)),
            turn_id: None,
            parent_agent_id: None,
            active_worktree_id: None,
            worktree_root: None,
        }
    }
}

fn new_live_task_id() -> agent::types::TaskId {
    format!("task_{}", new_opaque_id()).into()
}

fn live_task_summary(handle: &agent::types::AgentHandle) -> LiveTaskSummary {
    LiveTaskSummary {
        agent_id: handle.agent_id.to_string(),
        task_id: handle.task_id.clone(),
        role: handle.role.clone(),
        origin: agent::types::TaskOrigin::ChildAgentBacked,
        status: handle.status.clone().into(),
        session_ref: handle.session_id.to_string(),
        agent_session_ref: handle.agent_session_id.to_string(),
        worktree_id: handle.worktree_id.clone(),
        worktree_root: handle.worktree_root.clone(),
    }
}

fn live_task_summaries(handles: &[agent::types::AgentHandle]) -> Vec<LiveTaskSummary> {
    let mut summaries = handles.iter().map(live_task_summary).collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.task_id
            .cmp(&right.task_id)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    summaries
}

fn render_live_task_attention_message(outcome: &LiveTaskWaitOutcome) -> String {
    let mut lines = vec![format!(
        "Background task {} finished with status {}.",
        outcome.task_id, outcome.status
    )];
    if !outcome.summary.trim().is_empty() {
        lines.push(format!("Task summary: {}", outcome.summary.trim()));
    }
    if !outcome.claimed_files.is_empty() {
        lines.push(format!(
            "Claimed files: {}.",
            outcome.claimed_files.join(", ")
        ));
    }
    if !outcome.remaining_live_tasks.is_empty() {
        lines.push(format!(
            "Still running background tasks: {}.",
            outcome
                .remaining_live_tasks
                .iter()
                .map(render_live_task_attention_task)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.push(live_task_attention_instruction(outcome.status.clone()).to_string());
    lines.join("\n")
}

fn render_live_task_attention_task(task: &LiveTaskSummary) -> String {
    if let Some(worktree_id) = task.worktree_id.as_ref() {
        format!(
            "{} ({}, {}, worktree {})",
            task.task_id, task.role, task.status, worktree_id
        )
    } else {
        format!("{} ({}, {})", task.task_id, task.role, task.status)
    }
}

fn live_task_attention_instruction(status: agent::types::TaskStatus) -> &'static str {
    match status {
        agent::types::TaskStatus::Completed => {
            "Review the completed background task and integrate any useful findings."
        }
        agent::types::TaskStatus::Failed => {
            "Inspect the failed background task and decide whether to retry it."
        }
        agent::types::TaskStatus::Cancelled => {
            "Inspect the cancelled background task and decide whether it should be restarted."
        }
        agent::types::TaskStatus::Open
        | agent::types::TaskStatus::Queued
        | agent::types::TaskStatus::Running
        | agent::types::TaskStatus::WaitingApproval
        | agent::types::TaskStatus::WaitingMessage => {
            "Inspect the background task state before deciding on the next step."
        }
    }
}

fn resolve_live_task_reference<'a>(
    handles: &'a [agent::types::AgentHandle],
    task_or_agent_ref: &str,
) -> Result<&'a agent::types::AgentHandle> {
    if let Some(handle) = handles
        .iter()
        .find(|handle| handle.task_id.as_str() == task_or_agent_ref)
    {
        return Ok(handle);
    }
    if let Some(handle) = handles
        .iter()
        .find(|handle| handle.agent_id.as_str() == task_or_agent_ref)
    {
        return Ok(handle);
    }

    let task_matches = handles
        .iter()
        .filter(|handle| handle.task_id.as_str().starts_with(task_or_agent_ref))
        .collect::<Vec<_>>();
    match task_matches.as_slice() {
        [handle] => return Ok(handle),
        [] => {}
        _ => {
            return Err(anyhow::anyhow!(
                "ambiguous live task prefix {task_or_agent_ref}: {}",
                task_matches
                    .iter()
                    .take(6)
                    .map(|handle| handle.task_id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let agent_matches = handles
        .iter()
        .filter(|handle| handle.agent_id.as_str().starts_with(task_or_agent_ref))
        .collect::<Vec<_>>();
    match agent_matches.as_slice() {
        [] => Err(anyhow::anyhow!(
            "unknown live task or agent id: {task_or_agent_ref}"
        )),
        [handle] => Ok(handle),
        _ => Err(anyhow::anyhow!(
            "ambiguous live agent prefix {task_or_agent_ref}: {}",
            agent_matches
                .iter()
                .take(6)
                .map(|handle| preview_id(handle.agent_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}
