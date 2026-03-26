use crate::{Result, RigError, RigRequestOptions, to_completion_request};
use agent_core_runtime::{ModelBackendCapabilities, Result as RuntimeResult};
use agent_core_types::{
    AgentCoreError, ModelEvent, ModelRequest, ProviderContinuation, Reasoning,
    ReasoningContent as AgentReasoningContent, ToolCall, ToolCallId, ToolOrigin, new_opaque_id,
};
use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures::{StreamExt, TryStreamExt, stream::BoxStream};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use rig::providers::openai::responses_api::streaming::{
    ItemChunkKind, ResponseChunkKind, StreamingCompletionChunk, StreamingItemDoneOutput,
};
use rig::providers::openai::responses_api::{
    CompletionRequest as OpenAiCompletionRequest, Output, ReasoningSummary,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct RigOpenAiResponsesOptions {
    pub chain_previous_response: bool,
    pub store: Option<bool>,
    pub server_compaction: Option<RigOpenAiServerCompaction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigOpenAiServerCompaction {
    pub compact_threshold: usize,
}

impl RigOpenAiServerCompaction {
    #[must_use]
    pub fn as_json(&self) -> Value {
        json!([
            {
                "type": "compaction",
                "compact_threshold": self.compact_threshold,
            }
        ])
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OpenAiTransport {
    pub(crate) api_key: String,
    pub(crate) base_url: String,
    pub(crate) http_client: reqwest::Client,
}

impl OpenAiTransport {
    pub(crate) fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }
}

pub(crate) fn build_openai_transport(
    base_url: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<OpenAiTransport> {
    let base_url = base_url
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let api_key = api_key_override
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .ok_or_else(|| RigError::config("OPENAI_API_KEY not set"))?;
    Ok(OpenAiTransport {
        api_key,
        base_url,
        http_client: reqwest::Client::new(),
    })
}

pub(crate) fn openai_capabilities(options: &RigRequestOptions) -> ModelBackendCapabilities {
    ModelBackendCapabilities {
        provider_managed_history: options
            .openai_responses
            .as_ref()
            .is_some_and(|options| options.chain_previous_response),
        provider_native_compaction: options
            .openai_responses
            .as_ref()
            .is_some_and(|options| options.server_compaction.is_some()),
    }
}

pub(crate) async fn stream_openai_responses_turn(
    transport: OpenAiTransport,
    model: String,
    request: ModelRequest,
    request_options: RigRequestOptions,
) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
    let tool_origins = request
        .tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.origin.clone()))
        .collect::<BTreeMap<_, _>>();
    let body = build_openai_responses_body(model, request, &request_options)
        .map_err(agent_core_runtime::RuntimeError::from)?;
    let url = transport.responses_url();
    let api_key = transport.api_key.clone();
    let http_client = transport.http_client.clone();
    Ok(Box::pin(try_stream! {
        let response = http_client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(agent_core_runtime::RuntimeError::from)?;

        let status = response.status();
        let mut stream = if status.is_success() {
            response
                .bytes_stream()
                .eventsource()
                .map_err(|error| {
                    agent_core_runtime::RuntimeError::from(RigError::provider(error.to_string()))
                })
        } else {
            let body = response
                .text()
                .await
                .map_err(agent_core_runtime::RuntimeError::from)?;
            let error = classify_openai_error(status.as_u16(), &body)?;
            Err::<(), agent_core_runtime::RuntimeError>(error)?;
            unreachable!();
        };

        let mut saw_tool_call = false;
        let mut reasoning = Vec::new();
        let mut message_id = None;
        let mut response_id = None;

        while let Some(event) = stream.next().await {
            let event = event?;
            if event.data == "[DONE]" {
                break;
            }

            let chunk = serde_json::from_str::<StreamingCompletionChunk>(&event.data)
                .map_err(agent_core_runtime::RuntimeError::from)?;
            match chunk {
                StreamingCompletionChunk::Delta(item) => match item.data {
                    ItemChunkKind::OutputTextDelta(delta) => {
                        yield ModelEvent::TextDelta { delta: delta.delta };
                    }
                    ItemChunkKind::RefusalDelta(delta) => {
                        yield ModelEvent::TextDelta { delta: delta.delta };
                    }
                    ItemChunkKind::OutputItemDone(StreamingItemDoneOutput { item, .. }) => match item {
                        Output::FunctionCall(func) => {
                            saw_tool_call = true;
                            let tool_name = func.name.clone();
                            yield ModelEvent::ToolCallRequested {
                                call: ToolCall {
                                    id: ToolCallId(func.id),
                                    call_id: func.call_id,
                                    tool_name: tool_name.clone(),
                                    arguments: func.arguments,
                                    origin: tool_origins.get(&tool_name).cloned().unwrap_or_else(|| {
                                        ToolOrigin::Provider {
                                            provider: "openai".to_string(),
                                        }
                                    }),
                                },
                            };
                        }
                        Output::Reasoning {
                            id,
                            summary,
                            encrypted_content,
                            ..
                        } => {
                            reasoning.push(openai_reasoning_to_agent_reasoning(
                                id,
                                &summary,
                                encrypted_content.as_deref(),
                            ));
                        }
                        Output::Message(msg) => {
                            message_id = Some(msg.id);
                        }
                    },
                    _ => {}
                },
                StreamingCompletionChunk::Response(chunk) => {
                    response_id = Some(chunk.response.id);
                    if matches!(chunk.kind, ResponseChunkKind::ResponseFailed) {
                        let message = chunk
                            .response
                            .error
                            .map(|error| format!("{}: {}", error.code, error.message))
                            .unwrap_or_else(|| "OpenAI response failed".to_string());
                        Err::<(), agent_core_runtime::RuntimeError>(
                            AgentCoreError::ModelBackend(message).into(),
                        )?;
                    }
                }
            }
        }

        yield ModelEvent::ResponseComplete {
            stop_reason: Some(if saw_tool_call { "tool_use" } else { "stop" }.to_string()),
            message_id: Some(message_id.unwrap_or_else(new_opaque_id)),
            continuation: response_id.map(|response_id| ProviderContinuation::OpenAiResponses { response_id }),
            reasoning,
        };
    }))
}

fn build_openai_responses_body(
    model: String,
    request: ModelRequest,
    request_options: &RigRequestOptions,
) -> Result<Value> {
    let continuation = request.continuation.clone();
    let instructions = render_openai_instructions(&request.instructions);
    // OpenAI Responses keeps instructions in a dedicated top-level field. When
    // we continue with `previous_response_id`, resending those instructions as
    // normal system messages would append duplicate guidance into the chain
    // instead of replacing the active system prompt for the new turn.
    let request = ModelRequest {
        instructions: Vec::new(),
        continuation: None,
        ..request
    };
    let rig_request =
        to_completion_request(request, request_options, &crate::RigProviderKind::OpenAi)?;
    let mut openai_request = OpenAiCompletionRequest::try_from((model, rig_request))
        .map_err(|error| RigError::protocol(error.to_string()))?;
    openai_request.instructions = instructions;
    openai_request.stream = Some(true);

    if let Some(options) = &request_options.openai_responses {
        if let Some(store) = options.store {
            openai_request.additional_parameters.store = Some(store);
        }
        if let Some(previous_response_id) = continuation
            .and_then(|continuation| match continuation {
                ProviderContinuation::OpenAiResponses { response_id } => Some(response_id),
            })
        {
            if matches!(options.store, Some(false)) {
                return Err(RigError::config(
                    "OpenAI HTTP previous_response_id chaining requires stored responses; do not set store=false when chain_previous_response is enabled",
                ));
            }
            openai_request.additional_parameters.previous_response_id = Some(previous_response_id);
        }
    }

    let mut body = serde_json::to_value(openai_request)?;
    let object = body
        .as_object_mut()
        .ok_or_else(|| RigError::protocol("OpenAI Responses request body must be a JSON object"))?;

    // `rig-core`'s typed OpenAI Responses request currently normalizes unknown
    // top-level fields away. We therefore insert request-level controls such as
    // prompt caching and `context_management` after typed serialization so the
    // substrate can expose them without leaking raw JSON into runtime code.
    if let Some(prompt_cache_key) = &request_options.prompt_cache_key {
        object.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key.clone()),
        );
    }
    if let Some(prompt_cache_retention) = request_options.prompt_cache_retention {
        object.insert(
            "prompt_cache_retention".to_string(),
            Value::String(prompt_cache_retention.as_api_value().to_string()),
        );
    }
    if let Some(options) = &request_options.openai_responses
        && let Some(server_compaction) = &options.server_compaction
    {
        object.insert(
            "context_management".to_string(),
            server_compaction.as_json(),
        );
    }
    Ok(body)
}

fn render_openai_instructions(instructions: &[String]) -> Option<String> {
    let rendered = instructions
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if rendered.is_empty() {
        None
    } else {
        Some(rendered)
    }
}

fn openai_reasoning_to_agent_reasoning(
    id: String,
    summary: &[ReasoningSummary],
    encrypted_content: Option<&str>,
) -> Reasoning {
    let mut content = summary
        .iter()
        .map(|summary| match summary {
            ReasoningSummary::SummaryText { text } => AgentReasoningContent::Summary(text.clone()),
        })
        .collect::<Vec<_>>();
    if let Some(encrypted_content) = encrypted_content {
        content.push(AgentReasoningContent::Encrypted(
            encrypted_content.to_string(),
        ));
    }
    Reasoning {
        id: Some(id),
        content,
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorEnvelope {
    error: OpenAiErrorBody,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorBody {
    code: Option<String>,
    message: String,
}

fn classify_openai_error(status: u16, body: &str) -> Result<agent_core_runtime::RuntimeError> {
    let parsed = serde_json::from_str::<OpenAiErrorEnvelope>(body).ok();
    if parsed
        .as_ref()
        .and_then(|error| error.error.code.as_deref())
        == Some("previous_response_not_found")
    {
        // Conversation-state guides recommend falling back to a fresh request
        // with full input when the previous response chain cannot be resumed.
        // The runtime handles that retry after this typed error crosses the
        // backend boundary.
        return Ok(AgentCoreError::ProviderContinuationLost(
            parsed
                .map(|error| error.error.message)
                .unwrap_or_else(|| "OpenAI previous_response_id could not be resumed".to_string()),
        )
        .into());
    }

    let message = parsed
        .map(|error| match error.error.code {
            Some(code) => format!("{code}: {}", error.error.message),
            None => error.error.message,
        })
        .unwrap_or_else(|| format!("OpenAI Responses request failed with status {status}: {body}"));
    Ok(AgentCoreError::ModelBackend(message).into())
}

#[cfg(test)]
mod tests {
    use super::{
        RigOpenAiResponsesOptions, RigOpenAiServerCompaction, build_openai_responses_body,
        classify_openai_error, stream_openai_responses_turn,
    };
    use crate::{RigPromptCacheRetention, RigRequestOptions};
    use agent_core_types::{
        AgentCoreError, Message, ModelEvent, ModelRequest, ProviderContinuation, RunId, SessionId,
        TurnId,
    };
    use futures::StreamExt;
    use serde_json::{Value, json};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn base_request() -> ModelRequest {
        ModelRequest {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec![
                "You are a coding agent.".to_string(),
                "Use tools before guessing.".to_string(),
            ],
            messages: vec![Message::user("inspect the repo")],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        }
    }

    #[test]
    fn openai_responses_body_uses_top_level_instructions_and_continuation() {
        let body = build_openai_responses_body(
            "gpt-5.4".to_string(),
            ModelRequest {
                continuation: Some(ProviderContinuation::OpenAiResponses {
                    response_id: "resp_123".to_string(),
                }),
                ..base_request()
            },
            &RigRequestOptions {
                openai_responses: Some(RigOpenAiResponsesOptions {
                    chain_previous_response: true,
                    store: Some(true),
                    server_compaction: None,
                }),
                ..RigRequestOptions::default()
            },
        )
        .unwrap();

        assert_eq!(
            body.get("instructions"),
            Some(&Value::String(
                "You are a coding agent.\n\nUse tools before guessing.".to_string()
            ))
        );
        assert_eq!(
            body.get("previous_response_id"),
            Some(&Value::String("resp_123".to_string()))
        );
        assert_eq!(body.get("store"), Some(&Value::Bool(true)));
        let input = body.get("input").and_then(Value::as_array).unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(
            input[0].get("role"),
            Some(&Value::String("user".to_string()))
        );
    }

    #[test]
    fn openai_responses_body_preserves_prompt_cache_and_server_compaction() {
        let body = build_openai_responses_body(
            "gpt-5.4".to_string(),
            base_request(),
            &RigRequestOptions {
                prompt_cache_key: Some("workspace:main".to_string()),
                prompt_cache_retention: Some(RigPromptCacheRetention::Hours24),
                openai_responses: Some(RigOpenAiResponsesOptions {
                    chain_previous_response: true,
                    store: Some(true),
                    server_compaction: Some(RigOpenAiServerCompaction {
                        compact_threshold: 200_000,
                    }),
                }),
                ..RigRequestOptions::default()
            },
        )
        .unwrap();

        assert_eq!(
            body.get("prompt_cache_key"),
            Some(&Value::String("workspace:main".to_string()))
        );
        assert_eq!(
            body.get("prompt_cache_retention"),
            Some(&Value::String("24h".to_string()))
        );
        assert_eq!(
            body.get("context_management"),
            Some(&json!([
                {
                    "type": "compaction",
                    "compact_threshold": 200_000
                }
            ]))
        );
    }

    #[test]
    fn previous_response_not_found_maps_to_continuation_loss() {
        let error = classify_openai_error(
            400,
            r#"{"error":{"code":"previous_response_not_found","message":"missing chain"}}"#,
        )
        .unwrap();

        assert!(matches!(
            error,
            agent_core_runtime::RuntimeError::AgentCore(
                AgentCoreError::ProviderContinuationLost(message)
            ) if message == "missing chain"
        ));
    }

    #[tokio::test]
    async fn openai_stream_emits_tool_calls_and_continuation() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"item_id\":\"msg_1\",\"content_index\":0,\"sequence_number\":1,\"delta\":\"hel\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item_id\":\"fc_1\",\"sequence_number\":2,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\",\"call_id\":\"call_1\",\"name\":\"read\",\"status\":\"completed\"}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item_id\":\"msg_1\",\"sequence_number\":3,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[]}}\n\n",
            "data: {\"type\":\"response.completed\",\"sequence_number\":0,\"response\":{\"id\":\"resp_1\",\"object\":\"response\",\"created_at\":0,\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"instructions\":\"You are a coding agent.\",\"max_output_tokens\":null,\"model\":\"gpt-5.4\",\"usage\":{\"input_tokens\":1,\"input_tokens_details\":{\"cached_tokens\":0},\"output_tokens\":1,\"output_tokens_details\":{\"reasoning_tokens\":0},\"total_tokens\":2},\"output\":[],\"tools\":[]}}\n\n",
            "data: [DONE]\n\n"
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("accept", "text/event-stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .mount(&server)
            .await;

        let stream = stream_openai_responses_turn(
            super::OpenAiTransport {
                api_key: "test-key".to_string(),
                base_url: server.uri(),
                http_client: reqwest::Client::new(),
            },
            "gpt-5.4".to_string(),
            base_request(),
            RigRequestOptions::default(),
        )
        .await
        .unwrap();

        let events = stream.collect::<Vec<_>>().await;
        assert!(matches!(
            &events[0],
            Ok(ModelEvent::TextDelta { delta }) if delta == "hel"
        ));
        assert!(matches!(
            &events[1],
            Ok(ModelEvent::ToolCallRequested { call })
                if call.tool_name == "read" && call.call_id == "call_1"
        ));
        assert!(matches!(
            events.last(),
            Some(Ok(ModelEvent::ResponseComplete {
                message_id: Some(message_id),
                continuation: Some(ProviderContinuation::OpenAiResponses { response_id }),
                ..
            })) if message_id == "msg_1" && response_id == "resp_1"
        ));
    }
}
