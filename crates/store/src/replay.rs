use std::collections::BTreeMap;
use types::{
    AgentSessionId, Message, MessageId, MessageRole, SessionEventEnvelope, SessionEventKind,
};

#[must_use]
pub fn replay_transcript(events: &[SessionEventEnvelope]) -> Vec<Message> {
    let mut transcript = Vec::<Option<Message>>::new();
    let mut by_message_id = BTreeMap::new();

    for event in events {
        match &event.event {
            SessionEventKind::TranscriptMessage { message } => {
                by_message_id.insert(message.message_id.clone(), transcript.len());
                transcript.push(Some(message.clone()));
            }
            SessionEventKind::TranscriptMessagePatched {
                message_id,
                message,
            } => {
                let Some(index) = by_message_id.get(message_id).copied() else {
                    continue;
                };
                let mut patched = message.clone();
                patched.message_id = message_id.clone();
                transcript[index] = Some(patched);
            }
            SessionEventKind::TranscriptMessageRemoved { message_id } => {
                let Some(index) = by_message_id.remove(message_id) else {
                    continue;
                };
                transcript[index] = None;
            }
            _ => {}
        }
    }

    transcript.into_iter().flatten().collect()
}

#[must_use]
pub fn visible_transcript(events: &[SessionEventEnvelope]) -> Vec<Message> {
    let mut agent_session_ids = Vec::<AgentSessionId>::new();
    for event in events {
        if agent_session_ids
            .iter()
            .any(|agent_session_id| agent_session_id == &event.agent_session_id)
        {
            continue;
        }
        agent_session_ids.push(event.agent_session_id.clone());
    }

    if agent_session_ids.len() <= 1 {
        return visible_agent_session_transcript(events);
    }

    let mut transcript = Vec::new();
    for agent_session_id in agent_session_ids {
        let scoped_events = events
            .iter()
            .filter(|event| event.agent_session_id == agent_session_id)
            .cloned()
            .collect::<Vec<_>>();
        transcript.extend(visible_agent_session_transcript(&scoped_events));
    }
    transcript
}

fn visible_agent_session_transcript(events: &[SessionEventEnvelope]) -> Vec<Message> {
    let transcript = replay_transcript(events);
    let Some((summary_message_id, retained_tail_message_ids)) =
        latest_compaction_checkpoint(events)
    else {
        return transcript;
    };

    let message_index = transcript
        .iter()
        .enumerate()
        .map(|(index, message)| (message.message_id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let Some(summary_index) = message_index.get(&summary_message_id).copied() else {
        return transcript;
    };

    let retained_tail_indices = retained_tail_message_ids
        .iter()
        .filter_map(|message_id| message_index.get(message_id).copied())
        .filter(|index| *index < summary_index)
        .collect::<Vec<_>>();
    let retained_tail_indices = upgrade_retained_tail_indices_for_request_rounds(
        &transcript,
        retained_tail_indices,
        summary_index,
    );

    let mut visible_indices =
        Vec::with_capacity(1 + retained_tail_indices.len() + transcript.len());
    visible_indices.push(summary_index);
    visible_indices.extend(retained_tail_indices);
    visible_indices.extend((summary_index + 1)..transcript.len());
    visible_indices
        .into_iter()
        .filter_map(|index| transcript.get(index).cloned())
        .collect()
}

fn latest_compaction_checkpoint(
    events: &[SessionEventEnvelope],
) -> Option<(MessageId, Vec<MessageId>)> {
    events.iter().rev().find_map(|event| match &event.event {
        SessionEventKind::CompactionCompleted {
            summary_message_id,
            retained_tail_message_ids,
            ..
        } => Some(
            summary_message_id
                .clone()
                .map(|summary_message_id| (summary_message_id, retained_tail_message_ids.clone())),
        ),
        _ => None,
    })?
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

    // Visible transcript projections should preserve the same request-side
    // prefix that resume and compaction continuity now keep when older
    // checkpoints only named the tail end of a request round.
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
    use super::{replay_transcript, visible_transcript};
    use types::{
        AgentSessionId, Message, MessageId, SessionEventEnvelope, SessionEventKind, SessionId,
    };

    #[test]
    fn replay_only_keeps_transcript_messages() {
        let events = vec![
            SessionEventEnvelope::new(
                SessionId::new(),
                AgentSessionId::new(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("hello"),
                },
            ),
            SessionEventEnvelope::new(
                SessionId::new(),
                AgentSessionId::new(),
                None,
                None,
                SessionEventKind::Stop { reason: None },
            ),
        ];
        assert_eq!(replay_transcript(&events).len(), 1);
    }

    #[test]
    fn replay_applies_message_patch_and_remove_events() {
        let session_id = SessionId::new();
        let agent_session_id = AgentSessionId::new();
        let message_id = MessageId::from("msg_1");
        let removed_id = MessageId::from("msg_2");
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("draft one").with_message_id(message_id.clone()),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::assistant("draft two").with_message_id(removed_id.clone()),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessagePatched {
                    message_id: message_id.clone(),
                    message: Message::user("patched one"),
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                None,
                None,
                SessionEventKind::TranscriptMessageRemoved {
                    message_id: removed_id,
                },
            ),
        ];

        let transcript = replay_transcript(&events);
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].message_id, message_id);
        assert_eq!(transcript[0].text_content(), "patched one");
    }

    #[test]
    fn visible_transcript_projects_compacted_window() {
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
                agent_session_id,
                None,
                None,
                SessionEventKind::TranscriptMessage { message: follow_up },
            ),
        ];

        let visible = visible_transcript(&events);
        assert_eq!(
            visible
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "summary".to_string(),
                "keep this".to_string(),
                "after compaction".to_string(),
            ]
        );
    }

    #[test]
    fn visible_transcript_backfills_request_round_prefix_for_older_checkpoints() {
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
                agent_session_id,
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

        let visible = visible_transcript(&events);
        assert_eq!(
            visible
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "summary".to_string(),
                "prefer terse answers".to_string(),
                "recalled workspace memory".to_string(),
                "real user prompt".to_string(),
                "latest assistant reply".to_string(),
            ]
        );
    }

    #[test]
    fn visible_transcript_ignores_older_checkpoint_when_latest_metadata_is_missing() {
        let session_id = SessionId::from("session_demo");
        let agent_session_id = AgentSessionId::from("agent_demo");
        let older_prompt =
            Message::user("older prompt").with_message_id(MessageId::from("msg_older_prompt"));
        let older_answer =
            Message::assistant("older answer").with_message_id(MessageId::from("msg_older_answer"));
        let older_summary =
            Message::system("older summary").with_message_id(MessageId::from("msg_older_summary"));
        let newer_summary =
            Message::system("newer summary").with_message_id(MessageId::from("msg_newer_summary"));
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
                    message: older_summary.clone(),
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
                    retained_message_count: 0,
                    summary_chars: 13,
                    summary_message_id: Some(older_summary.message_id.clone()),
                    retained_tail_message_ids: Vec::new(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: newer_summary,
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                None,
                None,
                SessionEventKind::CompactionCompleted {
                    reason: "manual".to_string(),
                    source_message_count: 1,
                    retained_message_count: 0,
                    summary_chars: 13,
                    summary_message_id: None,
                    retained_tail_message_ids: Vec::new(),
                },
            ),
        ];

        let visible = visible_transcript(&events);
        assert_eq!(
            visible
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "older prompt".to_string(),
                "older answer".to_string(),
                "older summary".to_string(),
                "newer summary".to_string(),
            ]
        );
    }

    #[test]
    fn visible_transcript_concatenates_agent_sessions_in_first_seen_order() {
        let session_id = SessionId::from("session_demo");
        let first_agent_session_id = AgentSessionId::from("agent_root");
        let second_agent_session_id = AgentSessionId::from("agent_rotated");
        let summary = Message::system("summary").with_message_id(MessageId::from("msg_summary"));
        let kept = Message::user("kept prompt").with_message_id(MessageId::from("msg_keep"));
        let after =
            Message::assistant("after compaction").with_message_id(MessageId::from("msg_after"));
        let fresh_prompt =
            Message::user("fresh prompt").with_message_id(MessageId::from("msg_fresh_prompt"));
        let fresh_answer =
            Message::assistant("fresh answer").with_message_id(MessageId::from("msg_fresh_answer"));
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("older prompt")
                        .with_message_id(MessageId::from("msg_old_prompt")),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::assistant("older answer")
                        .with_message_id(MessageId::from("msg_old_answer")),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: kept.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: summary.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
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
                session_id.clone(),
                first_agent_session_id,
                None,
                None,
                SessionEventKind::TranscriptMessage { message: after },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                second_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: fresh_prompt,
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                second_agent_session_id,
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: fresh_answer,
                },
            ),
        ];

        let visible = visible_transcript(&events);
        assert_eq!(
            visible
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "summary".to_string(),
                "kept prompt".to_string(),
                "after compaction".to_string(),
                "fresh prompt".to_string(),
                "fresh answer".to_string(),
            ]
        );
    }
}
