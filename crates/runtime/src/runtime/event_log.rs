use super::AgentRuntime;
use crate::{Result, append_transcript_message};
use types::{
    Message, SessionEventEnvelope, SessionEventKind, ToolCall, ToolLifecycleEventEnvelope, TurnId,
};

impl AgentRuntime {
    pub(super) async fn append_event(
        &self,
        turn_id: Option<TurnId>,
        tool_call_id: Option<types::ToolCallId>,
        event: SessionEventKind,
    ) -> Result<()> {
        self.store
            .append(SessionEventEnvelope::new(
                self.session.session_id.clone(),
                self.session.agent_session_id.clone(),
                turn_id,
                tool_call_id,
                event,
            ))
            .await?;
        Ok(())
    }

    pub(super) async fn append_tool_lifecycle_event(
        &self,
        turn_id: &TurnId,
        call: &ToolCall,
        event: SessionEventKind,
    ) -> Result<ToolLifecycleEventEnvelope> {
        // Tool lifecycle updates are one of the few events that outer hosts
        // often need both live and durably. Build the canonical SessionEventEnvelope
        // once, append it, then project the host-facing typed event from it.
        let envelope = SessionEventEnvelope::new(
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
            Some(turn_id.clone()),
            Some(call.id.clone()),
            event,
        );
        let lifecycle = envelope
            .tool_lifecycle_event()
            .expect("tool lifecycle event");
        self.store.append(envelope).await?;
        Ok(lifecycle)
    }

    pub(super) async fn append_hook_messages(
        &mut self,
        turn_id: &TurnId,
        messages: &[Message],
    ) -> Result<()> {
        for message in messages {
            let event = append_transcript_message(
                &mut self.session.transcript,
                message.clone(),
                self.session.session_id.clone(),
                self.session.agent_session_id.clone(),
                turn_id.clone(),
            );
            self.store.append(event).await?;
        }
        Ok(())
    }

    pub(super) async fn append_turn_failure_event(
        &self,
        turn_id: &TurnId,
        stage: impl Into<String>,
        error: impl Into<String>,
    ) -> Result<()> {
        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::TurnFailed {
                stage: stage.into(),
                error: error.into(),
            },
        )
        .await
    }
}
