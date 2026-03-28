use super::AgentRuntime;
use crate::{Result, RuntimeObserver, RuntimeProgressEvent, append_transcript_message};
use serde_json::json;
use std::collections::BTreeMap;
use types::{
    AgentCoreError, GateDecision, HookContext, HookEvent, HookRegistration, Message, RunEventKind,
    TurnId,
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
        self.ensure_session_started(turn_id, hooks).await?;
        self.record_instruction_load(turn_id, hooks, instructions)
            .await?;

        let async_context = self.hook_runner.drain_async_context().await;
        self.append_hook_context_messages(turn_id, &async_context)
            .await?;

        self.submit_user_prompt(turn_id, hooks, prompt, observer)
            .await
    }

    async fn ensure_session_started(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
    ) -> Result<()> {
        if self.session.session_started {
            return Ok(());
        }

        let session_start_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::SessionStart,
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: None,
                    fields: [("reason".to_string(), "new_session".to_string())]
                        .into_iter()
                        .collect(),
                    payload: json!({ "reason": "new_session" }),
                },
            )
            .await?;
        self.append_hook_context_messages(turn_id, &session_start_hooks)
            .await?;
        self.append_event(
            None,
            None,
            RunEventKind::SessionStart {
                reason: Some("new_session".to_string()),
            },
        )
        .await?;
        self.session.session_started = true;
        Ok(())
    }

    async fn record_instruction_load(
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
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("reason".to_string(), "runtime_instructions".to_string())]
                        .into_iter()
                        .collect(),
                    payload: json!({ "count": instructions.len() }),
                },
            )
            .await?;
        self.append_hook_context_messages(turn_id, &instruction_hooks)
            .await?;
        self.append_event(
            Some(turn_id.clone()),
            None,
            RunEventKind::InstructionsLoaded {
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
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: BTreeMap::new(),
                    payload: json!({ "prompt": prompt }),
                },
            )
            .await?;
        if matches!(user_hooks.gate_decision, Some(GateDecision::Block))
            || !user_hooks.continue_allowed
        {
            return Err(AgentCoreError::HookBlocked(
                user_hooks
                    .gate_reason
                    .or(user_hooks.stop_reason)
                    .unwrap_or_else(|| "user prompt blocked".to_string()),
            )
            .into());
        }
        self.append_hook_context_messages(turn_id, &user_hooks)
            .await?;

        let transcript_event = append_transcript_message(
            &mut self.session.transcript,
            Message::user(prompt.to_string()),
            self.session.run_id.clone(),
            self.session.session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(transcript_event).await?;
        self.append_event(
            Some(turn_id.clone()),
            None,
            RunEventKind::UserPromptSubmit {
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
