use super::AgentRuntime;
use crate::{Result, RuntimeObserver, RuntimeProgressEvent, append_transcript_message};
use serde_json::json;
use std::collections::BTreeMap;
use types::{
    AgentCoreError, HookContext, HookEvent, HookRegistration, Message, SessionEventKind, TurnId,
};

impl AgentRuntime {
    pub(super) async fn prepare_user_turn(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        instructions: &[String],
        prompt: &str,
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

        self.submit_user_prompt(turn_id, hooks, prompt, observer)
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

    async fn submit_user_prompt(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        prompt: &str,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        let user_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: BTreeMap::new(),
                    payload: json!({ "prompt": prompt }),
                },
            )
            .await?;
        let user_effects = self
            .apply_hook_effects(
                turn_id,
                user_hooks,
                Some(Message::user(prompt.to_string())),
                None,
            )
            .await?;
        if let Some(reason) = user_effects.blocked_reason("user prompt blocked") {
            return Err(AgentCoreError::HookBlocked(reason).into());
        }
        let Some(user_message) = user_effects.current_message else {
            return Err(
                AgentCoreError::HookBlocked("user prompt removed by hook".to_string()).into(),
            );
        };

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
                prompt: prompt.to_string(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::UserPromptAdded {
            prompt: prompt.to_string(),
        })?;
        Ok(())
    }
}
