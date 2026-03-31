use crate::{Result, RuntimeError};
use std::collections::{BTreeMap, HashSet};
use store::SessionStore;
use types::{
    AgentSessionId, Message, MessageId, MessageRole, ProviderContinuation, SessionEventEnvelope,
    SessionEventKind, SessionId, TokenLedgerSnapshot,
};

pub const HISTORY_ONLY_RESUME_REASON: &str =
    "This persisted agent session predates resume checkpoints for compacted history.";

#[derive(Clone, Debug)]
pub struct RuntimeSession {
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub transcript: Vec<Message>,
    pub provider_continuation: Option<ProviderContinuation>,
    pub provider_transcript_cursor: usize,
    pub compaction_summary_index: Option<usize>,
    pub retained_tail_indices: Vec<usize>,
    pub post_summary_start: usize,
    pub removed_message_ids: HashSet<MessageId>,
    pub agent_session_started: bool,
    pub token_ledger: TokenLedgerSnapshot,
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self::new(SessionId::new(), AgentSessionId::new())
    }
}

impl RuntimeSession {
    #[must_use]
    pub fn new(session_id: SessionId, agent_session_id: AgentSessionId) -> Self {
        Self {
            session_id,
            agent_session_id,
            transcript: Vec::new(),
            provider_continuation: None,
            provider_transcript_cursor: 0,
            compaction_summary_index: None,
            retained_tail_indices: Vec::new(),
            post_summary_start: 0,
            removed_message_ids: HashSet::new(),
            agent_session_started: false,
            token_ledger: TokenLedgerSnapshot::default(),
        }
    }

    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.agent_session_started
            || !self.transcript.is_empty()
            || self.provider_continuation.is_some()
            || self.provider_transcript_cursor > 0
            || self.compaction_summary_index.is_some()
            || !self.retained_tail_indices.is_empty()
            || self.post_summary_start > 0
            || !self.removed_message_ids.is_empty()
            || self.token_ledger != TokenLedgerSnapshot::default()
    }
}

#[derive(Clone, Debug)]
struct CompactionCheckpoint {
    summary_message_id: MessageId,
    retained_tail_message_ids: Vec<MessageId>,
}

/// Rebuild a runtime session window for one persisted agent session.
///
/// The reconstructed session intentionally keeps provider continuation state
/// empty because persisted event logs only carry transcript messages and
/// compaction checkpoints. A resumed runtime replays from transcript state and
/// starts a fresh provider exchange for the next turn.
pub fn reconstruct_runtime_session(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
) -> Result<RuntimeSession> {
    let cutoff = events
        .iter()
        .rposition(|event| &event.agent_session_id == agent_session_id)
        .ok_or_else(|| {
            RuntimeError::invalid_state(format!(
                "agent session missing from persisted event log: {agent_session_id}"
            ))
        })?;
    let slice = &events[..=cutoff];
    let session_id = slice
        .last()
        .map(|event| event.session_id.clone())
        .unwrap_or_else(SessionId::new);
    let transcript = store::replay_transcript(slice);

    let mut session = RuntimeSession::new(session_id, agent_session_id.clone());
    session.transcript = transcript;

    if let Some(checkpoint) = latest_compaction_checkpoint(slice)? {
        let message_index = session
            .transcript
            .iter()
            .enumerate()
            .map(|(index, message)| (message.message_id.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let summary_index = message_index
            .get(&checkpoint.summary_message_id)
            .copied()
            .ok_or_else(|| {
                RuntimeError::invalid_state(format!(
                    "compaction summary message missing from reconstructed transcript: {}",
                    checkpoint.summary_message_id
                ))
            })?;
        session.compaction_summary_index = Some(summary_index);
        let retained_tail_indices = checkpoint
            .retained_tail_message_ids
            .iter()
            .filter_map(|message_id| message_index.get(message_id).copied())
            .filter(|index| *index < summary_index)
            .collect();
        session.retained_tail_indices = upgrade_retained_tail_indices_for_request_rounds(
            &session.transcript,
            retained_tail_indices,
            summary_index,
        );
        session.post_summary_start = summary_index + 1;
    }

    Ok(session)
}

#[must_use]
pub fn fork_runtime_session(
    parent: &RuntimeSession,
    session_id: SessionId,
    agent_session_id: AgentSessionId,
) -> RuntimeSession {
    // Forked child sessions inherit the parent's reconstructed transcript window
    // but must start a fresh provider exchange and token ledger under their own
    // runtime ids.
    let mut session = parent.clone();
    session.session_id = session_id;
    session.agent_session_id = agent_session_id;
    session.provider_continuation = None;
    session.provider_transcript_cursor = 0;
    session.agent_session_started = false;
    session.token_ledger = TokenLedgerSnapshot::default();
    session
}

pub fn seed_runtime_session_events(session: &RuntimeSession) -> Result<Vec<SessionEventEnvelope>> {
    let mut events = session
        .transcript
        .iter()
        .cloned()
        .map(|message| {
            SessionEventEnvelope::new(
                session.session_id.clone(),
                session.agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage { message },
            )
        })
        .collect::<Vec<_>>();

    if let Some(summary_index) = session.compaction_summary_index {
        let summary_message = session.transcript.get(summary_index).ok_or_else(|| {
            RuntimeError::invalid_state(format!(
                "forked session summary index out of bounds: {summary_index}"
            ))
        })?;
        let retained_tail_message_ids = session
            .retained_tail_indices
            .iter()
            .map(|index| {
                session
                    .transcript
                    .get(*index)
                    .map(|message| message.message_id.clone())
                    .ok_or_else(|| {
                        RuntimeError::invalid_state(format!(
                            "forked session retained tail index out of bounds: {index}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let retained_before_summary = session
            .retained_tail_indices
            .iter()
            .filter(|index| **index < summary_index)
            .count();
        events.push(SessionEventEnvelope::new(
            session.session_id.clone(),
            session.agent_session_id.clone(),
            None,
            None,
            SessionEventKind::CompactionCompleted {
                reason: "fork_context".to_string(),
                source_message_count: summary_index.saturating_sub(retained_before_summary),
                retained_message_count: retained_tail_message_ids.len(),
                summary_chars: summary_message.text_content().chars().count(),
                summary_message_id: Some(summary_message.message_id.clone()),
                retained_tail_message_ids,
            },
        ));
    }

    Ok(events)
}

pub async fn load_runtime_session(
    store: &(dyn SessionStore + Send + Sync),
    session_id: &SessionId,
    agent_session_id: &AgentSessionId,
) -> Result<RuntimeSession> {
    let events = store.events(session_id).await?;
    reconstruct_runtime_session(&events, agent_session_id)
}

fn latest_compaction_checkpoint(
    events: &[SessionEventEnvelope],
) -> Result<Option<CompactionCheckpoint>> {
    let checkpoint = events.iter().rev().find_map(|event| match &event.event {
        SessionEventKind::CompactionCompleted {
            summary_message_id,
            retained_tail_message_ids,
            ..
        } => Some((
            summary_message_id.clone(),
            retained_tail_message_ids.clone(),
        )),
        _ => None,
    });

    match checkpoint {
        None => Ok(None),
        Some((Some(summary_message_id), retained_tail_message_ids)) => {
            Ok(Some(CompactionCheckpoint {
                summary_message_id,
                retained_tail_message_ids,
            }))
        }
        Some((None, _)) => Err(RuntimeError::invalid_state(HISTORY_ONLY_RESUME_REASON)),
    }
}

fn upgrade_retained_tail_indices_for_request_rounds(
    transcript: &[Message],
    retained_tail_indices: Vec<usize>,
    summary_index: usize,
) -> Vec<usize> {
    let Some(&first_retained_index) = retained_tail_indices.first() else {
        return retained_tail_indices;
    };
    if first_retained_index >= summary_index {
        return retained_tail_indices;
    }

    let mut upgraded_start = first_retained_index;
    while upgraded_start > 0 && !starts_request_round(transcript, upgraded_start) {
        upgraded_start -= 1;
    }
    if upgraded_start == first_retained_index {
        return retained_tail_indices;
    }

    // Older checkpoints may have kept only the tail end of a request round.
    // Resume should project the same request-side context shape that the live
    // runtime now preserves across compaction boundaries.
    let mut upgraded = (upgraded_start..first_retained_index).collect::<Vec<_>>();
    upgraded.extend(retained_tail_indices);
    upgraded
}

fn starts_request_round(transcript: &[Message], index: usize) -> bool {
    let Some(message) = transcript.get(index) else {
        return false;
    };
    if !matches!(message.role, MessageRole::User | MessageRole::System) {
        return false;
    }
    if index == 0 {
        return true;
    }
    matches!(
        transcript.get(index - 1).map(|message| &message.role),
        Some(MessageRole::Assistant | MessageRole::Tool)
    )
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_ONLY_RESUME_REASON, reconstruct_runtime_session};
    use types::{
        AgentSessionId, Message, MessageId, SessionEventEnvelope, SessionEventKind, SessionId,
    };

    #[test]
    fn reconstructs_transcript_window_from_latest_compaction_checkpoint() {
        let session_id = SessionId::from("session_demo");
        let agent_session_id = AgentSessionId::from("agent_demo");
        let first = Message::user("draft one").with_message_id(MessageId::from("msg_one"));
        let second = Message::assistant("draft two").with_message_id(MessageId::from("msg_two"));
        let kept = Message::user("keep this").with_message_id(MessageId::from("msg_keep"));
        let summary = Message::system("summary").with_message_id(MessageId::from("msg_summary"));
        let follow_up =
            Message::assistant("after compaction").with_message_id(MessageId::from("msg_after"));
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage { message: first },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage { message: second },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: kept.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: summary.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::CompactionCompleted {
                    reason: "manual".to_string(),
                    source_message_count: 2,
                    retained_message_count: 1,
                    summary_chars: 7,
                    summary_message_id: Some(summary.message_id.clone()),
                    retained_tail_message_ids: vec![kept.message_id.clone()],
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage { message: follow_up },
            ),
        ];

        let reconstructed = reconstruct_runtime_session(&events, &agent_session_id).unwrap();
        assert_eq!(reconstructed.compaction_summary_index, Some(3));
        assert_eq!(reconstructed.retained_tail_indices, vec![2]);
        assert_eq!(reconstructed.post_summary_start, 4);
    }

    #[test]
    fn rejects_compacted_sessions_without_resume_checkpoint_metadata() {
        let session_id = SessionId::from("session_demo");
        let agent_session_id = AgentSessionId::from("agent_demo");
        let events = vec![SessionEventEnvelope::new(
            session_id,
            agent_session_id.clone(),
            None,
            None,
            SessionEventKind::CompactionCompleted {
                reason: "manual".to_string(),
                source_message_count: 2,
                retained_message_count: 1,
                summary_chars: 7,
                summary_message_id: None,
                retained_tail_message_ids: Vec::new(),
            },
        )];

        let error = reconstruct_runtime_session(&events, &agent_session_id).unwrap_err();
        assert!(error.to_string().contains(HISTORY_ONLY_RESUME_REASON));
    }

    #[test]
    fn resume_backfills_request_round_prefix_for_older_checkpoints() {
        let session_id = SessionId::from("session_demo");
        let agent_session_id = AgentSessionId::from("agent_demo");
        let older_prompt =
            Message::user("older prompt").with_message_id(MessageId::from("msg_older_user"));
        let older_answer = Message::assistant("older answer")
            .with_message_id(MessageId::from("msg_older_assistant"));
        let steer =
            Message::system("prefer terse answers").with_message_id(MessageId::from("msg_steer"));
        let recall = Message::user("recalled workspace memory")
            .with_message_id(MessageId::from("msg_recall"));
        let prompt =
            Message::user("real user prompt").with_message_id(MessageId::from("msg_prompt"));
        let reply = Message::assistant("latest assistant reply")
            .with_message_id(MessageId::from("msg_reply"));
        let summary = Message::system("summary").with_message_id(MessageId::from("msg_summary"));
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: older_prompt,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: older_answer,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: steer.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: recall.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: prompt.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: reply.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: summary.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::CompactionCompleted {
                    reason: "manual".to_string(),
                    source_message_count: 3,
                    retained_message_count: 1,
                    summary_chars: 7,
                    summary_message_id: Some(summary.message_id.clone()),
                    retained_tail_message_ids: vec![reply.message_id.clone()],
                },
            ),
        ];

        let reconstructed = reconstruct_runtime_session(&events, &agent_session_id).unwrap();
        assert_eq!(reconstructed.compaction_summary_index, Some(6));
        assert_eq!(reconstructed.retained_tail_indices, vec![2, 3, 4, 5]);
        assert_eq!(reconstructed.post_summary_start, 7);
    }
}
