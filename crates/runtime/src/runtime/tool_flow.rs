use super::AgentRuntime;
use crate::{
    LoopSignalSeverity, Result, RuntimeObserver, RuntimeProgressEvent, ToolApprovalOutcome,
    ToolApprovalPolicyDecision, ToolApprovalRequest, append_transcript_message,
};
use serde_json::json;
use std::collections::BTreeMap;
use tracing::warn;
use types::{
    AgentCoreError, HookContext, HookEvent, HookRegistration, Message, PermissionBehavior,
    PermissionDecision, SessionEventKind, ToolCall, ToolSpec, TurnId,
};

impl AgentRuntime {
    pub(super) async fn handle_tool_call(
        &mut self,
        hooks: &[HookRegistration],
        turn_id: &TurnId,
        call: ToolCall,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        let mut call = call;
        let tool_name = call.tool_name.clone();
        let tool = self
            .tool_registry
            .get(tool_name.as_str())
            .ok_or_else(|| AgentCoreError::Tool(format!("tool not found: {tool_name}")))?;
        let tool_spec = tool.spec();
        let mut fields = BTreeMap::from([("tool_name".to_string(), tool_name.to_string())]);
        if let types::ToolOrigin::Mcp { server_name } = &call.origin {
            fields.insert("mcp_server_name".to_string(), server_name.to_string());
        }

        let pre_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::PreToolUse,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: fields.clone(),
                    payload: json!({ "tool_call": call.clone() }),
                },
            )
            .await?;
        let pre_effects = self
            .apply_hook_effects(turn_id, pre_hooks, None, Some(&tool_name))
            .await?;
        if let Some(arguments) = pre_effects.rewritten_tool_arguments.clone() {
            call.arguments = arguments;
        }

        let mut approval_reasons = approval_reasons_for_tool(&tool_spec);
        let mut hook_approval_reasons = Vec::new();
        let mut policy_only_reasons = Vec::new();
        if let Some(reason) = pre_effects.blocked_reason("tool use blocked") {
            return self
                .record_tool_failure_result(hooks, turn_id, &call, observer, reason)
                .await;
        }
        match pre_effects
            .permission_decision
            .unwrap_or(PermissionDecision::Allow)
        {
            PermissionDecision::Deny => {
                let error = AgentCoreError::ToolDenied(tool_name.to_string()).to_string();
                return self
                    .record_tool_failure_result(hooks, turn_id, &call, observer, error)
                    .await;
            }
            PermissionDecision::Ask => {
                let permission_hooks = self
                    .run_hooks(
                        hooks,
                        HookContext {
                            event: HookEvent::PermissionRequest,
                            session_id: self.session.session_id.clone(),
                            agent_session_id: self.session.agent_session_id.clone(),
                            turn_id: Some(turn_id.clone()),
                            fields,
                            payload: json!({ "tool_call": call.clone() }),
                        },
                    )
                    .await?;
                let permission_effects = self
                    .apply_hook_effects(turn_id, permission_hooks, None, Some(&tool_name))
                    .await?;
                if matches!(
                    permission_effects.permission_behavior,
                    Some(PermissionBehavior::Deny)
                ) || permission_effects
                    .blocked_reason("permission request denied")
                    .is_some()
                {
                    let error = AgentCoreError::PermissionDenied(tool_name.to_string()).to_string();
                    return self
                        .record_tool_failure_result(
                            hooks,
                            turn_id,
                            &call,
                            observer,
                            permission_effects
                                .gate_reason
                                .or(permission_effects.stop_reason)
                                .unwrap_or(error),
                        )
                        .await;
                }
                hook_approval_reasons.push(
                    permission_effects
                        .gate_reason
                        .unwrap_or_else(|| "hook requested approval".to_string()),
                );
            }
            PermissionDecision::Allow => {}
        }

        let policy_request = ToolApprovalRequest {
            call: call.clone(),
            spec: tool_spec.clone(),
            reasons: approval_reasons
                .iter()
                .chain(hook_approval_reasons.iter())
                .cloned()
                .collect(),
        };
        match self.tool_approval_policy.decide(&policy_request) {
            ToolApprovalPolicyDecision::Allow => {
                // Runtime-native allow rules can suppress baseline hint-driven
                // approval, but they intentionally do not erase hook-requested
                // review because hooks may encode higher-order host policy.
                approval_reasons.clear();
            }
            ToolApprovalPolicyDecision::Ask { reason } => {
                policy_only_reasons
                    .push(reason.unwrap_or_else(|| "approval policy requested review".to_string()));
            }
            ToolApprovalPolicyDecision::Deny { reason } => {
                let error = reason.unwrap_or_else(|| {
                    AgentCoreError::PermissionDenied(tool_name.to_string()).to_string()
                });
                return self
                    .record_tool_failure_result(hooks, turn_id, &call, observer, error)
                    .await;
            }
            ToolApprovalPolicyDecision::Abstain => {}
        }
        approval_reasons.extend(hook_approval_reasons);
        approval_reasons.extend(policy_only_reasons);

        if !approval_reasons.is_empty() {
            self.append_event(
                Some(turn_id.clone()),
                Some(call.id.clone()),
                SessionEventKind::ToolApprovalRequested {
                    call: call.clone(),
                    reasons: approval_reasons.clone(),
                },
            )
            .await?;
            observer.on_event(RuntimeProgressEvent::ToolApprovalRequested {
                call: call.clone(),
                reasons: approval_reasons.clone(),
            })?;
            let outcome = self
                .tool_approval_handler
                .decide(ToolApprovalRequest {
                    call: call.clone(),
                    spec: tool_spec,
                    reasons: approval_reasons,
                })
                .await?;
            match outcome {
                ToolApprovalOutcome::Approve => {
                    self.append_event(
                        Some(turn_id.clone()),
                        Some(call.id.clone()),
                        SessionEventKind::ToolApprovalResolved {
                            call: call.clone(),
                            approved: true,
                            reason: None,
                        },
                    )
                    .await?;
                    observer.on_event(RuntimeProgressEvent::ToolApprovalResolved {
                        call: call.clone(),
                        approved: true,
                        reason: None,
                    })?;
                }
                ToolApprovalOutcome::Deny { reason } => {
                    self.append_event(
                        Some(turn_id.clone()),
                        Some(call.id.clone()),
                        SessionEventKind::ToolApprovalResolved {
                            call: call.clone(),
                            approved: false,
                            reason: reason.clone(),
                        },
                    )
                    .await?;
                    observer.on_event(RuntimeProgressEvent::ToolApprovalResolved {
                        call: call.clone(),
                        approved: false,
                        reason: reason.clone(),
                    })?;
                    return self
                        .record_tool_failure_result(
                            hooks,
                            turn_id,
                            &call,
                            observer,
                            reason.unwrap_or_else(|| {
                                AgentCoreError::PermissionDenied(tool_name.to_string()).to_string()
                            }),
                        )
                        .await;
                }
            }
        }

        let lifecycle_event = self
            .append_tool_lifecycle_event(
                turn_id,
                &call,
                SessionEventKind::ToolCallStarted { call: call.clone() },
            )
            .await?;
        observer.on_event(RuntimeProgressEvent::ToolLifecycle {
            event: lifecycle_event,
        })?;

        if let Some(signal) = self.tool_loop_detector.inspect(&call) {
            let message = format!(
                "loop_detector [{}] {}",
                severity_label(signal.severity),
                signal.reason
            );
            self.append_event(
                Some(turn_id.clone()),
                Some(call.id.clone()),
                SessionEventKind::Notification {
                    source: "loop_detector".to_string(),
                    message: message.clone(),
                },
            )
            .await?;
            if matches!(signal.severity, LoopSignalSeverity::Critical) {
                // Critical loop signals are fed back as tool failures so the model can
                // recover in-band instead of the runtime hard-aborting the whole turn.
                warn!(
                    session_id = %self.session.session_id,
                    turn_id = %turn_id,
                    tool_name = %call.tool_name,
                    reason = %signal.reason,
                    "loop detector blocked tool execution"
                );
                return self
                    .record_tool_failure_result(
                        hooks,
                        turn_id,
                        &call,
                        observer,
                        format!("loop detector blocked tool call: {}", signal.reason),
                    )
                    .await;
            }
        }

        let scoped_tool_context = self.tool_context.with_runtime_scope(
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
            turn_id.clone(),
            tool_name.clone(),
            call.call_id.clone(),
        );

        match tool
            .execute(
                call.id.clone(),
                call.arguments.clone(),
                &scoped_tool_context,
            )
            .await
        {
            Ok(mut result) => {
                self.tool_loop_detector.record_result(&call, &result);
                result.call_id = call.call_id.clone();
                let event = append_transcript_message(
                    &mut self.session.transcript,
                    Message::tool_result(result.clone()),
                    self.session.session_id.clone(),
                    self.session.agent_session_id.clone(),
                    turn_id.clone(),
                );
                self.store.append(event).await?;
                let lifecycle_event = self
                    .append_tool_lifecycle_event(
                        turn_id,
                        &call,
                        SessionEventKind::ToolCallCompleted {
                            call: call.clone(),
                            output: result.clone(),
                        },
                    )
                    .await?;
                observer.on_event(RuntimeProgressEvent::ToolLifecycle {
                    event: lifecycle_event,
                })?;

                let post_hooks = self
                    .run_hooks(
                        hooks,
                        HookContext {
                            event: HookEvent::PostToolUse,
                            session_id: self.session.session_id.clone(),
                            agent_session_id: self.session.agent_session_id.clone(),
                            turn_id: Some(turn_id.clone()),
                            fields: [("tool_name".to_string(), tool_name.to_string())]
                                .into_iter()
                                .collect(),
                            payload: json!({ "result": result }),
                        },
                    )
                    .await?;
                let post_effects = self
                    .apply_hook_effects(turn_id, post_hooks, None, Some(&tool_name))
                    .await?;
                if let Some(reason) = post_effects.blocked_reason("post tool hook blocked") {
                    return Err(AgentCoreError::HookBlocked(reason).into());
                }
            }
            Err(error) => {
                self.tool_loop_detector
                    .record_error(&call, &error.to_string());
                self.record_tool_failure_result(hooks, turn_id, &call, observer, error.to_string())
                    .await?;
            }
        }

        Ok(())
    }

    async fn record_tool_failure_result(
        &mut self,
        hooks: &[HookRegistration],
        turn_id: &TurnId,
        call: &ToolCall,
        observer: &mut dyn RuntimeObserver,
        error: String,
    ) -> Result<()> {
        let result =
            types::ToolResult::error(call.id.clone(), call.tool_name.clone(), error.clone())
                .with_call_id(call.call_id.clone());
        let event = append_transcript_message(
            &mut self.session.transcript,
            Message::tool_result(result.clone()),
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        let lifecycle_event = self
            .append_tool_lifecycle_event(
                turn_id,
                call,
                SessionEventKind::ToolCallFailed {
                    call: call.clone(),
                    error: error.clone(),
                },
            )
            .await?;
        observer.on_event(RuntimeProgressEvent::ToolLifecycle {
            event: lifecycle_event,
        })?;
        let failure_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::PostToolUseFailure,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("tool_name".to_string(), call.tool_name.clone())]
                        .into_iter()
                        .map(|(key, value)| (key, value.to_string()))
                        .collect(),
                    payload: json!({ "error": error }),
                },
            )
            .await?;
        let _ = self
            .apply_hook_effects(turn_id, failure_hooks, None, Some(&call.tool_name))
            .await?;
        Ok(())
    }
}

fn severity_label(severity: LoopSignalSeverity) -> &'static str {
    match severity {
        LoopSignalSeverity::Warning => "warning",
        LoopSignalSeverity::Critical => "critical",
    }
}

fn approval_reasons_for_tool(spec: &ToolSpec) -> Vec<String> {
    let mut reasons = Vec::new();
    if spec.approval.mutates_state {
        reasons.push("tool mutates workspace or persistent state".to_string());
    }
    if spec.approval.needs_network {
        reasons.push("tool performs network access".to_string());
    }
    if spec.approval.open_world {
        reasons.push("tool reaches outside the workspace or touches external systems".to_string());
    }
    if spec.approval.needs_host_escape {
        reasons.push("tool requires host escape or unsandboxed execution".to_string());
    }
    if let Some(message) = &spec.approval.approval_message {
        reasons.push(message.clone());
    }
    reasons
}
