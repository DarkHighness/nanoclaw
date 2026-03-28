use super::AgentRuntime;
use crate::{HookInvocationBatch, Result, RuntimeError};
use types::{
    GateDecision, HookEffect, HookHandlerKind, HookMutationPermission, HookRegistration, Message,
    MessageSelector, PermissionBehavior, PermissionDecision, ToolName, TurnId,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct AppliedHookEffects {
    pub current_message: Option<Message>,
    pub appended_messages: Vec<Message>,
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

        self.pending_additional_context
            .extend(applied.additional_context.iter().cloned());
        self.pending_injected_instructions
            .extend(applied.injected_instructions.iter().cloned());
        self.append_hook_messages(turn_id, &applied.appended_messages)
            .await?;
        Ok(applied)
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
            apply_message_replacement(&mut applied.current_message, selector, message)?;
        }
        HookEffect::PatchMessage { selector, patch } => {
            apply_message_patch(&mut applied.current_message, selector, patch)?;
        }
        HookEffect::RemoveMessage { selector } => {
            apply_message_removal(&mut applied.current_message, selector)?;
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
            if !matches!(handler_kind, HookHandlerKind::Wasm) {
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
    selector: MessageSelector,
    message: Message,
) -> Result<()> {
    if !matches!(selector, MessageSelector::Current) {
        return Err(RuntimeError::hook(
            "only current in-flight message replacement is supported",
        ));
    }
    *current_message = Some(message);
    Ok(())
}

fn apply_message_patch(
    current_message: &mut Option<Message>,
    selector: MessageSelector,
    patch: types::MessagePatch,
) -> Result<()> {
    if !matches!(selector, MessageSelector::Current) {
        return Err(RuntimeError::hook(
            "only current in-flight message patching is supported",
        ));
    }
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

fn apply_message_removal(
    current_message: &mut Option<Message>,
    selector: MessageSelector,
) -> Result<()> {
    if !matches!(selector, MessageSelector::Current) {
        return Err(RuntimeError::hook(
            "only current in-flight message removal is supported",
        ));
    }
    *current_message = None;
    Ok(())
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
