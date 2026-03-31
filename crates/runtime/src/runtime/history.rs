use super::AgentRuntime;
use crate::{
    CompactionRequest, Result, RuntimeError, RuntimeObserver, RuntimeProgressEvent,
    append_transcript_message, estimate_prompt_tokens,
};
use serde_json::json;
use std::collections::BTreeSet;
use types::{
    HookContext, HookEvent, Message, MessageId, MessagePart, MessageRole, ToolCallId, TurnId,
};

const RETAINED_TAIL_MIN_TOKENS: usize = 10_000;
const RETAINED_TAIL_MIN_TEXT_MESSAGES: usize = 5;

impl AgentRuntime {
    pub(crate) fn visible_message_indices(&self) -> Vec<usize> {
        let indices = if let Some(summary_index) = self.session.compaction_summary_index {
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
        };
        indices
            .into_iter()
            .filter(|index| {
                self.session.transcript.get(*index).is_some_and(|message| {
                    !self
                        .session
                        .removed_message_ids
                        .contains(&message.message_id)
                })
            })
            .collect()
    }

    pub(crate) fn visible_transcript(&self) -> Vec<Message> {
        self.visible_message_indices()
            .into_iter()
            .filter_map(|index| self.session.transcript.get(index).cloned())
            .collect()
    }

    pub(crate) fn visible_transcript_index_for_message_id(
        &self,
        message_id: &MessageId,
    ) -> Option<usize> {
        self.visible_message_indices().into_iter().find(|index| {
            self.session
                .transcript
                .get(*index)
                .is_some_and(|message| &message.message_id == message_id)
        })
    }

    pub(crate) fn visible_transcript_last_index_for_role(
        &self,
        role: &MessageRole,
    ) -> Option<usize> {
        self.visible_message_indices()
            .into_iter()
            .rev()
            .find(|index| {
                self.session
                    .transcript
                    .get(*index)
                    .is_some_and(|message| &message.role == role)
            })
    }

    pub(crate) fn transcript_contains_message_id(&self, message_id: &MessageId) -> bool {
        self.session
            .transcript
            .iter()
            .any(|message| &message.message_id == message_id)
    }

    pub(crate) fn transcript_contains_role(&self, role: &MessageRole) -> bool {
        self.session
            .transcript
            .iter()
            .any(|message| &message.role == role)
    }

    pub(crate) async fn rollback_visible_history_from_message(
        &mut self,
        message_id: &MessageId,
    ) -> Result<crate::RollbackVisibleHistoryOutcome> {
        let visible_indices = self.visible_message_indices();
        let Some(start_at) = visible_indices.iter().position(|index| {
            self.session
                .transcript
                .get(*index)
                .is_some_and(|message| &message.message_id == message_id)
        }) else {
            return Err(RuntimeError::invalid_state(format!(
                "cannot roll back from unknown visible message `{message_id}`"
            )));
        };

        let removed_message_ids = visible_indices[start_at..]
            .iter()
            .filter_map(|index| {
                self.session
                    .transcript
                    .get(*index)
                    .map(|message| message.message_id.clone())
            })
            .collect::<Vec<_>>();
        if removed_message_ids.is_empty() {
            return Ok(crate::RollbackVisibleHistoryOutcome {
                removed_message_ids,
            });
        }

        let turn_id = TurnId::new();
        for removed_message_id in &removed_message_ids {
            // History rollback keeps earlier transcript nodes stable and
            // persists removals as append-only events so replayed sessions
            // reconstruct the same visible history after resume.
            self.session
                .removed_message_ids
                .insert(removed_message_id.clone());
            self.append_event(
                Some(turn_id.clone()),
                None,
                types::SessionEventKind::TranscriptMessageRemoved {
                    message_id: removed_message_id.clone(),
                },
            )
            .await?;
        }
        // Provider-native continuation chains assume append-only growth. Once
        // visible history is truncated, the next request must replay from the
        // surviving transcript boundary instead of the old upstream response id.
        self.reset_provider_continuation();

        Ok(crate::RollbackVisibleHistoryOutcome {
            removed_message_ids,
        })
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

        let estimated_tokens = estimate_prompt_tokens(
            instructions,
            &visible_messages,
            &self.model_visible_tool_specs(),
            &self.pending_additional_context,
        );
        if estimated_tokens < self.compaction_config.trigger_tokens {
            return Ok(false);
        }

        self.compact_visible_history(turn_id, "auto", None, Some(instructions), observer)
            .await
    }

    pub(super) async fn compact_visible_history(
        &mut self,
        turn_id: &TurnId,
        reason: &str,
        instructions: Option<String>,
        post_compaction_instructions: Option<&[String]>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<bool> {
        let visible_indices = self.visible_message_indices();
        if visible_indices.len() < 2 {
            return Ok(false);
        }
        let visible_messages = self.visible_transcript();

        let Some(split_at) = select_compaction_split_index(
            &visible_messages,
            self.compaction_config.preserve_recent_messages,
        ) else {
            return Ok(false);
        };
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
        let compacted_through_message_id = source_messages
            .last()
            .map(|message| message.message_id.clone())
            .expect("compaction source messages must be non-empty");

        let pre_hooks = self
            .run_hooks(
                &self.hook_registrations,
                HookContext {
                    event: HookEvent::PreCompact,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
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
        let pre_effects = self
            .apply_hook_effects_with_observer(turn_id, pre_hooks, None, None, observer)
            .await?;
        if pre_effects.blocked_reason("compaction blocked").is_some() {
            return Ok(false);
        }

        let mut compaction_instructions = instructions;
        if !pre_effects.injected_instructions.is_empty() {
            let injected = pre_effects.injected_instructions.join("\n\n");
            compaction_instructions = Some(match compaction_instructions {
                Some(existing) => format!("{existing}\n\n{injected}"),
                None => injected,
            });
        }

        let result = self
            .conversation_compactor
            .compact(CompactionRequest {
                session_id: self.session.session_id.clone(),
                agent_session_id: self.session.agent_session_id.clone(),
                turn_id: turn_id.clone(),
                messages: source_messages.clone(),
                visible_messages: visible_messages.clone(),
                instructions: compaction_instructions,
            })
            .await?;

        let retained_tail_message_ids = retained_tail_indices
            .iter()
            .filter_map(|index| {
                self.session
                    .transcript
                    .get(*index)
                    .map(|message| message.message_id.clone())
            })
            .collect::<Vec<_>>();
        let summary_index = self.session.transcript.len();
        let summary_message = Message::system(result.summary.clone());
        let summary_message_id = summary_message.message_id.clone();
        let event = append_transcript_message(
            &mut self.session.transcript,
            summary_message,
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
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
            types::SessionEventKind::CompactionCompleted {
                reason: reason.to_string(),
                source_message_count: source_messages.len(),
                retained_message_count: retained_tail_indices.len(),
                summary_chars: result.summary.chars().count(),
                summary_message_id: Some(summary_message_id.clone()),
                retained_tail_message_ids,
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::CompactionCompleted {
            reason: reason.to_string(),
            source_message_count: source_messages.len(),
            retained_message_count: retained_tail_indices.len(),
            summary: result.summary.clone(),
            compacted_through_message_id,
            summary_message_id,
        })?;

        let post_hooks = self
            .run_hooks(
                &self.hook_registrations,
                HookContext {
                    event: HookEvent::PostCompact,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
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
        let _ = self
            .apply_hook_effects_with_observer(turn_id, post_hooks, None, None, observer)
            .await?;

        let hooks = self.hook_registrations.clone();
        // AgentSession boundaries follow history-window boundaries, not user-turn
        // boundaries. Auto compaction can therefore split a single turn: the
        // prompt stays on the pre-compaction AgentSession, while the rebuilt
        // request window and subsequent provider response move to the fresh one.
        self.rotate_agent_session(turn_id, &hooks, "compaction", "compaction", observer)
            .await?;
        if let Some(instructions) = post_compaction_instructions {
            self.record_instruction_load(turn_id, &hooks, instructions, observer)
                .await?;
        }
        Ok(true)
    }
}

fn select_compaction_split_index(
    visible_messages: &[Message],
    preserve_recent_messages: usize,
) -> Option<usize> {
    if visible_messages.len() < 2 {
        return None;
    }
    let retain_count = preserve_recent_messages.min(visible_messages.len().saturating_sub(1));
    let mut split_at = visible_messages.len().saturating_sub(retain_count);
    if split_at < 2 {
        return None;
    }

    split_at = expand_retained_tail_for_context_floor(visible_messages, split_at);
    if split_at == visible_messages.len() {
        return Some(split_at);
    }
    split_at = rewind_split_at_to_turn_cluster_start(visible_messages, split_at)?;
    adjust_split_at_for_tool_pairs(visible_messages, split_at)
}

fn expand_retained_tail_for_context_floor(
    visible_messages: &[Message],
    mut split_at: usize,
) -> usize {
    // Claude-style session-memory compaction preserves a non-trivial tail of
    // recent conversational context. Keep the current message-count floor, but
    // for genuinely large transcripts expand the retained segment until it also
    // carries enough recent text to ground the post-compact continuation.
    if estimate_messages_tokens(visible_messages) < RETAINED_TAIL_MIN_TOKENS {
        return split_at;
    }

    let mut retained_tokens = estimate_messages_tokens(&visible_messages[split_at..]);
    let mut retained_text_messages = count_text_messages(&visible_messages[split_at..]);
    while split_at > 2
        && (retained_tokens < RETAINED_TAIL_MIN_TOKENS
            || retained_text_messages < RETAINED_TAIL_MIN_TEXT_MESSAGES)
    {
        split_at -= 1;
        retained_tokens += estimate_message_tokens(&visible_messages[split_at]);
        retained_text_messages +=
            usize::from(message_has_text_content(&visible_messages[split_at]));
    }
    split_at
}

fn adjust_split_at_for_tool_pairs(
    visible_messages: &[Message],
    mut split_at: usize,
) -> Option<usize> {
    loop {
        let missing_call_ids = missing_tool_call_ids(&visible_messages[split_at..]);
        if missing_call_ids.is_empty() {
            return Some(split_at);
        }
        let previous_call_index =
            find_previous_tool_call_index(visible_messages, split_at, &missing_call_ids)?;
        if previous_call_index < 2 {
            return None;
        }
        split_at = previous_call_index;
    }
}

fn rewind_split_at_to_turn_cluster_start(
    visible_messages: &[Message],
    mut split_at: usize,
) -> Option<usize> {
    // Post-compaction continuity is materially worse when the retained tail
    // starts in the middle of an assistant trajectory. Rewind across
    // assistant/tool messages until the tail starts at the request-side
    // cluster that kicked off the surviving turn. That cluster can include
    // system steer/reminder messages plus synthetic recall/user prefix
    // messages that all belong to the same model request.
    loop {
        match visible_messages.get(split_at)?.role {
            MessageRole::Assistant | MessageRole::Tool => {
                if split_at == 0 {
                    return None;
                }
                split_at -= 1;
            }
            MessageRole::User | MessageRole::System => {
                while split_at > 0
                    && matches!(
                        visible_messages
                            .get(split_at - 1)
                            .map(|message| &message.role),
                        Some(MessageRole::User | MessageRole::System)
                    )
                {
                    split_at -= 1;
                }
                return (split_at >= 2).then_some(split_at);
            }
        }
    }
}

fn missing_tool_call_ids(messages: &[Message]) -> BTreeSet<ToolCallId> {
    let tool_call_ids = messages
        .iter()
        .flat_map(message_tool_call_ids)
        .collect::<BTreeSet<_>>();
    messages
        .iter()
        .flat_map(message_tool_result_ids)
        .filter(|tool_call_id| !tool_call_ids.contains(tool_call_id))
        .collect()
}

fn find_previous_tool_call_index(
    visible_messages: &[Message],
    split_at: usize,
    required_call_ids: &BTreeSet<ToolCallId>,
) -> Option<usize> {
    (0..split_at).rev().find(|index| {
        message_tool_call_ids(&visible_messages[*index])
            .into_iter()
            .any(|tool_call_id| required_call_ids.contains(&tool_call_id))
    })
}

fn message_tool_call_ids(message: &Message) -> Vec<ToolCallId> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::ToolCall { call } => Some(call.id.clone()),
            _ => None,
        })
        .collect()
}

fn message_tool_result_ids(message: &Message) -> Vec<ToolCallId> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::ToolResult { result } => Some(result.id.clone()),
            _ => None,
        })
        .collect()
}

fn estimate_messages_tokens(messages: &[Message]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

fn estimate_message_tokens(message: &Message) -> usize {
    (message.text_content().len() + 32).div_ceil(4)
}

fn count_text_messages(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| message_has_text_content(message))
        .count()
}

fn message_has_text_content(message: &Message) -> bool {
    matches!(message.role, MessageRole::User | MessageRole::Assistant)
        && !message.text_content().trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        RETAINED_TAIL_MIN_TEXT_MESSAGES, RETAINED_TAIL_MIN_TOKENS, count_text_messages,
        estimate_messages_tokens, select_compaction_split_index,
    };
    use serde_json::json;
    use types::{Message, MessagePart, ToolCall, ToolOrigin, ToolResult};

    #[test]
    fn split_index_expands_retained_tail_for_large_contexts() {
        let visible_messages = (0..20)
            .map(|index| {
                if index % 2 == 0 {
                    Message::user(format!("prompt-{index} {}", "x".repeat(3000)))
                } else {
                    Message::assistant(format!("reply-{index} {}", "y".repeat(3000)))
                }
            })
            .collect::<Vec<_>>();

        let split_at = select_compaction_split_index(&visible_messages, 1).expect("split index");
        let retained_tail = &visible_messages[split_at..];

        assert!(estimate_messages_tokens(retained_tail) >= RETAINED_TAIL_MIN_TOKENS);
        assert!(count_text_messages(retained_tail) >= RETAINED_TAIL_MIN_TEXT_MESSAGES);
        assert!(split_at >= 2);
    }

    #[test]
    fn split_index_moves_backward_to_preserve_tool_pairs() {
        let visible_messages = vec![
            Message::user("older prompt"),
            Message::assistant("older answer"),
            Message::user("current prompt"),
            Message::assistant_parts(vec![MessagePart::ToolCall {
                call: ToolCall {
                    id: "tool-call-1".into(),
                    call_id: "call-1".into(),
                    tool_name: "read_file".into(),
                    arguments: json!({ "path": "README.md" }),
                    origin: ToolOrigin::Local,
                },
            }]),
            Message::tool_result(ToolResult::text("tool-call-1".into(), "read_file", "ok")),
            Message::assistant("follow up"),
        ];

        let split_at = select_compaction_split_index(&visible_messages, 2).expect("split index");

        assert_eq!(split_at, 2);
    }

    #[test]
    fn split_index_rewinds_to_start_of_user_cluster() {
        let visible_messages = vec![
            Message::user("older prompt"),
            Message::assistant("older answer"),
            Message::user("recalled workspace memory"),
            Message::user("real user prompt"),
            Message::assistant("latest assistant reply"),
        ];

        let split_at = select_compaction_split_index(&visible_messages, 1).expect("split index");

        assert_eq!(split_at, 2);
    }

    #[test]
    fn split_index_rewinds_to_preserve_request_side_system_messages() {
        let visible_messages = vec![
            Message::user("older prompt"),
            Message::assistant("older answer"),
            Message::system("prefer terse answers"),
            Message::user("recalled workspace memory"),
            Message::user("real user prompt"),
            Message::assistant("latest assistant reply"),
        ];

        let split_at = select_compaction_split_index(&visible_messages, 1).expect("split index");

        assert_eq!(split_at, 2);
    }

    #[test]
    fn split_index_keeps_full_tool_trajectory_turn() {
        let visible_messages = vec![
            Message::user("older prompt"),
            Message::assistant("older answer"),
            Message::user("recalled workspace memory"),
            Message::user("real user prompt"),
            Message::assistant_parts(vec![MessagePart::ToolCall {
                call: ToolCall {
                    id: "tool-call-2".into(),
                    call_id: "call-2".into(),
                    tool_name: "read_file".into(),
                    arguments: json!({ "path": "Cargo.toml" }),
                    origin: ToolOrigin::Local,
                },
            }]),
            Message::tool_result(ToolResult::text("tool-call-2".into(), "read_file", "ok")),
            Message::assistant("latest assistant reply"),
        ];

        let split_at = select_compaction_split_index(&visible_messages, 1).expect("split index");

        assert_eq!(split_at, 2);
    }
}
