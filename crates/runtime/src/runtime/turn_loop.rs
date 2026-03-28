use super::{AgentRuntime, RunTurnOutcome, provider_state::is_provider_continuation_lost};
use crate::{Result, RuntimeObserver, RuntimeProgressEvent, append_transcript_message};
use futures::StreamExt;
use serde_json::json;
use tracing::{debug, info, warn};
use types::{
    AgentCoreError, GateDecision, HookContext, HookEvent, HookRegistration, Message, MessageId,
    MessagePart, ModelEvent, RunEventKind, ToolCall, TurnId,
};

struct TurnResponse {
    assistant_text: String,
    tool_calls: Vec<ToolCall>,
    assistant_reasoning: Vec<types::Reasoning>,
    assistant_message_id: Option<MessageId>,
    provider_continuation: Option<types::ProviderContinuation>,
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
                        run_id = %self.session.run_id,
                        turn_id = %turn_id,
                        tool_name = %call.tool_name,
                        call_id = %call.call_id,
                        "dispatching tool call"
                    );
                    self.handle_tool_call(hooks, turn_id, call, observer)
                        .await?;
                }
                let drained = self.hook_runner.drain_async_context().await;
                self.append_hook_context_messages(turn_id, &drained).await?;
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
                RunEventKind::Stop {
                    reason: Some("assistant_complete".to_string()),
                },
            )
            .await?;
            info!(
                run_id = %self.session.run_id,
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
                request = self.build_model_request(turn_id, instructions, true);
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

        let mut response = TurnResponse {
            assistant_text: String::new(),
            tool_calls: Vec::new(),
            assistant_reasoning: Vec::new(),
            assistant_message_id: None,
            provider_continuation: None,
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
                    reasoning,
                    ..
                } => {
                    response.assistant_message_id = Some(message_id.unwrap_or_else(MessageId::new));
                    response.provider_continuation = continuation;
                    response.assistant_reasoning = reasoning;
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
                assistant_text: response.assistant_text.clone(),
                tool_calls: response.tool_calls.clone(),
                continuation: response.provider_continuation.clone(),
            },
        )
        .await?;
        debug!(
            run_id = %self.session.run_id,
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
        Ok(response)
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
            self.session.run_id.clone(),
            self.session.session_id.clone(),
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
            if stop_hooks.system_messages.is_empty() && stop_hooks.additional_context.is_empty() {
                return Err(AgentCoreError::HookBlocked(reason).into());
            }
            self.append_hook_context_messages(turn_id, &stop_hooks)
                .await?;
            return Ok(true);
        }

        Ok(false)
    }
}
