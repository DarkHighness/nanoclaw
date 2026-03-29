use types::{AgentSessionId, Message, RunEventEnvelope, RunEventKind, RunId, TurnId};

pub fn append_transcript_message(
    transcript: &mut Vec<Message>,
    message: Message,
    run_id: RunId,
    agent_session_id: AgentSessionId,
    turn_id: TurnId,
) -> RunEventEnvelope {
    transcript.push(message.clone());
    RunEventEnvelope::new(
        run_id,
        agent_session_id,
        Some(turn_id),
        None,
        RunEventKind::TranscriptMessage { message },
    )
}
