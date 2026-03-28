use super::AgentRuntime;
use crate::{
    CompactionRequest, Result, RuntimeObserver, RuntimeProgressEvent, append_transcript_message,
    estimate_prompt_tokens,
};
use serde_json::json;
use types::{GateDecision, HookContext, HookEvent, Message, TurnId};

impl AgentRuntime {
    fn visible_message_indices(&self) -> Vec<usize> {
        if let Some(summary_index) = self.session.compaction_summary_index {
            let mut indices = Vec::with_capacity(
                1 + self.session.retained_tail_indices.len() + self.session.transcript.len(),
            );
            indices.push(summary_index);
            indices.extend(
                self.session
                    .retained_tail_indices
                    .iter()
                    .copied()
                    .filter(|index| *index < summary_index),
            );
            indices.extend(self.session.post_summary_start..self.session.transcript.len());
            indices
        } else {
            (0..self.session.transcript.len()).collect()
        }
    }

    pub(super) fn visible_transcript(&self) -> Vec<Message> {
        self.visible_message_indices()
            .into_iter()
            .filter_map(|index| self.session.transcript.get(index).cloned())
            .collect()
    }

    pub(super) async fn compact_if_needed(
        &mut self,
        turn_id: &TurnId,
        instructions: &[String],
        observer: &mut dyn RuntimeObserver,
    ) -> Result<bool> {
        if !self.compaction_config.enabled {
            return Ok(false);
        }
        if self.backend.capabilities().provider_managed_history
            && self.session.provider_continuation.is_some()
        {
            return Ok(false);
        }
        let visible_messages = self.visible_transcript();
        if visible_messages.len() < 3 {
            return Ok(false);
        }

        let estimated_tokens =
            estimate_prompt_tokens(instructions, &visible_messages, &self.tool_registry.specs());
        if estimated_tokens < self.compaction_config.trigger_tokens {
            return Ok(false);
        }

        self.compact_visible_history(turn_id, "auto", None, observer)
            .await
    }

    pub(super) async fn compact_visible_history(
        &mut self,
        turn_id: &TurnId,
        reason: &str,
        instructions: Option<String>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<bool> {
        let visible_indices = self.visible_message_indices();
        if visible_indices.len() < 2 {
            return Ok(false);
        }

        let retain_count = self
            .compaction_config
            .preserve_recent_messages
            .min(visible_indices.len().saturating_sub(1));
        let split_at = visible_indices.len().saturating_sub(retain_count);
        if split_at < 2 {
            return Ok(false);
        }
        let source_indices = visible_indices[..split_at].to_vec();
        let retained_tail_indices = visible_indices[split_at..].to_vec();
        let source_messages = source_indices
            .iter()
            .filter_map(|index| self.session.transcript.get(*index).cloned())
            .collect::<Vec<_>>();
        if source_messages.len() < 2 {
            return Ok(false);
        }

        let pre_hooks = self
            .run_hooks(
                &self.hook_registrations,
                HookContext {
                    event: HookEvent::PreCompact,
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("reason".to_string(), reason.to_string())]
                        .into_iter()
                        .collect(),
                    payload: json!({
                        "reason": reason,
                        "source_message_count": source_messages.len(),
                        "retained_message_count": retained_tail_indices.len(),
                    }),
                },
            )
            .await?;
        if matches!(pre_hooks.gate_decision, Some(GateDecision::Block))
            || !pre_hooks.continue_allowed
        {
            return Ok(false);
        }

        let mut compaction_instructions = instructions;
        if let Some(message) = pre_hooks.system_messages.first() {
            compaction_instructions = Some(match compaction_instructions {
                Some(existing) => format!("{existing}\n\n{message}"),
                None => message.clone(),
            });
        }

        let result = self
            .conversation_compactor
            .compact(CompactionRequest {
                run_id: self.session.run_id.clone(),
                session_id: self.session.session_id.clone(),
                turn_id: turn_id.clone(),
                messages: source_messages.clone(),
                instructions: compaction_instructions,
            })
            .await?;

        let summary_index = self.session.transcript.len();
        let summary_message = Message::system(result.summary.clone());
        let event = append_transcript_message(
            &mut self.session.transcript,
            summary_message,
            self.session.run_id.clone(),
            self.session.session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        // A local compaction rewrites the request window into a new synthetic
        // summary/tail boundary. Any upstream `previous_response_id` chain now
        // refers to a different history shape, so the next provider request
        // must restart from the compacted visible transcript.
        self.reset_provider_continuation();
        self.session.compaction_summary_index = Some(summary_index);
        self.session.retained_tail_indices = retained_tail_indices.clone();
        self.session.post_summary_start = summary_index + 1;

        self.append_event(
            Some(turn_id.clone()),
            None,
            types::RunEventKind::CompactionCompleted {
                reason: reason.to_string(),
                source_message_count: source_messages.len(),
                retained_message_count: retained_tail_indices.len(),
                summary_chars: result.summary.chars().count(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::CompactionCompleted {
            reason: reason.to_string(),
            source_message_count: source_messages.len(),
            retained_message_count: retained_tail_indices.len(),
            summary: result.summary.clone(),
        })?;

        let post_hooks = self
            .run_hooks(
                &self.hook_registrations,
                HookContext {
                    event: HookEvent::PostCompact,
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("reason".to_string(), reason.to_string())]
                        .into_iter()
                        .collect(),
                    payload: json!({
                        "reason": reason,
                        "source_message_count": source_messages.len(),
                        "retained_message_count": retained_tail_indices.len(),
                        "summary": result.summary,
                    }),
                },
            )
            .await?;
        self.append_hook_context_messages(turn_id, &post_hooks)
            .await?;
        Ok(true)
    }
}
