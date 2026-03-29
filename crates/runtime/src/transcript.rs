use types::{AgentSessionId, Message, SessionEventEnvelope, SessionEventKind, SessionId, TurnId};

pub fn append_transcript_message(
    transcript: &mut Vec<Message>,
    message: Message,
    session_id: SessionId,
    agent_session_id: AgentSessionId,
    turn_id: TurnId,
) -> SessionEventEnvelope {
    transcript.push(message.clone());
    SessionEventEnvelope::new(
        session_id,
        agent_session_id,
        Some(turn_id),
        None,
        SessionEventKind::TranscriptMessage { message },
    )
}
