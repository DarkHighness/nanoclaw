use super::{AgentRuntime, RunTurnOutcome, provider_state::is_provider_continuation_lost};
use crate::{
    Result, RuntimeObserver, RuntimeProgressEvent, append_transcript_message,
    estimate_prompt_tokens,
};
use futures::StreamExt;
use serde_json::json;
use tracing::{debug, info, warn};
use types::{
    AgentCoreError, ContextWindowUsage, HookContext, HookEvent, HookRegistration, Message,
    MessageId, MessagePart, ModelEvent, SessionEventKind, TokenLedgerSnapshot, TokenUsage,
    TokenUsagePhase, ToolCall, TurnId,
};

struct TurnResponse {
    assistant_text: String,
    tool_calls: Vec<ToolCall>,
    assistant_reasoning: Vec<types::Reasoning>,
    assistant_message_id: Option<MessageId>,
    provider_continuation: Option<types::ProviderContinuation>,
    token_usage: Option<TokenUsage>,
}

impl AgentRuntime {
    pub(super) async fn run_turn_loop(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        instructions: &[String],
        observer: &mut dyn RuntimeObserver,
    ) -> Result<RunTurnOutcome> {
        let mut iteration = 0usize;
        loop {
            iteration = iteration.saturating_add(1);
            let _ = self
                .compact_if_needed(turn_id, instructions, observer)
                .await?;
            let response = self
                .collect_model_response(turn_id, instructions, iteration, observer)
                .await?;
            self.persist_assistant_response(turn_id, &response).await?;
            self.update_provider_continuation(response.provider_continuation.clone());

            if !response.tool_calls.is_empty() {
                for call in response.tool_calls {
                    debug!(
                        session_id = %self.session.session_id,
                        turn_id = %turn_id,
                        tool_name = %call.tool_name,
                        call_id = %call.call_id,
                        "dispatching tool call"
                    );
                    self.handle_tool_call(hooks, turn_id, call, observer)
                        .await?;
                }
                let drained = self.hook_runner.drain_async_invocations().await;
                let _ = self
                    .apply_hook_effects(turn_id, drained, None, None)
                    .await?;
                continue;
            }

            if self
                .handle_stop_hooks(turn_id, hooks, &response.assistant_text)
                .await?
            {
                continue;
            }

            self.append_event(
                Some(turn_id.clone()),
                None,
                SessionEventKind::Stop {
                    reason: Some("assistant_complete".to_string()),
                },
            )
            .await?;
            info!(
                session_id = %self.session.session_id,
                turn_id = %turn_id,
                assistant_chars = response.assistant_text.chars().count(),
                "completed user turn"
            );
            observer.on_event(RuntimeProgressEvent::TurnCompleted {
                turn_id: turn_id.clone(),
                assistant_text: response.assistant_text.clone(),
            })?;
            return Ok(RunTurnOutcome {
                turn_id: turn_id.clone(),
                assistant_text: response.assistant_text,
            });
        }
    }

    async fn collect_model_response(
        &mut self,
        turn_id: &TurnId,
        instructions: &[String],
        iteration: usize,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<TurnResponse> {
        let mut request = self.build_model_request(turn_id, instructions, false);
        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::ModelRequestStarted {
                request: request.clone(),
            },
        )
        .await?;
        let request_context_usage = ContextWindowUsage {
            used_tokens: estimate_prompt_tokens(
                &request.instructions,
                &request.messages,
                &request.tools,
                &request.additional_context,
            ),
            max_tokens: self.compaction_config.context_window_tokens,
        };
        let request_ledger = self.record_request_token_window(request_context_usage);
        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::TokenUsageUpdated {
                phase: TokenUsagePhase::RequestStarted,
                ledger: request_ledger.clone(),
            },
        )
        .await?;
        debug!(
            session_id = %self.session.session_id,
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
        observer.on_event(RuntimeProgressEvent::TokenUsageUpdated {
            phase: TokenUsagePhase::RequestStarted,
            ledger: request_ledger,
        })?;

        let used_continuation = request.continuation.is_some();
        let mut stream = match self.backend.stream_turn(request.clone()).await {
            Ok(stream) => stream,
            Err(error) if used_continuation && is_provider_continuation_lost(&error) => {
                warn!(
                    session_id = %self.session.session_id,
                    turn_id = %turn_id,
                    iteration,
                    error = %error,
                    "provider continuation was rejected; retrying with rebuilt transcript"
                );
                self.reset_provider_continuation();
                self.append_event(
                    Some(turn_id.clone()),
                    None,
                    SessionEventKind::Notification {
                        source: "provider_state".to_string(),
                        message: error.to_string(),
                    },
                )
                .await?;
                request = self.build_model_request(turn_id, instructions, true);
                self.append_event(
                    Some(turn_id.clone()),
                    None,
                    SessionEventKind::ModelRequestStarted {
                        request: request.clone(),
                    },
                )
                .await?;
                observer.on_event(RuntimeProgressEvent::ModelRequestStarted {
                    turn_id: turn_id.clone(),
                    iteration,
                })?;
                let retried_context_usage = ContextWindowUsage {
                    used_tokens: estimate_prompt_tokens(
                        &request.instructions,
                        &request.messages,
                        &request.tools,
                        &request.additional_context,
                    ),
                    max_tokens: self.compaction_config.context_window_tokens,
                };
                let retried_ledger = self.record_request_token_window(retried_context_usage);
                self.append_event(
                    Some(turn_id.clone()),
                    None,
                    SessionEventKind::TokenUsageUpdated {
                        phase: TokenUsagePhase::RequestStarted,
                        ledger: retried_ledger.clone(),
                    },
                )
                .await?;
                observer.on_event(RuntimeProgressEvent::TokenUsageUpdated {
                    phase: TokenUsagePhase::RequestStarted,
                    ledger: retried_ledger,
                })?;
                self.backend.stream_turn(request).await?
            }
            Err(error) => return Err(error),
        };

        let mut response = TurnResponse {
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            assistant_reasoning: Vec::new(),
            assistant_message_id: None,
            provider_continuation: None,
            token_usage: None,
        };
        while let Some(event) = stream.next().await {
            match event? {
                ModelEvent::TextDelta { delta } => {
                    response.assistant_text.push_str(&delta);
                    observer.on_event(RuntimeProgressEvent::AssistantTextDelta { delta })?;
                }
                ModelEvent::ToolCallRequested { call } => {
                    response.tool_calls.push(call.clone());
                    observer.on_event(RuntimeProgressEvent::ToolCallRequested { call })?;
                }
                ModelEvent::ResponseComplete {
                    message_id,
                    continuation,
                    usage,
                    reasoning,
                    ..
                } => {
                    response.assistant_message_id = Some(message_id.unwrap_or_else(MessageId::new));
                    response.provider_continuation = continuation;
                    response.assistant_reasoning = reasoning;
                    response.token_usage = usage;
                }
                ModelEvent::Error { message } => {
                    return Err(AgentCoreError::ModelBackend(message).into());
                }
            }
        }

        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::ModelResponseCompleted {
                assistant_text: response.assistant_text.clone(),
                tool_calls: response.tool_calls.clone(),
                continuation: response.provider_continuation.clone(),
            },
        )
        .await?;
        debug!(
            session_id = %self.session.session_id,
            turn_id = %turn_id,
            iteration,
            assistant_chars = response.assistant_text.chars().count(),
            tool_call_count = response.tool_calls.len(),
            "completed model response"
        );
        observer.on_event(RuntimeProgressEvent::ModelResponseCompleted {
            assistant_text: response.assistant_text.clone(),
            tool_calls: response.tool_calls.clone(),
        })?;
        let response_ledger = self.record_response_token_usage(response.token_usage);
        self.append_event(
            Some(turn_id.clone()),
            None,
            SessionEventKind::TokenUsageUpdated {
                phase: TokenUsagePhase::ResponseCompleted,
                ledger: response_ledger.clone(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::TokenUsageUpdated {
            phase: TokenUsagePhase::ResponseCompleted,
            ledger: response_ledger,
        })?;
        self.clear_pending_request_effects();
        Ok(response)
    }

    fn record_request_token_window(
        &mut self,
        context_window: ContextWindowUsage,
    ) -> TokenLedgerSnapshot {
        self.session.token_ledger.context_window = Some(context_window);
        self.session.token_ledger.clone()
    }

    fn record_response_token_usage(&mut self, usage: Option<TokenUsage>) -> TokenLedgerSnapshot {
        self.session.token_ledger.last_usage = usage;
        if let Some(usage) = self.session.token_ledger.last_usage.as_ref() {
            self.session.token_ledger.cumulative_usage.accumulate(usage);
        }
        self.session.token_ledger.clone()
    }

    async fn persist_assistant_response(
        &mut self,
        turn_id: &TurnId,
        response: &TurnResponse,
    ) -> Result<()> {
        if response.assistant_text.is_empty()
            && response.tool_calls.is_empty()
            && response.assistant_reasoning.is_empty()
        {
            return Ok(());
        }

        let mut parts = Vec::new();
        if !response.assistant_text.is_empty() {
            parts.push(MessagePart::text(response.assistant_text.clone()));
        }
        parts.extend(
            response
                .assistant_reasoning
                .iter()
                .cloned()
                .map(|reasoning| MessagePart::Reasoning { reasoning }),
        );
        parts.extend(
            response
                .tool_calls
                .iter()
                .cloned()
                .map(|call| MessagePart::ToolCall { call }),
        );
        let message = Message::assistant_parts(parts).with_message_id(
            response
                .assistant_message_id
                .clone()
                .unwrap_or_else(MessageId::new),
        );
        let event = append_transcript_message(
            &mut self.session.transcript,
            message,
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        Ok(())
    }

    async fn handle_stop_hooks(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        assistant_text: &str,
    ) -> Result<bool> {
        let stop_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: HookEvent::Stop,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    fields: [("reason".to_string(), "assistant_complete".to_string())]
                        .into_iter()
                        .collect(),
                    payload: json!({ "assistant_text": assistant_text }),
                },
            )
            .await?;
        let stop_effects = self
            .apply_hook_effects(turn_id, stop_hooks, None, None)
            .await?;
        if let Some(reason) = stop_effects.blocked_reason("stop blocked") {
            self.append_event(
                Some(turn_id.clone()),
                None,
                SessionEventKind::StopFailure {
                    reason: Some(reason.clone()),
                },
            )
            .await?;
            if stop_effects.appended_messages.is_empty()
                && stop_effects.additional_context.is_empty()
                && stop_effects.injected_instructions.is_empty()
            {
                return Err(AgentCoreError::HookBlocked(reason).into());
            }
            return Ok(true);
        }

        Ok(false)
    }
}
