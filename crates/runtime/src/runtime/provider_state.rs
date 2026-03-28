use super::AgentRuntime;
use crate::RuntimeError;
use serde_json::json;
use types::{AgentCoreError, Message, ModelRequest, ProviderContinuation, TurnId};

impl AgentRuntime {
    pub(super) fn build_model_request(
        &self,
        turn_id: &TurnId,
        instructions: &[String],
        force_full_transcript: bool,
    ) -> ModelRequest {
        let (messages, continuation) = if force_full_transcript {
            (self.visible_transcript(), None)
        } else {
            self.request_window()
        };
        ModelRequest {
            run_id: self.session.run_id.clone(),
            session_id: self.session.session_id.clone(),
            turn_id: turn_id.clone(),
            instructions: instructions.to_vec(),
            messages,
            tools: self.tool_registry.specs(),
            additional_context: Vec::new(),
            continuation,
            metadata: json!({}),
        }
    }

    fn request_window(&self) -> (Vec<Message>, Option<ProviderContinuation>) {
        let capabilities = self.backend.capabilities();
        if !capabilities.provider_managed_history {
            return (self.visible_transcript(), None);
        }

        let Some(continuation) = self.session.provider_continuation.clone() else {
            return (self.visible_transcript(), None);
        };

        // Provider-managed chaining references the prior upstream response and
        // sends only append-only transcript growth after that response. This
        // avoids reserializing the visible transcript while keeping runtime
        // history itself immutable on disk and in memory.
        let start = self
            .session
            .provider_transcript_cursor
            .min(self.session.transcript.len());
        (
            self.session.transcript[start..].to_vec(),
            Some(continuation),
        )
    }

    pub(super) fn update_provider_continuation(
        &mut self,
        continuation: Option<ProviderContinuation>,
    ) {
        self.session.provider_continuation = continuation;
        self.session.provider_transcript_cursor = self.session.transcript.len();
    }

    pub(super) fn reset_provider_continuation(&mut self) {
        self.session.provider_continuation = None;
        self.session.provider_transcript_cursor = 0;
    }
}

pub(super) fn is_provider_continuation_lost(error: &RuntimeError) -> bool {
    matches!(
        error,
        RuntimeError::AgentCore(AgentCoreError::ProviderContinuationLost(_))
    )
}
