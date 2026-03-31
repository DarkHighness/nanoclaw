use super::AgentRuntime;
use crate::{Result, RuntimeObserver, RuntimeProgressEvent, append_transcript_message};
use serde_json::json;
use std::collections::BTreeMap;
use types::{
    AgentCoreError, HookContext, HookEvent, HookRegistration, Message, SessionEventKind, TurnId,
    message_operator_text,
};

impl AgentRuntime {
    pub(super) async fn prepare_user_turn(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        instructions: &[String],
        message: Message,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        self.clear_pending_request_effects();
        self.permission_grants.clear_turn();
        self.ensure_session_started(turn_id, hooks).await?;
        self.record_instruction_load(turn_id, hooks, instructions)
            .await?;

        let async_context = self.hook_runner.drain_async_invocations().await;
        let _ = self
            .apply_hook_effects(turn_id, async_context, None, None)
            .await?;
        let augmented = self.augment_user_message(message).await;

        self.submit_user_message(
            turn_id,
            hooks,
            augmented.prefix_messages,
            augmented.message,
            observer,
        )
        .await
    }

    async fn ensure_session_started(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
    ) -> Result<()> {
        self.start_agent_session(turn_id, hooks, "new_session")
            .await
    }

    pub(super) async fn record_instruction_load(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        instructions: &[String],
    ) -> Result<()> {
        if instructions.is_empty() {
            return Ok(());
        }

        let instruction_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::InstructionsLoaded,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("reason".to_string(), "runtime_instructions".to_string())]
                        .into_iter()
                        .collect(),
                    payload: json!({ "count": instructions.len() }),
                },
            )
            .await?;
        let instruction_effects = self
            .apply_hook_effects(turn_id, instruction_hooks, None, None)
            .await?;
        if let Some(reason) = instruction_effects.blocked_reason("instruction load blocked") {
            return Err(AgentCoreError::HookBlocked(reason).into());
        }
        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::InstructionsLoaded {
                count: instructions.len(),
            },
        )
        .await
    }

    async fn submit_user_message(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        prefix_messages: Vec<Message>,
        message: Message,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        let prompt = preview_user_message(&message);
        let user_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: BTreeMap::new(),
                    payload: json!({ "prompt": prompt, "message": message }),
                },
            )
            .await?;
        let user_effects = self
            .apply_hook_effects(turn_id, user_hooks, Some(message), None)
            .await?;
        if let Some(reason) = user_effects.blocked_reason("user prompt blocked") {
            return Err(AgentCoreError::HookBlocked(reason).into());
        }
        let Some(user_message) = user_effects.current_message else {
            return Err(
                AgentCoreError::HookBlocked("user prompt removed by hook".to_string()).into(),
            );
        };

        // Augmentor-produced recall/context must stay as distinct transcript
        // messages ahead of the operator prompt. Merging them back into the
        // user message would hide provenance and mutate the original prompt
        // bytes that hooks, logs, and providers should continue to observe.
        self.append_prefix_messages(turn_id, prefix_messages)
            .await?;
        let transcript_event = append_transcript_message(
            &mut self.session.transcript,
            user_message,
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(transcript_event).await?;
        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::UserPromptSubmit {
                prompt: prompt.clone(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::UserPromptAdded { prompt })?;
        Ok(())
    }

    async fn append_prefix_messages(
        &mut self,
        turn_id: &TurnId,
        messages: Vec<Message>,
    ) -> Result<()> {
        for message in messages {
            let transcript_event = append_transcript_message(
                &mut self.session.transcript,
                message,
                self.session.session_id.clone(),
                self.session.agent_session_id.clone(),
                turn_id.clone(),
            );
            self.store.append(transcript_event).await?;
        }
        Ok(())
    }
}

fn preview_user_message(message: &Message) -> String {
    // Hook payloads should use the same operator-visible renderer as the TUI and
    // session export. Falling back to `text_content()` here hides attachment-only
    // turns and typed references, which makes audit trails drift across surfaces.
    let text = message_operator_text(message);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "<structured user input>".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::preview_user_message;
    use types::{Message, MessagePart, MessageRole};

    #[test]
    fn preview_user_message_keeps_attachment_and_reference_markers() {
        let message = Message::new(
            MessageRole::User,
            vec![
                MessagePart::ImageUrl {
                    url: "https://example.com/failure.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                },
                MessagePart::reference(
                    "mention",
                    Some("workspace".to_string()),
                    Some("app://workspace/snapshot".to_string()),
                    None,
                ),
            ],
        );

        assert_eq!(
            preview_user_message(&message),
            "[image_url:https://example.com/failure.png image/png]\n[reference:mention workspace app://workspace/snapshot]"
        );
    }
}
