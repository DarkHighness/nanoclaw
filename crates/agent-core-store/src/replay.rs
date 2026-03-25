use agent_core_types::{Message, RunEventEnvelope, RunEventKind};

#[must_use]
pub fn replay_transcript(events: &[RunEventEnvelope]) -> Vec<Message> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            RunEventKind::TranscriptMessage { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::replay_transcript;
    use agent_core_types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId};

    #[test]
    fn replay_only_keeps_transcript_messages() {
        let events = vec![
            RunEventEnvelope::new(
                RunId::new(),
                SessionId::new(),
                None,
                None,
                RunEventKind::TranscriptMessage {
                    message: Message::user("hello"),
                },
            ),
            RunEventEnvelope::new(
                RunId::new(),
                SessionId::new(),
                None,
                None,
                RunEventKind::Stop { reason: None },
            ),
        ];
        assert_eq!(replay_transcript(&events).len(), 1);
    }
}
