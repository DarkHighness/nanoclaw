use store::replay_transcript;
use types::{AgentSessionId, Message, MessageRole, SessionEventEnvelope, TurnId};

#[must_use]
pub fn agent_session_events(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
) -> Vec<SessionEventEnvelope> {
    events
        .iter()
        .filter(|event| &event.agent_session_id == agent_session_id)
        .cloned()
        .collect()
}

#[must_use]
pub fn focus_events(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
    turn_id: Option<&TurnId>,
) -> Vec<SessionEventEnvelope> {
    let scoped = agent_session_events(events, agent_session_id);
    match turn_id {
        Some(turn_id) => scoped
            .into_iter()
            .filter(|event| event.turn_id.as_ref() == Some(turn_id))
            .collect(),
        None => scoped,
    }
}

#[must_use]
pub fn agent_session_transcript(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
) -> Vec<Message> {
    replay_transcript(&agent_session_events(events, agent_session_id))
}

#[must_use]
pub fn focus_transcript(
    events: &[SessionEventEnvelope],
    agent_session_id: &AgentSessionId,
    turn_id: Option<&TurnId>,
) -> Vec<Message> {
    let focused = focus_events(events, agent_session_id, turn_id);
    let transcript = replay_transcript(&focused);
    if transcript.is_empty() && turn_id.is_some() {
        return agent_session_transcript(events, agent_session_id);
    }
    transcript
}

#[must_use]
pub fn last_user_prompt(transcript: &[Message]) -> Option<String> {
    transcript
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(Message::text_content)
        .filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{agent_session_transcript, focus_events, focus_transcript, last_user_prompt};
    use types::{
        AgentSessionId, Message, SessionEventEnvelope, SessionEventKind, SessionId, TurnId,
    };

    fn event(
        agent_session_id: &str,
        turn_id: Option<&str>,
        event: SessionEventKind,
    ) -> SessionEventEnvelope {
        SessionEventEnvelope::new(
            SessionId::from("session-replay"),
            AgentSessionId::from(agent_session_id),
            turn_id.map(TurnId::from),
            None,
            event,
        )
    }

    #[test]
    fn scopes_focus_events_to_turn_inside_agent_session() {
        let events = vec![
            event(
                "agent-a",
                Some("turn-1"),
                SessionEventKind::TranscriptMessage {
                    message: Message::user("prompt one"),
                },
            ),
            event(
                "agent-a",
                Some("turn-2"),
                SessionEventKind::TranscriptMessage {
                    message: Message::assistant("response two"),
                },
            ),
            event(
                "agent-b",
                Some("turn-1"),
                SessionEventKind::TranscriptMessage {
                    message: Message::user("other agent"),
                },
            ),
        ];

        let focused = focus_events(
            &events,
            &AgentSessionId::from("agent-a"),
            Some(&TurnId::from("turn-1")),
        );
        assert_eq!(focused.len(), 1);
        assert_eq!(focused[0].agent_session_id, AgentSessionId::from("agent-a"));
        assert_eq!(focused[0].turn_id, Some(TurnId::from("turn-1")));
    }

    #[test]
    fn falls_back_to_agent_session_transcript_when_turn_scope_has_no_messages() {
        let events = vec![
            event(
                "agent-a",
                Some("turn-1"),
                SessionEventKind::UserPromptSubmit {
                    prompt: "hello".to_string(),
                },
            ),
            event(
                "agent-a",
                Some("turn-1"),
                SessionEventKind::TranscriptMessage {
                    message: Message::user("hello"),
                },
            ),
        ];

        let transcript = focus_transcript(
            &events,
            &AgentSessionId::from("agent-a"),
            Some(&TurnId::from("turn-2")),
        );
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].text_content(), "hello");
    }

    #[test]
    fn extracts_last_user_prompt_from_transcript() {
        let transcript = agent_session_transcript(
            &[event(
                "agent-a",
                Some("turn-1"),
                SessionEventKind::TranscriptMessage {
                    message: Message::user("latest prompt"),
                },
            )],
            &AgentSessionId::from("agent-a"),
        );

        assert_eq!(
            last_user_prompt(&transcript).as_deref(),
            Some("latest prompt")
        );
    }
}
