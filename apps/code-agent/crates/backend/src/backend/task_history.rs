use super::session_history::{list_sessions, resolve_session_reference};
use crate::ui::{LoadedTask, LoadedTaskMessage, PersistedTaskSummary};
use agent::types::{
    AgentEnvelope, AgentEnvelopeKind, AgentStatus, AgentTaskSpec, Message, SessionEventEnvelope,
    SessionEventKind, SessionId,
};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::sync::Arc;
use store::{SessionStore, TokenUsageRecord};

pub async fn list_tasks(
    store: &Arc<dyn SessionStore>,
    session_ref: Option<&str>,
) -> Result<Vec<PersistedTaskSummary>> {
    let sessions = list_sessions(store).await?;
    let filtered_session_id = session_ref
        .map(|session_ref| resolve_session_reference(&sessions, session_ref))
        .transpose()?;
    let mut tasks = Vec::new();
    for session in sessions.into_iter().filter(|summary| {
        filtered_session_id
            .as_ref()
            .is_none_or(|session_id| summary.session_id == *session_id)
    }) {
        let events = store.events(&session.session_id).await?;
        tasks.extend(persisted_task_summaries(
            session.session_id.as_str(),
            &events,
        ));
    }
    tasks.sort_by(|left, right| {
        right
            .last_timestamp_ms
            .cmp(&left.last_timestamp_ms)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    Ok(tasks)
}

pub async fn load_task(
    store: &Arc<dyn SessionStore>,
    summary: PersistedTaskSummary,
) -> Result<LoadedTask> {
    let session_id = SessionId::from(summary.session_ref.clone());
    let events = store.events(&session_id).await?;
    let token_usage = store.token_usage(&session_id).await?;
    let child_transcript = if let Some(child_session_ref) = &summary.child_session_ref {
        store
            .replay_transcript(&SessionId::from(child_session_ref.clone()))
            .await?
    } else {
        Vec::new()
    };
    project_loaded_task(summary, &events, child_transcript, token_usage.tasks)
}

pub fn resolve_task_reference<'a>(
    tasks: &'a [PersistedTaskSummary],
    task_ref: &str,
) -> Result<&'a PersistedTaskSummary> {
    if let Some(task) = tasks.iter().find(|summary| summary.task_id == task_ref) {
        return Ok(task);
    }

    let matches = tasks
        .iter()
        .filter(|summary| summary.task_id.starts_with(task_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow!("unknown task id or prefix: {task_ref}")),
        [task] => Ok(task),
        _ => Err(anyhow!(
            "ambiguous task prefix {task_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|task| task.task_id.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn persisted_task_summaries(
    session_ref: &str,
    events: &[SessionEventEnvelope],
) -> Vec<PersistedTaskSummary> {
    #[derive(Clone, Debug)]
    struct TaskAccumulator {
        spec: AgentTaskSpec,
        parent_agent_session_ref: String,
        child_session_ref: Option<String>,
        child_agent_session_ref: Option<String>,
        status: AgentStatus,
        first_timestamp_ms: u128,
        last_timestamp_ms: u128,
        summary: Option<String>,
    }

    let mut by_task = BTreeMap::<String, TaskAccumulator>::new();
    for event in events {
        match &event.event {
            SessionEventKind::TaskCreated { task, .. } => {
                by_task
                    .entry(task.task_id.clone())
                    .or_insert_with(|| TaskAccumulator {
                        spec: task.clone(),
                        parent_agent_session_ref: event.agent_session_id.to_string(),
                        child_session_ref: None,
                        child_agent_session_ref: None,
                        status: AgentStatus::Queued,
                        first_timestamp_ms: event.timestamp_ms,
                        last_timestamp_ms: event.timestamp_ms,
                        summary: Some(preview_text(&task.prompt, 64)),
                    });
            }
            SessionEventKind::SubagentStart { handle, task } => {
                let entry =
                    by_task
                        .entry(task.task_id.clone())
                        .or_insert_with(|| TaskAccumulator {
                            spec: task.clone(),
                            parent_agent_session_ref: event.agent_session_id.to_string(),
                            child_session_ref: None,
                            child_agent_session_ref: None,
                            status: handle.status.clone(),
                            first_timestamp_ms: event.timestamp_ms,
                            last_timestamp_ms: event.timestamp_ms,
                            summary: Some(preview_text(&task.prompt, 64)),
                        });
                entry.spec = task.clone();
                entry.parent_agent_session_ref = event.agent_session_id.to_string();
                entry.child_session_ref = Some(handle.session_id.to_string());
                entry.child_agent_session_ref = Some(handle.agent_session_id.to_string());
                entry.status = handle.status.clone();
                entry.first_timestamp_ms = entry.first_timestamp_ms.min(event.timestamp_ms);
                entry.last_timestamp_ms = entry.last_timestamp_ms.max(event.timestamp_ms);
            }
            SessionEventKind::TaskCompleted {
                task_id, status, ..
            } => {
                if let Some(entry) = by_task.get_mut(task_id) {
                    entry.status = status.clone();
                    entry.first_timestamp_ms = entry.first_timestamp_ms.min(event.timestamp_ms);
                    entry.last_timestamp_ms = entry.last_timestamp_ms.max(event.timestamp_ms);
                }
            }
            SessionEventKind::SubagentStop {
                handle,
                result,
                error,
            } => {
                if let Some(entry) = by_task.get_mut(handle.task_id.as_str()) {
                    entry.child_session_ref = Some(handle.session_id.to_string());
                    entry.child_agent_session_ref = Some(handle.agent_session_id.to_string());
                    entry.status = result
                        .as_ref()
                        .map(|result| result.status.clone())
                        .unwrap_or_else(|| handle.status.clone());
                    entry.summary = result
                        .as_ref()
                        .map(|result| result.summary.clone())
                        .or_else(|| error.clone());
                    entry.first_timestamp_ms = entry.first_timestamp_ms.min(event.timestamp_ms);
                    entry.last_timestamp_ms = entry.last_timestamp_ms.max(event.timestamp_ms);
                }
            }
            _ => {}
        }
    }

    by_task
        .into_iter()
        .map(|(task_id, entry)| PersistedTaskSummary {
            task_id,
            session_ref: session_ref.to_string(),
            parent_agent_session_ref: entry.parent_agent_session_ref,
            child_session_ref: entry.child_session_ref,
            child_agent_session_ref: entry.child_agent_session_ref,
            role: entry.spec.role,
            status: entry.status,
            first_timestamp_ms: entry.first_timestamp_ms,
            last_timestamp_ms: entry.last_timestamp_ms,
            summary: entry.summary.unwrap_or_else(|| "task".to_string()),
        })
        .collect()
}

fn project_loaded_task(
    summary: PersistedTaskSummary,
    events: &[SessionEventEnvelope],
    child_transcript: Vec<Message>,
    task_token_usage: Vec<TokenUsageRecord>,
) -> Result<LoadedTask> {
    let mut spec = None;
    let mut result = None;
    let mut error = None;
    let mut artifacts = Vec::new();
    let mut messages = Vec::new();

    for event in events {
        match &event.event {
            SessionEventKind::TaskCreated { task, .. } if task.task_id == summary.task_id => {
                spec = Some(task.clone());
            }
            SessionEventKind::SubagentStart { task, .. } if task.task_id == summary.task_id => {
                spec = Some(task.clone());
            }
            SessionEventKind::SubagentStop {
                handle,
                result: stop_result,
                error: stop_error,
            } if handle.task_id == summary.task_id => {
                if let Some(stop_result) = stop_result {
                    result = Some(stop_result.clone());
                }
                if let Some(stop_error) = stop_error {
                    error = Some(stop_error.clone());
                }
            }
            SessionEventKind::AgentEnvelope { envelope }
                if envelope_belongs_to_task(&summary, envelope) =>
            {
                match &envelope.kind {
                    AgentEnvelopeKind::Input { message, .. } => {
                        messages.push(LoadedTaskMessage {
                            message: message.clone(),
                        });
                    }
                    AgentEnvelopeKind::Artifact { artifact } => {
                        artifacts.push(artifact.clone());
                    }
                    AgentEnvelopeKind::Result {
                        result: envelope_result,
                    } => {
                        result = Some(envelope_result.clone());
                    }
                    AgentEnvelopeKind::Failed {
                        error: envelope_error,
                    } => {
                        error = Some(envelope_error.clone());
                    }
                    AgentEnvelopeKind::Cancelled { reason } => {
                        error = Some(
                            reason
                                .clone()
                                .unwrap_or_else(|| "child task cancelled".to_string()),
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let spec =
        spec.ok_or_else(|| anyhow!("task missing creation metadata: {}", summary.task_id))?;
    let token_usage = task_token_usage
        .into_iter()
        .find(|record| record.task_id.as_deref() == Some(summary.task_id.as_str()));

    Ok(LoadedTask {
        summary,
        spec,
        child_transcript,
        result,
        error,
        artifacts,
        messages,
        token_usage,
    })
}

fn envelope_belongs_to_task(summary: &PersistedTaskSummary, envelope: &AgentEnvelope) -> bool {
    if summary
        .child_agent_session_ref
        .as_deref()
        .is_some_and(|agent_session_ref| envelope.agent_session_id.as_str() == agent_session_ref)
    {
        return true;
    }
    summary
        .child_session_ref
        .as_deref()
        .is_some_and(|session_ref| envelope.session_id.as_str() == session_ref)
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "<empty>".to_string();
    }
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        format!(
            "{}...",
            collapsed
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{PersistedTaskSummary, persisted_task_summaries, project_loaded_task};
    use agent::types::{
        AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
        AgentSessionId, AgentStatus, AgentTaskSpec, Message, SessionEventEnvelope,
        SessionEventKind, SessionId, TokenLedgerSnapshot,
    };
    use store::{TokenUsageRecord, TokenUsageScope};

    #[test]
    fn persisted_task_summaries_capture_child_runtime_refs() {
        let session_id = SessionId::from("session-root");
        let parent_agent_session_id = AgentSessionId::from("agent-root");
        let child_session_id = SessionId::from("session-child");
        let child_agent_session_id = AgentSessionId::from("agent-child");
        let task = AgentTaskSpec {
            task_id: "review-task".to_string(),
            role: "reviewer".to_string(),
            prompt: "inspect the patch".to_string(),
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let handle = AgentHandle {
            agent_id: AgentId::from("agent-reviewer"),
            parent_agent_id: None,
            session_id: child_session_id.clone(),
            agent_session_id: child_agent_session_id.clone(),
            task_id: task.task_id.clone(),
            role: task.role.clone(),
            status: AgentStatus::Completed,
        };
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                parent_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TaskCreated {
                    task: task.clone(),
                    parent_agent_id: None,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                parent_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::SubagentStart {
                    handle: handle.clone(),
                    task: task.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                parent_agent_session_id,
                None,
                None,
                SessionEventKind::SubagentStop {
                    handle,
                    result: Some(AgentResultEnvelope {
                        agent_id: AgentId::from("agent-reviewer"),
                        task_id: task.task_id.clone(),
                        status: AgentStatus::Completed,
                        summary: "looks good".to_string(),
                        text: "looks good".to_string(),
                        artifacts: Vec::new(),
                        claimed_files: Vec::new(),
                        structured_payload: None,
                    }),
                    error: None,
                },
            ),
        ];

        let summaries = persisted_task_summaries(session_id.as_str(), &events);

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].task_id, "review-task");
        assert_eq!(
            summaries[0].child_session_ref.as_deref(),
            Some(child_session_id.as_str())
        );
        assert_eq!(
            summaries[0].child_agent_session_ref.as_deref(),
            Some(child_agent_session_id.as_str())
        );
        assert_eq!(summaries[0].status, AgentStatus::Completed);
    }

    #[test]
    fn project_loaded_task_collects_envelopes_and_token_usage() {
        let summary = PersistedTaskSummary {
            task_id: "review-task".to_string(),
            session_ref: "session-root".to_string(),
            parent_agent_session_ref: "agent-root".to_string(),
            child_session_ref: Some("session-child".to_string()),
            child_agent_session_ref: Some("agent-child".to_string()),
            role: "reviewer".to_string(),
            status: AgentStatus::Completed,
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            summary: "looks good".to_string(),
        };
        let task = AgentTaskSpec {
            task_id: summary.task_id.clone(),
            role: summary.role.clone(),
            prompt: "inspect the patch".to_string(),
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let child_session_id = SessionId::from("session-child");
        let child_agent_session_id = AgentSessionId::from("agent-child");
        let events = vec![
            SessionEventEnvelope::new(
                SessionId::from("session-root"),
                AgentSessionId::from("agent-root"),
                None,
                None,
                SessionEventKind::TaskCreated {
                    task: task.clone(),
                    parent_agent_id: None,
                },
            ),
            SessionEventEnvelope::new(
                SessionId::from("session-root"),
                AgentSessionId::from("agent-root"),
                None,
                None,
                SessionEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        AgentId::from("agent-reviewer"),
                        None,
                        child_session_id.clone(),
                        child_agent_session_id.clone(),
                        AgentEnvelopeKind::Input {
                            message: Message::user("running"),
                            delivery: agent::types::AgentInputDelivery::Queue,
                        },
                    ),
                },
            ),
            SessionEventEnvelope::new(
                SessionId::from("session-root"),
                AgentSessionId::from("agent-root"),
                None,
                None,
                SessionEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        AgentId::from("agent-reviewer"),
                        None,
                        child_session_id,
                        child_agent_session_id,
                        AgentEnvelopeKind::Artifact {
                            artifact: AgentArtifact {
                                kind: "note".to_string(),
                                uri: "memory://artifact".to_string(),
                                label: Some("note".to_string()),
                                metadata: None,
                            },
                        },
                    ),
                },
            ),
        ];

        let loaded = project_loaded_task(
            summary,
            &events,
            vec![Message::assistant("done")],
            vec![TokenUsageRecord {
                scope: TokenUsageScope::Task,
                session_id: SessionId::from("session-child"),
                agent_session_id: Some(AgentSessionId::from("agent-child")),
                agent_name: Some("reviewer".to_string()),
                task_id: Some("review-task".to_string()),
                ledger: TokenLedgerSnapshot::default(),
            }],
        )
        .unwrap();

        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.artifacts.len(), 1);
        assert_eq!(loaded.child_transcript.len(), 1);
        assert!(loaded.token_usage.is_some());
    }
}
