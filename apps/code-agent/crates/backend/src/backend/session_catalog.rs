use crate::backend::session_resume::{
    HISTORY_ONLY_RESUME_REASON, can_resume_agent_session, reconstruct_runtime_session,
    visible_transcript,
};
use crate::ui::{
    PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
    ResumeSupport,
};
use agent::types::{AgentSessionId, SessionEventEnvelope, SessionEventKind};
#[cfg(test)]
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use store::{SessionSearchResult, SessionSummary, replay_transcript};

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSessionResumeStatus {
    pub agent_session_ref: String,
    pub session_ref: String,
    pub support: ResumeSupport,
}

const TOP_LEVEL_HISTORY_ONLY_REASON: &str =
    "Session history can be inspected, but top-level session refs are not direct resume targets.";

pub fn persisted_session_summary(
    summary: &SessionSummary,
    active_session_ref: &str,
    session_title: Option<String>,
) -> PersistedSessionSummary {
    PersistedSessionSummary {
        session_ref: summary.session_id.to_string(),
        first_timestamp_ms: summary.first_timestamp_ms,
        last_timestamp_ms: summary.last_timestamp_ms,
        event_count: summary.event_count,
        worker_session_count: summary.agent_session_count,
        transcript_message_count: summary.transcript_message_count,
        session_title,
        last_user_prompt: summary.last_user_prompt.clone(),
        token_usage: summary.token_usage.clone(),
        resume_support: session_resume_support_for(summary.session_id.as_str(), active_session_ref),
    }
}

pub fn persisted_session_search_match(
    result: &SessionSearchResult,
    active_session_ref: &str,
    session_title: Option<String>,
) -> PersistedSessionSearchMatch {
    PersistedSessionSearchMatch {
        summary: persisted_session_summary(&result.summary, active_session_ref, session_title),
        matched_event_count: result.matched_event_count,
        preview_matches: result.preview_matches.clone(),
    }
}

pub fn persisted_agent_session_summaries(
    session_ref: &str,
    session_title: Option<&str>,
    events: &[SessionEventEnvelope],
    active_agent_session_ref: &str,
) -> Vec<PersistedAgentSessionSummary> {
    #[derive(Clone, Debug)]
    struct AgentSessionAccumulator {
        label: Option<String>,
        first_timestamp_ms: u128,
        last_timestamp_ms: u128,
        event_count: usize,
        last_user_prompt: Option<String>,
    }

    let Some(root_agent_session_id) = events.first().map(|event| event.agent_session_id.clone())
    else {
        return Vec::new();
    };

    let mut by_agent_session = BTreeMap::<AgentSessionId, AgentSessionAccumulator>::new();
    for event in events {
        let entry = by_agent_session
            .entry(event.agent_session_id.clone())
            .or_insert_with(|| AgentSessionAccumulator {
                label: None,
                first_timestamp_ms: event.timestamp_ms,
                last_timestamp_ms: event.timestamp_ms,
                event_count: 0,
                last_user_prompt: None,
            });
        entry.first_timestamp_ms = entry.first_timestamp_ms.min(event.timestamp_ms);
        entry.last_timestamp_ms = entry.last_timestamp_ms.max(event.timestamp_ms);
        entry.event_count += 1;
        match &event.event {
            SessionEventKind::UserPromptSubmit { prompt } => {
                let preview = prompt.preview_text();
                if !preview.is_empty() {
                    entry.last_user_prompt = Some(preview);
                }
            }
            SessionEventKind::SubagentStart { task, .. } => {
                entry.label.get_or_insert_with(|| task.role.clone());
            }
            _ => {}
        }
    }

    let mut summaries = by_agent_session
        .into_iter()
        .map(|(agent_session_id, entry)| PersistedAgentSessionSummary {
            agent_session_ref: agent_session_id.to_string(),
            session_ref: session_ref.to_string(),
            label: if agent_session_id == root_agent_session_id {
                "root".to_string()
            } else {
                entry.label.unwrap_or_else(|| "worker".to_string())
            },
            first_timestamp_ms: entry.first_timestamp_ms,
            last_timestamp_ms: entry.last_timestamp_ms,
            event_count: entry.event_count,
            transcript_message_count: visible_agent_session_transcript_message_count(
                events,
                &agent_session_id,
            ),
            session_title: session_title.map(ToString::to_string),
            last_user_prompt: entry.last_user_prompt,
            resume_support: agent_session_resume_support_for(
                events,
                &agent_session_id,
                active_agent_session_ref,
            ),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .last_timestamp_ms
            .cmp(&left.last_timestamp_ms)
            .then_with(|| left.agent_session_ref.cmp(&right.agent_session_ref))
    });
    summaries
}

fn visible_agent_session_transcript_message_count(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
) -> usize {
    let scoped_events = events
        .iter()
        .filter(|event| &event.agent_session_id == agent_session_id)
        .cloned()
        .collect::<Vec<_>>();
    reconstruct_runtime_session(&scoped_events, agent_session_id)
        .map(|session| visible_transcript(&session).len())
        .unwrap_or_else(|_| replay_transcript(&scoped_events).len())
}

#[cfg(test)]
pub fn resolve_agent_session_resume_status(
    agent_sessions: &[PersistedAgentSessionSummary],
    agent_session_ref: &str,
) -> Result<AgentSessionResumeStatus> {
    let summary = resolve_agent_session_reference(agent_sessions, agent_session_ref)?;
    Ok(AgentSessionResumeStatus {
        agent_session_ref: summary.agent_session_ref.clone(),
        session_ref: summary.session_ref.clone(),
        support: summary.resume_support.clone(),
    })
}

fn session_resume_support_for(session_ref: &str, active_session_ref: &str) -> ResumeSupport {
    if session_ref == active_session_ref {
        ResumeSupport::AttachedToActiveRuntime
    } else {
        ResumeSupport::NotYetSupported {
            reason: TOP_LEVEL_HISTORY_ONLY_REASON.to_string(),
        }
    }
}

fn agent_session_resume_support_for(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
    active_agent_session_ref: &str,
) -> ResumeSupport {
    if agent_session_id.as_str() == active_agent_session_ref {
        return ResumeSupport::AttachedToActiveRuntime;
    }
    match can_resume_agent_session(events, agent_session_id) {
        Ok(()) => ResumeSupport::Reattachable,
        Err(_) => ResumeSupport::NotYetSupported {
            reason: HISTORY_ONLY_RESUME_REASON.to_string(),
        },
    }
}

#[cfg(test)]
pub fn resolve_agent_session_reference<'a>(
    agent_sessions: &'a [PersistedAgentSessionSummary],
    agent_session_ref: &str,
) -> Result<&'a PersistedAgentSessionSummary> {
    if let Some(summary) = agent_sessions
        .iter()
        .find(|summary| summary.agent_session_ref == agent_session_ref)
    {
        return Ok(summary);
    }

    let matches = agent_sessions
        .iter()
        .filter(|summary| summary.agent_session_ref.starts_with(agent_session_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow!(
            "unknown agent session id or prefix: {agent_session_ref}"
        )),
        [summary] => Ok(summary),
        _ => Err(anyhow!(
            "ambiguous agent session prefix {agent_session_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|summary| summary
                    .agent_session_ref
                    .chars()
                    .take(8)
                    .collect::<String>())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ResumeSupport, persisted_agent_session_summaries, persisted_session_summary,
        resolve_agent_session_resume_status,
    };
    use crate::backend::session_resume::HISTORY_ONLY_RESUME_REASON;
    use agent::types::{
        AgentSessionId, SessionEventEnvelope, SessionEventKind, SessionId, SubmittedPromptSnapshot,
    };
    use store::SessionSummary;

    #[test]
    fn active_runtime_session_reports_attached_resume_support() {
        let summary = persisted_session_summary(
            &SessionSummary {
                session_id: SessionId::from("active_session"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 4,
                last_user_prompt: Some("inspect".to_string()),
                token_usage: None,
            },
            "active_session",
            Some("Active session title".to_string()),
        );

        assert_eq!(
            summary.resume_support,
            ResumeSupport::AttachedToActiveRuntime
        );
        assert_eq!(
            summary.session_title.as_deref(),
            Some("Active session title")
        );
    }

    #[test]
    fn persisted_agent_session_resume_status_is_explicitly_history_only() {
        let status = resolve_agent_session_resume_status(
            &[super::PersistedAgentSessionSummary {
                agent_session_ref: "agent_archived".to_string(),
                session_ref: "session_archived".to_string(),
                label: "root".to_string(),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                transcript_message_count: 1,
                session_title: None,
                last_user_prompt: None,
                resume_support: ResumeSupport::NotYetSupported {
                    reason: HISTORY_ONLY_RESUME_REASON.to_string(),
                },
            }],
            "agent_archived",
        )
        .unwrap();
        assert_eq!(status.support.label(), "history-only");
        match status.support {
            ResumeSupport::AttachedToActiveRuntime => {
                panic!("expected persisted history to stay history-only")
            }
            ResumeSupport::Reattachable => {
                panic!("expected persisted history to stay history-only")
            }
            ResumeSupport::NotYetSupported { reason } => {
                assert!(reason.contains("predates resume checkpoints"));
            }
        }
    }

    #[test]
    fn persisted_agent_session_summaries_group_root_and_worker_windows() {
        let session_id = SessionId::from("session_demo");
        let root_agent_session_id = AgentSessionId::from("agent_root");
        let worker_agent_session_id = AgentSessionId::from("agent_worker");
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::SessionStart {
                    reason: Some("new_session".to_string()),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("inspect"),
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                worker_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::SessionStart {
                    reason: Some("subagent".to_string()),
                },
            ),
        ];

        let summaries = persisted_agent_session_summaries(
            "session_demo",
            Some("Deploy rollback follow-up"),
            &events,
            root_agent_session_id.as_str(),
        );
        assert_eq!(summaries.len(), 2);
        let worker = summaries
            .iter()
            .find(|summary| summary.agent_session_ref == "agent_worker")
            .unwrap();
        assert_eq!(worker.label, "worker");
        assert_eq!(worker.resume_support.label(), "reattachable");

        let root = summaries
            .iter()
            .find(|summary| summary.agent_session_ref == "agent_root")
            .unwrap();
        assert_eq!(root.label, "root");
        assert_eq!(
            root.session_title.as_deref(),
            Some("Deploy rollback follow-up")
        );
        assert_eq!(root.last_user_prompt.as_deref(), Some("inspect"));
        assert_eq!(root.resume_support.label(), "attached");
    }
}
