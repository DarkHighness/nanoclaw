mod provider_state;

use crate::{
    CompactionConfig, CompactionRequest, ConversationCompactor, HookAggregate, HookRunner,
    LoopDetectionConfig, LoopSignalSeverity, ModelBackend, NoopRuntimeObserver, Result,
    RuntimeCommand, RuntimeObserver, RuntimeProgressEvent, RuntimeSession, ToolApprovalHandler,
    ToolApprovalOutcome, ToolApprovalPolicy, ToolApprovalPolicyDecision, ToolApprovalRequest,
    ToolLoopDetector, append_transcript_message, estimate_prompt_tokens,
};
use futures::StreamExt;
use provider_state::is_provider_continuation_lost;
use serde_json::json;
use skills::SkillCatalog;
use std::collections::BTreeMap;
use std::sync::Arc;
use store::RunStore;
use tools::{ToolExecutionContext, ToolRegistry};
use tracing::{debug, info, warn};
use types::{
    AgentCoreError, GateDecision, HookContext, HookEvent, HookRegistration, Message, MessageId,
    MessagePart, ModelEvent, PermissionBehavior, PermissionDecision, RunEventEnvelope,
    RunEventKind, ToolCall, ToolLifecycleEventEnvelope, ToolSpec, TurnId,
};

pub struct AgentRuntime {
    backend: Arc<dyn ModelBackend>,
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn RunStore>,
    tool_registry: ToolRegistry,
    tool_context: ToolExecutionContext,
    tool_approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
    conversation_compactor: Arc<dyn ConversationCompactor>,
    compaction_config: CompactionConfig,
    tool_loop_detector: ToolLoopDetector,
    base_instructions: Vec<String>,
    hook_registrations: Vec<HookRegistration>,
    session: RuntimeSession,
}

pub struct RunTurnOutcome {
    pub turn_id: TurnId,
    pub assistant_text: String,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        backend: Arc<dyn ModelBackend>,
        hook_runner: Arc<HookRunner>,
        store: Arc<dyn RunStore>,
        tool_registry: ToolRegistry,
        tool_context: ToolExecutionContext,
        tool_approval_handler: Arc<dyn ToolApprovalHandler>,
        tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
        conversation_compactor: Arc<dyn ConversationCompactor>,
        compaction_config: CompactionConfig,
        loop_detection_config: LoopDetectionConfig,
        base_instructions: Vec<String>,
        hook_registrations: Vec<HookRegistration>,
        _skill_catalog: SkillCatalog,
    ) -> Self {
        Self {
            backend,
            hook_runner,
            store,
            tool_registry,
            tool_context,
            tool_approval_handler,
            tool_approval_policy,
            conversation_compactor,
            compaction_config,
            tool_loop_detector: ToolLoopDetector::new(loop_detection_config),
            base_instructions,
            hook_registrations,
            session: RuntimeSession::default(),
        }
    }

    #[must_use]
    pub fn run_id(&self) -> types::RunId {
        self.session.run_id.clone()
    }

    #[must_use]
    pub fn session_id(&self) -> types::SessionId {
        self.session.session_id.clone()
    }

    #[must_use]
    pub fn tool_registry_names(&self) -> Vec<String> {
        self.tool_registry
            .names()
            .into_iter()
            .map(|name| name.to_string())
            .collect()
    }

    #[must_use]
    pub fn tool_specs(&self) -> Vec<types::ToolSpec> {
        self.tool_registry.specs()
    }

    pub async fn end_session(&mut self, reason: Option<String>) -> Result<()> {
        self.append_event(None, None, RunEventKind::SessionEnd { reason })
            .await
    }

    pub async fn run_user_prompt(&mut self, prompt: impl Into<String>) -> Result<RunTurnOutcome> {
        let mut observer = NoopRuntimeObserver;
        self.run_user_prompt_with_observer(prompt, &mut observer)
            .await
    }

    pub async fn compact_now(&mut self, instructions: Option<String>) -> Result<bool> {
        let mut observer = NoopRuntimeObserver;
        self.compact_visible_history(&TurnId::new(), "manual", instructions, &mut observer)
            .await
    }

    pub async fn steer(
        &mut self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> Result<()> {
        let mut observer = NoopRuntimeObserver;
        self.steer_with_observer(message, reason, &mut observer)
            .await
    }

    pub async fn steer_with_observer(
        &mut self,
        message: impl Into<String>,
        reason: Option<String>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        let message = message.into();
        let turn_id = TurnId::new();
        // Steering is append-only runtime control: it becomes a new system message
        // in the transcript instead of mutating the fixed preamble or prior history.
        let event = append_transcript_message(
            &mut self.session.transcript,
            Message::system(message.clone()),
            self.session.run_id.clone(),
            self.session.session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        self.append_event(
            Some(turn_id),
            None,
            RunEventKind::SteerApplied {
                message: message.clone(),
                reason: reason.clone(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::SteerApplied { message, reason })?;
        Ok(())
    }

    pub async fn apply_control(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<Option<RunTurnOutcome>> {
        let mut observer = NoopRuntimeObserver;
        self.apply_control_with_observer(command, &mut observer)
            .await
    }

    pub async fn apply_control_with_observer(
        &mut self,
        command: RuntimeCommand,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<Option<RunTurnOutcome>> {
        match command {
            RuntimeCommand::Prompt { prompt } => self
                .run_user_prompt_with_observer(prompt, observer)
                .await
                .map(Some),
            RuntimeCommand::Steer { message, reason } => {
                self.steer_with_observer(message, reason, observer).await?;
                Ok(None)
            }
        }
    }

    pub async fn run_user_prompt_with_observer(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<RunTurnOutcome> {
        let prompt = prompt.into();
        let turn_id = TurnId::new();
        let hooks = self.hook_registrations.clone();
        let instructions = self.base_instructions.clone();
        info!(
            run_id = %self.session.run_id,
            session_id = %self.session.session_id,
            turn_id = %turn_id,
            prompt_chars = prompt.chars().count(),
            "starting user turn"
        );

        if !self.session.session_started {
            let session_start_hooks = self
                .run_hooks(
                    &hooks,
                    HookContext {
                        event: HookEvent::SessionStart,
                        run_id: self.session.run_id.clone(),
                        session_id: self.session.session_id.clone(),
                        turn_id: None,
                        fields: [("reason".to_string(), "new_session".to_string())]
                            .into_iter()
                            .collect(),
                        payload: json!({"reason":"new_session"}),
                    },
                )
                .await?;
            self.append_hook_context_messages(&turn_id, &session_start_hooks)
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
        }

        if !instructions.is_empty() {
            let instruction_hooks = self
                .run_hooks(
                    &hooks,
                    HookContext {
                        event: HookEvent::InstructionsLoaded,
                        run_id: self.session.run_id.clone(),
                        session_id: self.session.session_id.clone(),
                        turn_id: Some(turn_id.clone()),
                        fields: [("reason".to_string(), "runtime_instructions".to_string())]
                            .into_iter()
                            .collect(),
                        payload: json!({"count": instructions.len()}),
                    },
                )
                .await?;
            self.append_hook_context_messages(&turn_id, &instruction_hooks)
                .await?;
            self.append_event(
                Some(turn_id.clone()),
                None,
                RunEventKind::InstructionsLoaded {
                    count: instructions.len(),
                },
            )
            .await?;
        }

        let async_context = self.hook_runner.drain_async_context().await;
        self.append_hook_context_messages(&turn_id, &async_context)
            .await?;

        let user_hooks = self
            .run_hooks(
                &hooks,
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
        self.append_hook_context_messages(&turn_id, &user_hooks)
            .await?;

        let user_message = Message::user(prompt.clone());
        let transcript_event = append_transcript_message(
            &mut self.session.transcript,
            user_message,
            self.session.run_id.clone(),
            self.session.session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(transcript_event).await?;
        self.append_event(
            Some(turn_id.clone()),
            None,
            RunEventKind::UserPromptSubmit {
                prompt: prompt.clone(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::UserPromptAdded {
            prompt: prompt.clone(),
        })?;

        let mut iteration = 0usize;
        loop {
            iteration = iteration.saturating_add(1);
            let _ = self
                .compact_if_needed(&turn_id, &instructions, observer)
                .await?;
            let mut request = self.build_model_request(&turn_id, &instructions, false);
            self.append_event(
                Some(turn_id.clone()),
                None,
                RunEventKind::ModelRequestStarted {
                    request: request.clone(),
                },
            )
            .await?;
            debug!(
                run_id = %self.session.run_id,
                turn_id = %turn_id,
                iteration,
                uses_provider_continuation = request.continuation.is_some(),
                message_count = request.messages.len(),
                tool_count = request.tools.len(),
                "starting model request"
            );
            observer.on_event(RuntimeProgressEvent::ModelRequestStarted {
                turn_id: turn_id.clone(),
                iteration,
            })?;

            let used_continuation = request.continuation.is_some();
            let mut stream = match self.backend.stream_turn(request.clone()).await {
                Ok(stream) => stream,
                Err(error) if used_continuation && is_provider_continuation_lost(&error) => {
                    warn!(
                        run_id = %self.session.run_id,
                        turn_id = %turn_id,
                        iteration,
                        error = %error,
                        "provider continuation was rejected; retrying with rebuilt transcript"
                    );
                    self.reset_provider_continuation();
                    self.append_event(
                        Some(turn_id.clone()),
                        None,
                        RunEventKind::Notification {
                            source: "provider_state".to_string(),
                            message: error.to_string(),
                        },
                    )
                    .await?;
                    request = self.build_model_request(&turn_id, &instructions, true);
                    self.append_event(
                        Some(turn_id.clone()),
                        None,
                        RunEventKind::ModelRequestStarted {
                            request: request.clone(),
                        },
                    )
                    .await?;
                    observer.on_event(RuntimeProgressEvent::ModelRequestStarted {
                        turn_id: turn_id.clone(),
                        iteration,
                    })?;
                    self.backend.stream_turn(request).await?
                }
                Err(error) => return Err(error),
            };
            let mut assistant_text = String::new();
            let mut tool_calls = Vec::new();
            let mut assistant_reasoning = Vec::new();
            let mut assistant_message_id = None;
            let mut provider_continuation = None;
            while let Some(event) = stream.next().await {
                match event? {
                    ModelEvent::TextDelta { delta } => {
                        assistant_text.push_str(&delta);
                        observer.on_event(RuntimeProgressEvent::AssistantTextDelta { delta })?;
                    }
                    ModelEvent::ToolCallRequested { call } => {
                        tool_calls.push(call.clone());
                        observer.on_event(RuntimeProgressEvent::ToolCallRequested { call })?;
                    }
                    ModelEvent::ResponseComplete {
                        message_id,
                        continuation,
                        reasoning,
                        ..
                    } => {
                        assistant_message_id = Some(message_id.unwrap_or_else(MessageId::new));
                        provider_continuation = continuation;
                        assistant_reasoning = reasoning;
                    }
                    ModelEvent::Error { message } => {
                        return Err(AgentCoreError::ModelBackend(message).into());
                    }
                }
            }

            self.append_event(
                Some(turn_id.clone()),
                None,
                RunEventKind::ModelResponseCompleted {
                    assistant_text: assistant_text.clone(),
                    tool_calls: tool_calls.clone(),
                    continuation: provider_continuation.clone(),
                },
            )
            .await?;
            debug!(
                run_id = %self.session.run_id,
                turn_id = %turn_id,
                iteration,
                assistant_chars = assistant_text.chars().count(),
                tool_call_count = tool_calls.len(),
                "completed model response"
            );
            observer.on_event(RuntimeProgressEvent::ModelResponseCompleted {
                assistant_text: assistant_text.clone(),
                tool_calls: tool_calls.clone(),
            })?;

            if !assistant_text.is_empty()
                || !tool_calls.is_empty()
                || !assistant_reasoning.is_empty()
            {
                let mut parts = Vec::new();
                if !assistant_text.is_empty() {
                    parts.push(MessagePart::text(assistant_text.clone()));
                }
                parts.extend(
                    assistant_reasoning
                        .iter()
                        .cloned()
                        .map(|reasoning| MessagePart::Reasoning { reasoning }),
                );
                parts.extend(
                    tool_calls
                        .iter()
                        .cloned()
                        .map(|call| MessagePart::ToolCall { call }),
                );
                let message = Message::assistant_parts(parts)
                    .with_message_id(assistant_message_id.unwrap_or_else(MessageId::new));
                let event = append_transcript_message(
                    &mut self.session.transcript,
                    message,
                    self.session.run_id.clone(),
                    self.session.session_id.clone(),
                    turn_id.clone(),
                );
                self.store.append(event).await?;
            }
            self.update_provider_continuation(provider_continuation);

            if !tool_calls.is_empty() {
                for call in tool_calls {
                    debug!(
                        run_id = %self.session.run_id,
                        turn_id = %turn_id,
                        tool_name = %call.tool_name,
                        call_id = %call.call_id,
                        "dispatching tool call"
                    );
                    self.handle_tool_call(&hooks, &turn_id, call, observer)
                        .await?;
                }
                let drained = self.hook_runner.drain_async_context().await;
                self.append_hook_context_messages(&turn_id, &drained)
                    .await?;
                continue;
            }

            let stop_hooks = self
                .run_hooks(
                    &hooks,
                    HookContext {
                        event: HookEvent::Stop,
                        run_id: self.session.run_id.clone(),
                        session_id: self.session.session_id.clone(),
                        turn_id: Some(turn_id.clone()),
                        fields: [("reason".to_string(), "assistant_complete".to_string())]
                            .into_iter()
                            .collect(),
                        payload: json!({ "assistant_text": assistant_text }),
                    },
                )
                .await?;

            if matches!(stop_hooks.gate_decision, Some(GateDecision::Block))
                || !stop_hooks.continue_allowed
            {
                let reason = stop_hooks
                    .gate_reason
                    .clone()
                    .or(stop_hooks.stop_reason.clone())
                    .unwrap_or_else(|| "stop blocked".to_string());
                self.append_event(
                    Some(turn_id.clone()),
                    None,
                    RunEventKind::StopFailure {
                        reason: Some(reason.clone()),
                    },
                )
                .await?;
                if stop_hooks.system_messages.is_empty() && stop_hooks.additional_context.is_empty()
                {
                    return Err(AgentCoreError::HookBlocked(reason).into());
                }
                self.append_hook_context_messages(&turn_id, &stop_hooks)
                    .await?;
                continue;
            }

            self.append_event(
                Some(turn_id.clone()),
                None,
                RunEventKind::Stop {
                    reason: Some("assistant_complete".to_string()),
                },
            )
            .await?;
            info!(
                run_id = %self.session.run_id,
                turn_id = %turn_id,
                assistant_chars = assistant_text.chars().count(),
                "completed user turn"
            );
            observer.on_event(RuntimeProgressEvent::TurnCompleted {
                turn_id: turn_id.clone(),
                assistant_text: assistant_text.clone(),
            })?;
            return Ok(RunTurnOutcome {
                turn_id,
                assistant_text,
            });
        }
    }

    async fn handle_tool_call(
        &mut self,
        hooks: &[HookRegistration],
        turn_id: &TurnId,
        call: ToolCall,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        let tool_name = call.tool_name.clone();
        let tool = self
            .tool_registry
            .get(tool_name.as_str())
            .ok_or_else(|| AgentCoreError::Tool(format!("tool not found: {tool_name}")))?;
        let tool_spec = tool.spec();
        let mut fields = BTreeMap::from([("tool_name".to_string(), tool_name.to_string())]);
        if let types::ToolOrigin::Mcp { server_name } = &call.origin {
            fields.insert("mcp_server_name".to_string(), server_name.clone());
        }

        let pre_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::PreToolUse,
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: fields.clone(),
                    payload: json!({ "tool_call": call.clone() }),
                },
            )
            .await?;
        self.append_hook_context_messages(turn_id, &pre_hooks)
            .await?;

        let mut approval_reasons = approval_reasons_for_tool(&tool_spec);
        let mut hook_approval_reasons = Vec::new();
        let mut policy_only_reasons = Vec::new();
        match pre_hooks
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
                            run_id: self.session.run_id.clone(),
                            session_id: self.session.session_id.clone(),
                            turn_id: Some(turn_id.clone()),
                            fields,
                            payload: json!({ "tool_call": call.clone() }),
                        },
                    )
                    .await?;
                self.append_hook_context_messages(turn_id, &permission_hooks)
                    .await?;
                if matches!(
                    permission_hooks.permission_behavior,
                    Some(PermissionBehavior::Deny)
                ) {
                    let error = AgentCoreError::PermissionDenied(tool_name.to_string()).to_string();
                    return self
                        .record_tool_failure_result(
                            hooks,
                            turn_id,
                            &call,
                            observer,
                            permission_hooks.gate_reason.unwrap_or(error),
                        )
                        .await;
                }
                hook_approval_reasons.push(
                    permission_hooks
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
                RunEventKind::ToolApprovalRequested {
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
                        RunEventKind::ToolApprovalResolved {
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
                        RunEventKind::ToolApprovalResolved {
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
                RunEventKind::ToolCallStarted { call: call.clone() },
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
                RunEventKind::Notification {
                    source: "loop_detector".to_string(),
                    message: message.clone(),
                },
            )
            .await?;
            if matches!(signal.severity, LoopSignalSeverity::Critical) {
                // Critical loop signals are fed back as tool failures so the model can
                // recover in-band instead of the runtime hard-aborting the whole turn.
                warn!(
                    run_id = %self.session.run_id,
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
            self.session.run_id.clone(),
            self.session.session_id.clone(),
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
                    self.session.run_id.clone(),
                    self.session.session_id.clone(),
                    turn_id.clone(),
                );
                self.store.append(event).await?;
                let lifecycle_event = self
                    .append_tool_lifecycle_event(
                        turn_id,
                        &call,
                        RunEventKind::ToolCallCompleted {
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
                            run_id: self.session.run_id.clone(),
                            session_id: self.session.session_id.clone(),
                            turn_id: Some(turn_id.clone()),
                            fields: [("tool_name".to_string(), tool_name.to_string())]
                                .into_iter()
                                .collect(),
                            payload: json!({ "result": result }),
                        },
                    )
                    .await?;
                if matches!(post_hooks.gate_decision, Some(GateDecision::Block))
                    || !post_hooks.continue_allowed
                {
                    return Err(AgentCoreError::HookBlocked(
                        post_hooks
                            .gate_reason
                            .or(post_hooks.stop_reason)
                            .unwrap_or_else(|| "post tool hook blocked".to_string()),
                    )
                    .into());
                }
                self.append_hook_context_messages(turn_id, &post_hooks)
                    .await?;
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

    async fn run_hooks(
        &self,
        hooks: &[HookRegistration],
        context: HookContext,
    ) -> Result<HookAggregate> {
        self.hook_runner.run(hooks, context).await
    }

    async fn append_event(
        &self,
        turn_id: Option<TurnId>,
        tool_call_id: Option<types::ToolCallId>,
        event: RunEventKind,
    ) -> Result<()> {
        self.store
            .append(RunEventEnvelope::new(
                self.session.run_id.clone(),
                self.session.session_id.clone(),
                turn_id,
                tool_call_id,
                event,
            ))
            .await?;
        Ok(())
    }

    async fn append_tool_lifecycle_event(
        &self,
        turn_id: &TurnId,
        call: &ToolCall,
        event: RunEventKind,
    ) -> Result<ToolLifecycleEventEnvelope> {
        // Tool lifecycle updates are one of the few events that outer hosts
        // often need both live and durably. Build the canonical RunEventEnvelope
        // once, append it, then project the host-facing typed event from it.
        let envelope = RunEventEnvelope::new(
            self.session.run_id.clone(),
            self.session.session_id.clone(),
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

    async fn append_hook_context_messages(
        &mut self,
        turn_id: &TurnId,
        aggregate: &HookAggregate,
    ) -> Result<()> {
        for message in aggregate
            .system_messages
            .iter()
            .chain(aggregate.additional_context.iter())
        {
            let event = append_transcript_message(
                &mut self.session.transcript,
                Message::system(message.clone()),
                self.session.run_id.clone(),
                self.session.session_id.clone(),
                turn_id.clone(),
            );
            self.store.append(event).await?;
        }
        Ok(())
    }

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

    fn visible_transcript(&self) -> Vec<Message> {
        self.visible_message_indices()
            .into_iter()
            .filter_map(|index| self.session.transcript.get(index).cloned())
            .collect()
    }

    async fn compact_if_needed(
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

    async fn compact_visible_history(
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
            RunEventKind::CompactionCompleted {
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
            self.session.run_id.clone(),
            self.session.session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        let lifecycle_event = self
            .append_tool_lifecycle_event(
                turn_id,
                call,
                RunEventKind::ToolCallFailed {
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
                    run_id: self.session.run_id.clone(),
                    session_id: self.session.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("tool_name".to_string(), call.tool_name.clone())]
                        .into_iter()
                        .map(|(key, value)| (key, value.to_string()))
                        .collect(),
                    payload: json!({ "error": error }),
                },
            )
            .await?;
        self.append_hook_context_messages(turn_id, &failure_hooks)
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
    if tool_annotation_bool(spec, "destructiveHint").unwrap_or(true) {
        reasons.push("tool is marked destructive".to_string());
    }
    if tool_annotation_bool(spec, "openWorldHint").unwrap_or(true) {
        reasons.push("tool reaches outside the workspace or touches external systems".to_string());
    }
    reasons
}

fn tool_annotation_bool(spec: &ToolSpec, key: &str) -> Option<bool> {
    spec.annotations
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .or_else(|| {
            spec.annotations
                .get("mcp_annotations")
                .and_then(serde_json::Value::as_object)
                .and_then(|value| value.get(key))
                .and_then(serde_json::Value::as_bool)
        })
}

#[cfg(test)]
mod tests {
    use super::AgentRuntime;
    use crate::{
        AgentRuntimeBuilder, CompactionConfig, CompactionRequest, CompactionResult,
        ConversationCompactor, DefaultCommandHookExecutor, HookRunner, ModelBackend,
        ModelBackendCapabilities, NoopAgentHookEvaluator, ReqwestHttpHookExecutor, Result,
        RuntimeCommand, RuntimeObserver, RuntimeProgressEvent, StringMatcher, ToolApprovalHandler,
        ToolApprovalMatcher, ToolApprovalOutcome, ToolApprovalRequest, ToolApprovalRule,
        ToolApprovalRuleSet, ToolArgumentMatcher,
    };
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use serde_json::Value;
    use skills::{Skill, SkillCatalog};
    use std::collections::{BTreeMap, VecDeque};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use store::{InMemoryRunStore, RunStore};
    use tools::{
        ReadTool, Tool, ToolError, ToolExecutionContext, ToolRegistry, mcp_tool_annotations,
    };
    use types::{
        AgentCoreError, HookContext, HookEvent, HookHandler, HookOutput, HookRegistration, Message,
        ModelEvent, ModelRequest, PromptHookHandler, ProviderContinuation, RunEventKind, ToolCall,
        ToolCallId, ToolLifecycleEventEnvelope, ToolLifecycleEventKind, ToolOrigin, ToolOutputMode,
        ToolResult, ToolSpec,
    };

    struct MockBackend;

    #[derive(Clone, Default)]
    struct RecordingBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl RecordingBackend {
        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[derive(Clone, Default)]
    struct ContinuingBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        fail_first_continuation: Arc<Mutex<bool>>,
    }

    impl ContinuingBackend {
        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn with_failed_continuation() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                fail_first_continuation: Arc::new(Mutex::new(true)),
            }
        }
    }

    struct StaticPromptEvaluator;

    struct StaticCompactor;

    #[async_trait]
    impl crate::PromptHookEvaluator for StaticPromptEvaluator {
        async fn evaluate(&self, _prompt: &str, _context: HookContext) -> Result<HookOutput> {
            Ok(HookOutput {
                system_message: Some("hook system message".to_string()),
                additional_context: vec!["hook additional context".to_string()],
                ..HookOutput::default()
            })
        }
    }

    #[async_trait]
    impl ConversationCompactor for StaticCompactor {
        async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult> {
            Ok(CompactionResult {
                summary: format!("summary for {} messages", request.messages.len()),
            })
        }
    }

    #[derive(Clone, Debug, Default)]
    struct FailingTool;

    #[async_trait]
    impl Tool for FailingTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "fail".into(),
                description: "Always fails".to_string(),
                input_schema: serde_json::json!({"type":"object","properties":{}}),
                output_mode: ToolOutputMode::Text,
                output_schema: None,
                origin: ToolOrigin::Local,
                annotations: Default::default(),
            }
        }

        async fn execute(
            &self,
            _call_id: ToolCallId,
            _arguments: Value,
            _ctx: &ToolExecutionContext,
        ) -> std::result::Result<ToolResult, ToolError> {
            Err(ToolError::invalid_state("boom"))
        }
    }

    #[derive(Clone, Debug, Default)]
    struct DangerousTool;

    #[async_trait]
    impl Tool for DangerousTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "danger".into(),
                description: "Mutates files".to_string(),
                input_schema: serde_json::json!({"type":"object","properties":{}}),
                output_mode: ToolOutputMode::Text,
                output_schema: None,
                origin: ToolOrigin::Local,
                annotations: mcp_tool_annotations("Dangerous Tool", false, true, true, false),
            }
        }

        async fn execute(
            &self,
            call_id: ToolCallId,
            _arguments: Value,
            _ctx: &ToolExecutionContext,
        ) -> std::result::Result<ToolResult, ToolError> {
            Ok(ToolResult::text(call_id, "danger", "mutated"))
        }
    }

    #[derive(Default)]
    struct MockApprovalHandler {
        requests: Mutex<Vec<ToolApprovalRequest>>,
        outcomes: Mutex<VecDeque<ToolApprovalOutcome>>,
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Vec<RuntimeProgressEvent>,
    }

    impl RuntimeObserver for RecordingObserver {
        fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()> {
            self.events.push(event);
            Ok(())
        }
    }

    impl MockApprovalHandler {
        fn with_outcomes(outcomes: impl IntoIterator<Item = ToolApprovalOutcome>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
            }
        }

        fn requests(&self) -> Vec<ToolApprovalRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ToolApprovalHandler for MockApprovalHandler {
        async fn decide(&self, request: ToolApprovalRequest) -> Result<ToolApprovalOutcome> {
            self.requests.lock().unwrap().push(request);
            Ok(self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ToolApprovalOutcome::Approve))
        }
    }

    #[async_trait]
    impl ModelBackend for MockBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            let user_text = request
                .messages
                .last()
                .map(Message::text_content)
                .unwrap_or_default();
            if user_text.contains("tool")
                && !request.messages.iter().any(|message| {
                    message
                        .parts
                        .iter()
                        .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
                })
            {
                let call = ToolCall {
                    id: ToolCallId::new(),
                    call_id: "call-read-1".into(),
                    tool_name: "read".into(),
                    arguments: serde_json::json!({"path":"sample.txt","line_count":1}),
                    origin: ToolOrigin::Local,
                };
                Ok(stream::iter(vec![
                    Ok(ModelEvent::ToolCallRequested { call }),
                    Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some("tool_use".to_string()),
                        message_id: None,
                        continuation: None,
                        reasoning: Vec::new(),
                    }),
                ])
                .boxed())
            } else {
                Ok(stream::iter(vec![
                    Ok(ModelEvent::TextDelta {
                        delta: "done".to_string(),
                    }),
                    Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some("stop".to_string()),
                        message_id: None,
                        continuation: None,
                        reasoning: Vec::new(),
                    }),
                ])
                .boxed())
            }
        }
    }

    #[async_trait]
    impl ModelBackend for RecordingBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request);
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "ok".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[async_trait]
    impl ModelBackend for ContinuingBackend {
        fn capabilities(&self) -> ModelBackendCapabilities {
            ModelBackendCapabilities {
                provider_managed_history: true,
                provider_native_compaction: true,
            }
        }

        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request.clone());
            if request.continuation.is_some() {
                let mut fail_first = self.fail_first_continuation.lock().unwrap();
                if *fail_first {
                    *fail_first = false;
                    return Err(AgentCoreError::ProviderContinuationLost(
                        "provider lost previous_response_id".to_string(),
                    )
                    .into());
                }
            }

            let response_index = self.requests.lock().unwrap().len();
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: format!("response {response_index}"),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: Some(format!("msg_{response_index}").into()),
                    continuation: Some(ProviderContinuation::OpenAiResponses {
                        response_id: format!("resp_{response_index}").into(),
                    }),
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[tokio::test]
    async fn runtime_handles_tool_loop() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
            .await
            .unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(ReadTool::new());
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(Arc::new(MockBackend), store)
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_registry(registry)
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();

        let outcome = runtime.run_user_prompt("please use tool").await.unwrap();
        assert_eq!(outcome.assistant_text, "done");
    }

    #[tokio::test]
    async fn observer_tool_lifecycle_events_share_store_event_ids() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
            .await
            .unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(ReadTool::new());
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime =
            AgentRuntimeBuilder::new(Arc::new(MockBackend), store.clone())
                .hook_runner(Arc::new(HookRunner::default()))
                .tool_registry(registry)
                .tool_context(ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    model_context_window_tokens: Some(128_000),
                    ..Default::default()
                })
                .skill_catalog(SkillCatalog::default())
                .build();
        let mut observer = RecordingObserver::default();

        let outcome = runtime
            .run_user_prompt_with_observer("please use tool", &mut observer)
            .await
            .unwrap();
        assert_eq!(outcome.assistant_text, "done");

        let observed_lifecycle = observer
            .events
            .iter()
            .filter_map(|event| match event {
                RuntimeProgressEvent::ToolLifecycle { event } => Some(event.clone()),
                _ => None,
            })
            .collect::<Vec<ToolLifecycleEventEnvelope>>();
        assert_eq!(observed_lifecycle.len(), 2);
        assert!(matches!(
            observed_lifecycle[0].event,
            ToolLifecycleEventKind::Started { .. }
        ));
        assert!(matches!(
            observed_lifecycle[1].event,
            ToolLifecycleEventKind::Completed { .. }
        ));

        let stored_lifecycle = store
            .events(&runtime.run_id())
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| event.tool_lifecycle_event())
            .collect::<Vec<_>>();
        assert_eq!(
            observed_lifecycle
                .iter()
                .map(|event| event.id.clone())
                .collect::<Vec<_>>(),
            stored_lifecycle
                .iter()
                .map(|event| event.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            observed_lifecycle
                .iter()
                .map(|event| event.tool_call_id.clone())
                .collect::<Vec<_>>(),
            stored_lifecycle
                .iter()
                .map(|event| event.tool_call_id.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn runtime_uses_provider_continuation_for_follow_up_turns() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(ContinuingBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store)
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();

        runtime.run_user_prompt("first task").await.unwrap();
        runtime.run_user_prompt("second task").await.unwrap();

        let requests = backend.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].continuation.is_none());
        assert_eq!(requests[0].messages.len(), 1);
        assert_eq!(requests[0].messages[0].text_content(), "first task");
        assert_eq!(
            requests[1].continuation,
            Some(ProviderContinuation::OpenAiResponses {
                response_id: "resp_1".into(),
            })
        );
        assert_eq!(requests[1].messages.len(), 1);
        assert_eq!(requests[1].messages[0].text_content(), "second task");
    }

    #[tokio::test]
    async fn runtime_retries_full_transcript_when_provider_continuation_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(ContinuingBackend::with_failed_continuation());
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();

        runtime.run_user_prompt("first task").await.unwrap();
        runtime.run_user_prompt("second task").await.unwrap();

        let requests = backend.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[1].continuation,
            Some(ProviderContinuation::OpenAiResponses {
                response_id: "resp_1".into(),
            })
        );
        assert!(requests[2].continuation.is_none());
        assert!(
            requests[2].messages.len() >= 3,
            "fallback request should resend visible transcript"
        );
        let events = store.events(&runtime.run_id()).await.unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::Notification { source, message }
                    if source == "provider_state"
                        && message.contains("provider continuation lost")
            )
        }));
    }

    #[tokio::test]
    async fn local_compaction_resets_provider_continuation() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(ContinuingBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store)
            .hook_runner(Arc::new(HookRunner::default()))
            .conversation_compactor(Arc::new(StaticCompactor))
            .compaction_config(CompactionConfig {
                enabled: true,
                context_window_tokens: 64,
                trigger_tokens: 32,
                preserve_recent_messages: 1,
            })
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();

        runtime.run_user_prompt("first task").await.unwrap();
        runtime
            .steer("keep explanations brief", Some("test".to_string()))
            .await
            .unwrap();
        assert!(runtime.compact_now(None).await.unwrap());
        runtime.run_user_prompt("second task").await.unwrap();

        let requests = backend.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].continuation.is_none());
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.text_content().contains("summary for 2 messages"))
        );
    }

    struct ToolErrorRecoveringBackend;

    #[async_trait]
    impl ModelBackend for ToolErrorRecoveringBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            let has_tool_result = request.messages.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
            });
            if !has_tool_result {
                let call = ToolCall {
                    id: ToolCallId::new(),
                    call_id: "call-fail-1".into(),
                    tool_name: "fail".into(),
                    arguments: serde_json::json!({}),
                    origin: ToolOrigin::Local,
                };
                Ok(stream::iter(vec![
                    Ok(ModelEvent::ToolCallRequested { call }),
                    Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some("tool_use".to_string()),
                        message_id: None,
                        continuation: None,
                        reasoning: Vec::new(),
                    }),
                ])
                .boxed())
            } else {
                Ok(stream::iter(vec![
                    Ok(ModelEvent::TextDelta {
                        delta: "recovered".to_string(),
                    }),
                    Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some("stop".to_string()),
                        message_id: None,
                        continuation: None,
                        reasoning: Vec::new(),
                    }),
                ])
                .boxed())
            }
        }
    }

    #[tokio::test]
    async fn runtime_continues_after_tool_error_result() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(FailingTool);
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime =
            AgentRuntimeBuilder::new(Arc::new(ToolErrorRecoveringBackend), store.clone())
                .hook_runner(Arc::new(HookRunner::default()))
                .tool_registry(registry)
                .tool_context(ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    model_context_window_tokens: Some(128_000),
                    ..Default::default()
                })
                .skill_catalog(SkillCatalog::default())
                .build();

        let outcome = runtime
            .run_user_prompt("please use the failing tool")
            .await
            .unwrap();
        assert_eq!(outcome.assistant_text, "recovered");

        let events = store.events(&runtime.run_id()).await.unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::ToolCallFailed { error, .. } if error.contains("boom")
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::TranscriptMessage { message }
                    if message.parts.iter().any(|part| matches!(
                        part,
                        types::MessagePart::ToolResult { result }
                            if result.is_error && result.text_content().contains("boom")
                ))
            )
        }));
    }

    struct ApprovalRecoveringBackend;

    #[async_trait]
    impl ModelBackend for ApprovalRecoveringBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            let has_tool_result = request.messages.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
            });
            if !has_tool_result {
                let call = ToolCall {
                    id: ToolCallId::new(),
                    call_id: "call-danger-1".into(),
                    tool_name: "danger".into(),
                    arguments: serde_json::json!({"path":"sample.txt"}),
                    origin: ToolOrigin::Local,
                };
                Ok(stream::iter(vec![
                    Ok(ModelEvent::ToolCallRequested { call }),
                    Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some("tool_use".to_string()),
                        message_id: None,
                        continuation: None,
                        reasoning: Vec::new(),
                    }),
                ])
                .boxed())
            } else {
                Ok(stream::iter(vec![
                    Ok(ModelEvent::TextDelta {
                        delta: "approval recovered".to_string(),
                    }),
                    Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some("stop".to_string()),
                        message_id: None,
                        continuation: None,
                        reasoning: Vec::new(),
                    }),
                ])
                .boxed())
            }
        }
    }

    #[tokio::test]
    async fn runtime_continues_after_tool_approval_denied() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(DangerousTool);
        let approval_handler = Arc::new(MockApprovalHandler::with_outcomes([
            ToolApprovalOutcome::Deny {
                reason: Some("user denied dangerous tool".to_string()),
            },
        ]));
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime =
            AgentRuntimeBuilder::new(Arc::new(ApprovalRecoveringBackend), store.clone())
                .hook_runner(Arc::new(HookRunner::default()))
                .tool_registry(registry)
                .tool_context(ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    model_context_window_tokens: Some(128_000),
                    ..Default::default()
                })
                .tool_approval_handler(approval_handler.clone())
                .skill_catalog(SkillCatalog::default())
                .build();

        let outcome = runtime
            .run_user_prompt("please use the dangerous tool")
            .await
            .unwrap();
        assert_eq!(outcome.assistant_text, "approval recovered");

        let requests = approval_handler.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].call.tool_name, types::ToolName::from("danger"));
        assert!(
            requests[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("destructive"))
        );

        let events = store.events(&runtime.run_id()).await.unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::ToolApprovalRequested { call, .. }
                    if call.tool_name == types::ToolName::from("danger")
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::ToolApprovalResolved { call, approved, .. }
                    if call.tool_name == types::ToolName::from("danger") && !approved
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::TranscriptMessage { message }
                    if message.parts.iter().any(|part| matches!(
                        part,
                        types::MessagePart::ToolResult { result }
                            if result.is_error
                                && result.text_content() == "user denied dangerous tool"
                    ))
            )
        }));
    }

    #[tokio::test]
    async fn approval_policy_can_auto_allow_matching_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(DangerousTool);
        let approval_handler = Arc::new(MockApprovalHandler::with_outcomes([
            ToolApprovalOutcome::Deny {
                reason: Some("fallback should not run".to_string()),
            },
        ]));
        let policy = Arc::new(ToolApprovalRuleSet::new(vec![ToolApprovalRule::allow(
            ToolApprovalMatcher {
                tool_names: [types::ToolName::from("danger")].into_iter().collect(),
                origins: vec![crate::ToolOriginMatcher::Local],
                argument_matchers: vec![ToolArgumentMatcher::String {
                    pointer: "/path".to_string(),
                    matcher: StringMatcher::Prefix("sample".to_string()),
                }],
            },
            "allow the sample fixture destructive tool",
        )]));
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime =
            AgentRuntimeBuilder::new(Arc::new(ApprovalRecoveringBackend), store)
                .hook_runner(Arc::new(HookRunner::default()))
                .tool_registry(registry)
                .tool_context(ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    model_context_window_tokens: Some(128_000),
                    ..Default::default()
                })
                .tool_approval_handler(approval_handler.clone())
                .tool_approval_policy(policy)
                .skill_catalog(SkillCatalog::default())
                .build();

        let outcome = runtime
            .run_user_prompt("please use the dangerous tool")
            .await
            .unwrap();

        assert_eq!(outcome.assistant_text, "approval recovered");
        assert!(approval_handler.requests().is_empty());
    }

    #[tokio::test]
    async fn approval_policy_can_require_review_for_otherwise_safe_tools() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
            .await
            .unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(ReadTool::new());
        let approval_handler = Arc::new(MockApprovalHandler::with_outcomes([
            ToolApprovalOutcome::Deny {
                reason: Some("review required for sensitive file".to_string()),
            },
        ]));
        let policy = Arc::new(ToolApprovalRuleSet::new(vec![ToolApprovalRule::ask(
            ToolApprovalMatcher {
                tool_names: [types::ToolName::from("read")].into_iter().collect(),
                origins: vec![crate::ToolOriginMatcher::Local],
                argument_matchers: vec![ToolArgumentMatcher::String {
                    pointer: "/path".to_string(),
                    matcher: StringMatcher::Exact("sample.txt".to_string()),
                }],
            },
            "sensitive file read requires review",
        )]));
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime: AgentRuntime =
            AgentRuntimeBuilder::new(Arc::new(MockBackend), store.clone())
                .hook_runner(Arc::new(HookRunner::default()))
                .tool_registry(registry)
                .tool_context(ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    model_context_window_tokens: Some(128_000),
                    ..Default::default()
                })
                .tool_approval_handler(approval_handler.clone())
                .tool_approval_policy(policy)
                .skill_catalog(SkillCatalog::default())
                .build();

        let outcome = runtime.run_user_prompt("please use tool").await.unwrap();
        assert_eq!(outcome.assistant_text, "done");

        let requests = approval_handler.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("sensitive file read requires review"))
        );
        let events = store.events(&runtime.run_id()).await.unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::ToolApprovalRequested { reasons, .. }
                    if reasons.iter().any(|reason| reason.contains("sensitive file read requires review"))
            )
        }));
    }

    struct StreamingTextBackend;

    #[async_trait]
    impl ModelBackend for StreamingTextBackend {
        async fn stream_turn(
            &self,
            _request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "hel".to_string(),
                }),
                Ok(ModelEvent::TextDelta {
                    delta: "lo".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[tokio::test]
    async fn runtime_notifies_observer_of_streaming_text_progress() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store)
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();
        let mut observer = RecordingObserver::default();

        let outcome = runtime
            .run_user_prompt_with_observer("hello there", &mut observer)
            .await
            .unwrap();

        assert_eq!(outcome.assistant_text, "hello");
        assert!(observer.events.iter().any(|event| matches!(
            event,
            RuntimeProgressEvent::UserPromptAdded { prompt } if prompt == "hello there"
        )));
        assert!(observer.events.iter().any(|event| matches!(
            event,
            RuntimeProgressEvent::AssistantTextDelta { delta } if delta == "hel"
        )));
        assert!(observer.events.iter().any(|event| matches!(
            event,
            RuntimeProgressEvent::AssistantTextDelta { delta } if delta == "lo"
        )));
        assert!(observer.events.iter().any(|event| matches!(
            event,
            RuntimeProgressEvent::TurnCompleted { assistant_text, .. } if assistant_text == "hello"
        )));
    }

    #[tokio::test]
    async fn runtime_steer_appends_system_message_and_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();
        let mut observer = RecordingObserver::default();

        runtime
            .steer_with_observer(
                "stay focused on tests",
                Some("manual".to_string()),
                &mut observer,
            )
            .await
            .unwrap();

        let transcript = store.replay_transcript(&runtime.run_id()).await.unwrap();
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].role, types::MessageRole::System);
        assert_eq!(transcript[0].text_content(), "stay focused on tests");

        let events = store.events(&runtime.run_id()).await.unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::SteerApplied { message, reason }
                    if message == "stay focused on tests"
                        && reason.as_deref() == Some("manual")
            )
        }));
        assert!(observer.events.iter().any(|event| matches!(
            event,
            RuntimeProgressEvent::SteerApplied { message, reason }
                if message == "stay focused on tests" && reason.as_deref() == Some("manual")
        )));
    }

    #[tokio::test]
    async fn runtime_apply_control_runs_prompt_and_steer_commands() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryRunStore::new());
        let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();

        let steer = runtime
            .apply_control(RuntimeCommand::Steer {
                message: "prefer terse answers".to_string(),
                reason: Some("queued".to_string()),
            })
            .await
            .unwrap();
        assert!(steer.is_none());

        let prompt = runtime
            .apply_control(RuntimeCommand::Prompt {
                prompt: "hello".to_string(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prompt.assistant_text, "hello");

        let transcript = store.replay_transcript(&runtime.run_id()).await.unwrap();
        assert_eq!(transcript[0].text_content(), "prefer terse answers");
        assert_eq!(transcript[1].text_content(), "hello");
        assert_eq!(transcript[2].text_content(), "hello");
    }

    #[tokio::test]
    async fn runtime_keeps_dynamic_hook_context_append_only_and_disables_prompt_skill_matching() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryRunStore::new());
        let backend = Arc::new(RecordingBackend::default());
        let skill_catalog = SkillCatalog::new(vec![Skill {
            name: "pdf".to_string(),
            description: "Use for PDF tasks".to_string(),
            aliases: vec!["acrobat".to_string()],
            body: "Use for PDF work.".to_string(),
            root_dir: PathBuf::from("/tmp/pdf"),
            tags: vec!["document".to_string()],
            hooks: Vec::new(),
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            metadata: BTreeMap::new(),
            extension_metadata: BTreeMap::new(),
        }]);
        let hook_runner = Arc::new(HookRunner::with_services(
            Arc::new(DefaultCommandHookExecutor::default()),
            Arc::new(ReqwestHttpHookExecutor::default()),
            Arc::new(StaticPromptEvaluator),
            Arc::new(NoopAgentHookEvaluator),
        ));
        let mut runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
            .hook_runner(hook_runner)
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .instructions(vec!["static base instruction".to_string()])
            .hooks(vec![HookRegistration {
                name: "inject_context".to_string(),
                event: HookEvent::UserPromptSubmit,
                matcher: None,
                handler: HookHandler::Prompt(PromptHookHandler {
                    prompt: "ignored".to_string(),
                }),
                timeout_ms: None,
            }])
            .skill_catalog(skill_catalog)
            .build();
        let mut observer = RecordingObserver::default();

        let _outcome = runtime
            .run_user_prompt_with_observer("please use acrobat skill on this file", &mut observer)
            .await
            .unwrap();

        let requests = backend.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].instructions, vec!["static base instruction"]);
        assert!(requests[0].additional_context.is_empty());
        assert_eq!(requests[0].messages.len(), 3);
        assert_eq!(requests[0].messages[0].role, types::MessageRole::System);
        assert_eq!(
            requests[0].messages[0].text_content(),
            "hook system message"
        );
        assert_eq!(requests[0].messages[1].role, types::MessageRole::System);
        assert_eq!(
            requests[0].messages[1].text_content(),
            "hook additional context"
        );
        assert_eq!(requests[0].messages[2].role, types::MessageRole::User);
        assert_eq!(
            requests[0].messages[2].text_content(),
            "please use acrobat skill on this file"
        );

        let transcript = store.replay_transcript(&runtime.run_id()).await.unwrap();
        assert_eq!(transcript.len(), 4);
        assert_eq!(transcript[0].text_content(), "hook system message");
        assert_eq!(transcript[1].text_content(), "hook additional context");
        assert_eq!(
            transcript[2].text_content(),
            "please use acrobat skill on this file"
        );
        assert_eq!(transcript[3].text_content(), "ok");
    }

    #[tokio::test]
    async fn runtime_auto_compacts_visible_history_before_request() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryRunStore::new());
        let backend = Arc::new(RecordingBackend::default());
        let mut runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .instructions(vec!["static base instruction".to_string()])
            .conversation_compactor(Arc::new(StaticCompactor))
            .compaction_config(CompactionConfig {
                enabled: true,
                context_window_tokens: 64,
                trigger_tokens: 1,
                preserve_recent_messages: 1,
            })
            .build();

        runtime.run_user_prompt("first turn").await.unwrap();
        runtime.run_user_prompt("second turn").await.unwrap();

        let requests = backend.requests();
        assert!(requests.len() >= 2);
        let last_request = requests.last().unwrap();
        assert_eq!(last_request.instructions, vec!["static base instruction"]);
        assert_eq!(last_request.messages[0].role, types::MessageRole::System);
        assert!(
            last_request.messages[0]
                .text_content()
                .starts_with("summary for ")
        );
        assert_eq!(last_request.messages.len(), 2);
        assert_eq!(last_request.messages[1].text_content(), "second turn");

        let events = store.events(&runtime.run_id()).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RunEventKind::CompactionCompleted { .. }))
        );
        assert!(events.iter().any(|event| {
            matches!(
                event.event,
                RunEventKind::CompactionCompleted {
                    source_message_count: 2,
                    retained_message_count: 1,
                    ..
                }
            )
        }));
    }
}
