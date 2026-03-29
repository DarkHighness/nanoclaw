use std::collections::BTreeMap;
use types::{Message, SessionEventEnvelope, SessionEventKind};

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

#[cfg(test)]
mod tests {
    use super::replay_transcript;
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
}
