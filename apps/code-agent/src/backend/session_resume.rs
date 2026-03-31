use agent::runtime::{RuntimeSession, reconstruct_runtime_session as rebuild_runtime_session};
use agent::types::{AgentSessionId, SessionEventEnvelope};
use anyhow::Result;

pub(crate) use agent::runtime::HISTORY_ONLY_RESUME_REASON;

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
    rebuild_runtime_session(events, agent_session_id).map_err(Into::into)
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
