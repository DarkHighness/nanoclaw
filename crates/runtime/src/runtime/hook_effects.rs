use super::AgentRuntime;
use crate::{HookInvocationBatch, Result, RuntimeError};
use types::{
    GateDecision, HookEffect, HookHandlerKind, HookMutationPermission, HookRegistration, Message,
    MessageId, MessageRole, MessageSelector, PermissionBehavior, PermissionDecision, RunEventKind,
    ToolName, TurnId,
};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TranscriptMutationTarget {
    MessageId(MessageId),
    LastOfRole(MessageRole),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TranscriptMutation {
    Replace {
        target: TranscriptMutationTarget,
        message: Message,
    },
    Patch {
        target: TranscriptMutationTarget,
        patch: types::MessagePatch,
    },
    Remove {
        target: TranscriptMutationTarget,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AppliedHookEffects {
    pub current_message: Option<Message>,
    pub appended_messages: Vec<Message>,
    pub transcript_mutations: Vec<TranscriptMutation>,
    pub additional_context: Vec<String>,
    pub injected_instructions: Vec<String>,
    pub permission_decision: Option<PermissionDecision>,
    pub permission_behavior: Option<PermissionBehavior>,
    pub gate_decision: Option<GateDecision>,
    pub gate_reason: Option<String>,
    pub stop_reason: Option<String>,
    pub rewritten_tool_arguments: Option<serde_json::Value>,
}

impl AppliedHookEffects {
    #[must_use]
    pub fn blocked_reason(&self, default_reason: &str) -> Option<String> {
        if matches!(self.gate_decision, Some(GateDecision::Block)) {
            return Some(
                self.gate_reason
                    .clone()
                    .or(self.stop_reason.clone())
                    .unwrap_or_else(|| default_reason.to_string()),
            );
        }
        self.stop_reason.clone()
    }
}

impl AgentRuntime {
    pub(super) fn clear_pending_request_effects(&mut self) {
        self.pending_additional_context.clear();
        self.pending_injected_instructions.clear();
    }

    pub(super) async fn apply_hook_effects(
        &mut self,
        turn_id: &TurnId,
        batch: HookInvocationBatch,
        current_message: Option<Message>,
        current_tool_name: Option<&ToolName>,
    ) -> Result<AppliedHookEffects> {
        let mut applied = AppliedHookEffects {
            current_message,
            ..AppliedHookEffects::default()
        };

        for invocation in batch.invocations {
            apply_hook_invocation(&mut applied, &invocation.registration, current_tool_name)?;
            for effect in invocation.output.effects {
                apply_hook_effect(
                    &mut applied,
                    &invocation.registration,
                    current_tool_name,
                    effect,
                )?;
            }
        }

        self.apply_transcript_mutations(turn_id, &applied.transcript_mutations)
            .await?;
        self.pending_additional_context
            .extend(applied.additional_context.iter().cloned());
        self.pending_injected_instructions
            .extend(applied.injected_instructions.iter().cloned());
        self.append_hook_messages(turn_id, &applied.appended_messages)
            .await?;
        Ok(applied)
    }

    async fn apply_transcript_mutations(
        &mut self,
        turn_id: &TurnId,
        mutations: &[TranscriptMutation],
    ) -> Result<()> {
        for mutation in mutations {
            match mutation.clone() {
                TranscriptMutation::Replace { target, message } => {
                    self.apply_transcript_message_replacement(turn_id, target, message)
                        .await?;
                }
                TranscriptMutation::Patch { target, patch } => {
                    self.apply_transcript_message_patch(turn_id, target, patch)
                        .await?;
                }
                TranscriptMutation::Remove { target } => {
                    self.apply_transcript_message_removal(turn_id, target)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn apply_transcript_message_replacement(
        &mut self,
        turn_id: &TurnId,
        target: TranscriptMutationTarget,
        mut message: Message,
    ) -> Result<()> {
        let (index, message_id) = self.resolve_mutable_transcript_target(&target)?;
        // Replacement keeps the original message identity stable so replay and
        // future `message_id` selectors still point at the same transcript node.
        message.message_id = message_id.clone();
        self.session.transcript[index] = message.clone();
        self.session.removed_message_ids.remove(&message_id);
        self.append_event(
            Some(turn_id.clone()),
            None,
            RunEventKind::TranscriptMessagePatched {
                message_id,
                message,
            },
        )
        .await?;
        self.reset_provider_continuation();
        Ok(())
    }

    async fn apply_transcript_message_patch(
        &mut self,
        turn_id: &TurnId,
        target: TranscriptMutationTarget,
        patch: types::MessagePatch,
    ) -> Result<()> {
        let (index, message_id) = self.resolve_mutable_transcript_target(&target)?;
        let Some(message) = self.session.transcript.get_mut(index) else {
            return Err(RuntimeError::invalid_state(format!(
                "transcript index {index} vanished during patch"
            )));
        };
        if let Some(role) = patch.role {
            message.role = role;
        }
        if let Some(parts) = patch.replace_parts {
            message.parts = parts;
        }
        if !patch.append_parts.is_empty() {
            message.parts.extend(patch.append_parts);
        }
        let patched_message = message.clone();
        self.append_event(
            Some(turn_id.clone()),
            None,
            RunEventKind::TranscriptMessagePatched {
                message_id: message_id.clone(),
                message: patched_message,
            },
        )
        .await?;
        self.reset_provider_continuation();
        Ok(())
    }

    async fn apply_transcript_message_removal(
        &mut self,
        turn_id: &TurnId,
        target: TranscriptMutationTarget,
    ) -> Result<()> {
        let (_index, message_id) = self.resolve_mutable_transcript_target(&target)?;
        self.session.removed_message_ids.insert(message_id.clone());
        self.append_event(
            Some(turn_id.clone()),
            None,
            RunEventKind::TranscriptMessageRemoved { message_id },
        )
        .await?;
        self.reset_provider_continuation();
        Ok(())
    }

    fn resolve_mutable_transcript_target(
        &self,
        target: &TranscriptMutationTarget,
    ) -> Result<(usize, MessageId)> {
        match target {
            TranscriptMutationTarget::MessageId(message_id) => {
                if let Some(index) = self.visible_transcript_index_for_message_id(message_id) {
                    return Ok((index, message_id.clone()));
                }
                let error = if self.transcript_contains_message_id(message_id) {
                    format!(
                        "hook cannot mutate compacted or otherwise hidden transcript message `{message_id}`"
                    )
                } else {
                    format!("unknown transcript message id `{message_id}`")
                };
                Err(RuntimeError::hook(error))
            }
            // Role-based selectors resolve at mutation-apply time so multiple
            // queued effects in one hook batch see earlier transcript changes.
            TranscriptMutationTarget::LastOfRole(role) => {
                if let Some(index) = self.visible_transcript_last_index_for_role(role) {
                    let message_id = self
                        .session
                        .transcript
                        .get(index)
                        .map(|message| message.message_id.clone())
                        .ok_or_else(|| {
                            RuntimeError::invalid_state(format!(
                                "visible transcript index {index} vanished during role lookup"
                            ))
                        })?;
                    return Ok((index, message_id));
                }
                let role_name = describe_role(role);
                let error = if self.transcript_contains_role(role) {
                    format!(
                        "hook cannot mutate compacted or otherwise hidden last `{role_name}` transcript message"
                    )
                } else {
                    format!("hook cannot find any `{role_name}` transcript message")
                };
                Err(RuntimeError::hook(error))
            }
        }
    }
}

fn apply_hook_invocation(
    _applied: &mut AppliedHookEffects,
    _registration: &HookRegistration,
    _current_tool_name: Option<&ToolName>,
) -> Result<()> {
    Ok(())
}

fn apply_hook_effect(
    applied: &mut AppliedHookEffects,
    registration: &HookRegistration,
    current_tool_name: Option<&ToolName>,
    effect: HookEffect,
) -> Result<()> {
    validate_effect(registration, &effect)?;
    match effect {
        HookEffect::AppendMessage { role, parts } => {
            applied.appended_messages.push(Message::new(role, parts));
        }
        HookEffect::ReplaceMessage { selector, message } => {
            apply_message_replacement(
                &mut applied.current_message,
                &mut applied.transcript_mutations,
                selector,
                message,
            )?;
        }
        HookEffect::PatchMessage { selector, patch } => {
            apply_message_patch(
                &mut applied.current_message,
                &mut applied.transcript_mutations,
                selector,
                patch,
            )?;
        }
        HookEffect::RemoveMessage { selector } => {
            apply_message_removal(
                &mut applied.current_message,
                &mut applied.transcript_mutations,
                selector,
            )?;
        }
        HookEffect::AddContext { text } => {
            applied.additional_context.push(text);
        }
        HookEffect::SetPermissionDecision { decision, reason } => {
            applied.permission_decision = Some(merge_permission_decision(
                applied.permission_decision,
                decision,
            ));
            if applied.gate_reason.is_none() {
                applied.gate_reason = reason;
            }
        }
        HookEffect::SetPermissionBehavior { behavior, reason } => {
            applied.permission_behavior = Some(merge_permission_behavior(
                applied.permission_behavior,
                behavior,
            ));
            if applied.gate_reason.is_none() {
                applied.gate_reason = reason;
            }
        }
        HookEffect::SetGateDecision { decision, reason } => {
            applied.gate_decision = Some(merge_gate_decision(applied.gate_decision, decision));
            if applied.gate_reason.is_none() {
                applied.gate_reason = reason;
            }
        }
        HookEffect::Elicitation { .. } => {}
        HookEffect::RewriteToolArgs {
            tool_name,
            arguments,
        } => {
            let Some(current_tool_name) = current_tool_name else {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` attempted to rewrite tool args outside tool execution",
                    registration.name
                )));
            };
            if &tool_name == current_tool_name {
                applied.rewritten_tool_arguments = Some(arguments);
            }
        }
        HookEffect::InjectInstruction { text } => {
            applied.injected_instructions.push(text);
        }
        HookEffect::Stop { reason } => {
            if applied.stop_reason.is_none() {
                applied.stop_reason = Some(reason);
            }
        }
    }
    Ok(())
}

fn validate_effect(registration: &HookRegistration, effect: &HookEffect) -> Result<()> {
    let handler_kind = registration.handler.kind();
    let host_defined = registration.execution.is_none();
    let effect_policy = registration
        .execution
        .as_ref()
        .map(|execution| execution.effects.clone());

    match effect {
        HookEffect::AppendMessage { .. } => {
            if !host_defined {
                ensure_message_mutation_allowed(registration, effect_policy.as_ref())?;
            }
        }
        HookEffect::ReplaceMessage { .. }
        | HookEffect::PatchMessage { .. }
        | HookEffect::RemoveMessage { .. } => {
            // Transcript mutation is still reserved for executable/plugin hooks
            // that flow through the explicit effect-permission model. Host-owned
            // hooks are trusted runtime wiring and may use the same effect path
            // without pretending to be WASM modules.
            if !host_defined && !matches!(handler_kind, HookHandlerKind::Wasm) {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` uses message mutation reserved for wasm hooks",
                    registration.name
                )));
            }
            if !host_defined {
                ensure_message_mutation_allowed(registration, effect_policy.as_ref())?;
            }
        }
        HookEffect::AddContext { .. } => {
            if !host_defined
                && !effect_policy
                    .as_ref()
                    .is_some_and(|policy| policy.allow_context_injection)
            {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` is not allowed to inject additional context",
                    registration.name
                )));
            }
        }
        HookEffect::InjectInstruction { .. } => {
            if !host_defined
                && !effect_policy
                    .as_ref()
                    .is_some_and(|policy| policy.allow_instruction_injection)
            {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` is not allowed to inject instructions",
                    registration.name
                )));
            }
        }
        HookEffect::RewriteToolArgs { .. } => {
            if !host_defined
                && !effect_policy
                    .as_ref()
                    .is_some_and(|policy| policy.allow_tool_arg_rewrite)
            {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` is not allowed to rewrite tool args",
                    registration.name
                )));
            }
        }
        HookEffect::SetPermissionDecision { .. } | HookEffect::SetPermissionBehavior { .. } => {
            if !host_defined
                && !effect_policy
                    .as_ref()
                    .is_some_and(|policy| policy.allow_permission_decision)
            {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` is not allowed to influence permission decisions",
                    registration.name
                )));
            }
        }
        HookEffect::SetGateDecision { .. } | HookEffect::Stop { .. } => {
            if !host_defined
                && !effect_policy
                    .as_ref()
                    .is_some_and(|policy| policy.allow_gate_decision)
            {
                return Err(RuntimeError::hook(format!(
                    "hook `{}` is not allowed to gate execution",
                    registration.name
                )));
            }
        }
        HookEffect::Elicitation { .. } => {}
    }
    Ok(())
}

fn ensure_message_mutation_allowed(
    registration: &HookRegistration,
    effect_policy: Option<&types::HookEffectPolicy>,
) -> Result<()> {
    match effect_policy
        .map(|policy| policy.message_mutation)
        .unwrap_or(HookMutationPermission::Deny)
    {
        HookMutationPermission::Allow => Ok(()),
        HookMutationPermission::ReviewRequired => Err(RuntimeError::hook(format!(
            "hook `{}` requires host review for message mutation",
            registration.name
        ))),
        HookMutationPermission::Deny => Err(RuntimeError::hook(format!(
            "hook `{}` is not granted message mutation permission",
            registration.name
        ))),
    }
}

fn apply_message_replacement(
    current_message: &mut Option<Message>,
    transcript_mutations: &mut Vec<TranscriptMutation>,
    selector: MessageSelector,
    message: Message,
) -> Result<()> {
    match selector {
        MessageSelector::Current => {
            *current_message = Some(message);
            Ok(())
        }
        MessageSelector::MessageId { message_id } => {
            transcript_mutations.push(TranscriptMutation::Replace {
                target: TranscriptMutationTarget::MessageId(message_id),
                message,
            });
            Ok(())
        }
        MessageSelector::LastOfRole { role } => {
            transcript_mutations.push(TranscriptMutation::Replace {
                target: TranscriptMutationTarget::LastOfRole(role),
                message,
            });
            Ok(())
        }
    }
}

fn apply_message_patch(
    current_message: &mut Option<Message>,
    transcript_mutations: &mut Vec<TranscriptMutation>,
    selector: MessageSelector,
    patch: types::MessagePatch,
) -> Result<()> {
    match selector {
        MessageSelector::Current => {
            let Some(message) = current_message.as_mut() else {
                return Err(RuntimeError::hook(
                    "hook attempted to patch a missing current message",
                ));
            };
            if let Some(role) = patch.role {
                message.role = role;
            }
            if let Some(parts) = patch.replace_parts {
                message.parts = parts;
            }
            if !patch.append_parts.is_empty() {
                message.parts.extend(patch.append_parts);
            }
            Ok(())
        }
        MessageSelector::MessageId { message_id } => {
            transcript_mutations.push(TranscriptMutation::Patch {
                target: TranscriptMutationTarget::MessageId(message_id),
                patch,
            });
            Ok(())
        }
        MessageSelector::LastOfRole { role } => {
            transcript_mutations.push(TranscriptMutation::Patch {
                target: TranscriptMutationTarget::LastOfRole(role),
                patch,
            });
            Ok(())
        }
    }
}

fn apply_message_removal(
    current_message: &mut Option<Message>,
    transcript_mutations: &mut Vec<TranscriptMutation>,
    selector: MessageSelector,
) -> Result<()> {
    match selector {
        MessageSelector::Current => {
            *current_message = None;
            Ok(())
        }
        MessageSelector::MessageId { message_id } => {
            transcript_mutations.push(TranscriptMutation::Remove {
                target: TranscriptMutationTarget::MessageId(message_id),
            });
            Ok(())
        }
        MessageSelector::LastOfRole { role } => {
            transcript_mutations.push(TranscriptMutation::Remove {
                target: TranscriptMutationTarget::LastOfRole(role),
            });
            Ok(())
        }
    }
}

fn describe_role(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn merge_permission_decision(
    current: Option<PermissionDecision>,
    next: PermissionDecision,
) -> PermissionDecision {
    match (current, next) {
        (Some(PermissionDecision::Deny), _) | (_, PermissionDecision::Deny) => {
            PermissionDecision::Deny
        }
        (Some(PermissionDecision::Ask), _) | (_, PermissionDecision::Ask) => {
            PermissionDecision::Ask
        }
        _ => PermissionDecision::Allow,
    }
}

fn merge_permission_behavior(
    current: Option<PermissionBehavior>,
    next: PermissionBehavior,
) -> PermissionBehavior {
    match (current, next) {
        (Some(PermissionBehavior::Deny), _) | (_, PermissionBehavior::Deny) => {
            PermissionBehavior::Deny
        }
        _ => PermissionBehavior::Allow,
    }
}

fn merge_gate_decision(current: Option<GateDecision>, next: GateDecision) -> GateDecision {
    match (current, next) {
        (Some(GateDecision::Block), _) | (_, GateDecision::Block) => GateDecision::Block,
        _ => GateDecision::Allow,
    }
}
