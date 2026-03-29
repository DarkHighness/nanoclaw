use agent::runtime::RuntimeSession;
use agent::types::{AgentSessionId, MessageId, SessionEventEnvelope, SessionEventKind, SessionId};
use anyhow::{Result, anyhow};
use store::replay_transcript;

pub(crate) const HISTORY_ONLY_RESUME_REASON: &str =
    "This persisted agent session predates resume checkpoints for compacted history.";

pub(crate) fn can_resume_agent_session(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
) -> Result<()> {
    reconstruct_runtime_session(events, agent_session_id).map(|_| ())
}

pub(crate) fn reconstruct_runtime_session(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
) -> Result<RuntimeSession> {
    let cutoff = events
        .iter()
        .rposition(|event| &event.agent_session_id == agent_session_id)
        .ok_or_else(|| {
            anyhow!("agent session missing from persisted event log: {agent_session_id}")
        })?;
    let slice = &events[..=cutoff];
    let session_id = slice
        .last()
        .map(|event| event.session_id.clone())
        .unwrap_or_else(SessionId::new);
    let transcript = replay_transcript(slice);

    let mut session = RuntimeSession::new(session_id, agent_session_id.clone());
    session.transcript = transcript;

    if let Some(checkpoint) = latest_compaction_checkpoint(slice)? {
        let message_index = session
            .transcript
            .iter()
            .enumerate()
            .map(|(index, message)| (message.message_id.clone(), index))
            .collect::<std::collections::BTreeMap<_, _>>();
        let summary_index = message_index
            .get(&checkpoint.summary_message_id)
            .copied()
            .ok_or_else(|| {
                anyhow!(
                    "compaction summary message missing from reconstructed transcript: {}",
                    checkpoint.summary_message_id
                )
            })?;
        session.compaction_summary_index = Some(summary_index);
        session.retained_tail_indices = checkpoint
            .retained_tail_message_ids
            .iter()
            .filter_map(|message_id| message_index.get(message_id).copied())
            .filter(|index| *index < summary_index)
            .collect();
        session.post_summary_start = summary_index + 1;
    }

    Ok(session)
}

#[derive(Clone, Debug)]
struct CompactionCheckpoint {
    summary_message_id: MessageId,
    retained_tail_message_ids: Vec<MessageId>,
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
        Some((None, _)) => Err(anyhow!(HISTORY_ONLY_RESUME_REASON)),
    }
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_ONLY_RESUME_REASON, reconstruct_runtime_session};
    use agent::types::{
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
}
