use types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId, TurnId};

pub fn append_transcript_message(
    transcript: &mut Vec<Message>,
    message: Message,
    run_id: RunId,
    session_id: SessionId,
    turn_id: TurnId,
) -> RunEventEnvelope {
    transcript.push(message.clone());
    RunEventEnvelope::new(
        run_id,
        session_id,
        Some(turn_id),
        None,
        RunEventKind::TranscriptMessage { message },
    )
}
