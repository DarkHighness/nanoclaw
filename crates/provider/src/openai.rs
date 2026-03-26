use crate::{
    ProviderError, RequestOptions, Result, data_url, merge_top_level_object,
    render_instruction_text, stringify_json, tool_schema,
};
use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures::{StreamExt, TryStreamExt, stream::BoxStream};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use types::{
    AgentCoreError, CallId, Message, MessageId, MessagePart, MessageRole, ModelEvent, ModelRequest,
    ProviderContinuation, Reasoning, ReasoningContent, ResponseId, ToolCall, ToolCallId,
    ToolOrigin,
};

#[derive(Clone, Debug, Default)]
pub struct OpenAiResponsesOptions {
    pub chain_previous_response: bool,
    pub store: Option<bool>,
    pub server_compaction: Option<OpenAiServerCompaction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiServerCompaction {
    pub compact_threshold: usize,
}

impl OpenAiServerCompaction {
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
        .ok_or_else(|| ProviderError::config("OPENAI_API_KEY not set"))?;
    Ok(OpenAiTransport {
        api_key,
        base_url,
        http_client: reqwest::Client::new(),
    })
}

pub(crate) fn openai_capabilities(options: &RequestOptions) -> runtime::ModelBackendCapabilities {
    runtime::ModelBackendCapabilities {
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
    request_options: RequestOptions,
) -> runtime::Result<BoxStream<'static, runtime::Result<ModelEvent>>> {
    let tool_origins = request
        .tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.origin.clone()))
        .collect::<BTreeMap<_, _>>();
    let body = build_openai_responses_body(model, request, &request_options)
        .map_err(runtime::RuntimeError::from)?;
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
            .map_err(runtime::RuntimeError::from)?;

        let status = response.status();
        let mut stream = if status.is_success() {
            response
                .bytes_stream()
                .eventsource()
                .map_err(|error| runtime::RuntimeError::from(ProviderError::request(error.to_string())))
        } else {
            let body = response.text().await.map_err(runtime::RuntimeError::from)?;
            Err::<(), runtime::RuntimeError>(classify_openai_error(status.as_u16(), &body)?)?;
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

            let chunk: Value = serde_json::from_str(&event.data).map_err(runtime::RuntimeError::from)?;
            match chunk.get("type").and_then(Value::as_str) {
                Some("response.output_text.delta") | Some("response.refusal.delta") => {
                    if let Some(delta) = chunk.get("delta").and_then(Value::as_str) {
                        yield ModelEvent::TextDelta { delta: delta.to_string() };
                    }
                }
                Some("response.output_item.done") => {
                    let Some(item) = chunk.get("item") else {
                        continue;
                    };
                    match item.get("type").and_then(Value::as_str) {
                        Some("function_call") => {
                            saw_tool_call = true;
                            let tool_name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let arguments = parse_openai_arguments(item.get("arguments"));
                            yield ModelEvent::ToolCallRequested {
                                call: ToolCall {
                                    id: ToolCallId::from(
                                        item.get("id")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string(),
                                    ),
                                    call_id: item
                                        .get("call_id")
                                        .and_then(Value::as_str)
                                        .map(CallId::from)
                                        .unwrap_or_else(CallId::new),
                                    tool_name: tool_name.clone(),
                                    arguments,
                                    origin: tool_origins.get(&tool_name).cloned().unwrap_or_else(|| {
                                        ToolOrigin::Provider {
                                            provider: "openai".to_string(),
                                        }
                                    }),
                                },
                            };
                        }
                        Some("reasoning") => {
                            let content = item
                                .get("summary")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(|summary| summary.get("text").and_then(Value::as_str))
                                .map(|text| ReasoningContent::Summary(text.to_string()))
                                .collect::<Vec<_>>();
                            let mut content = content;
                            if let Some(encrypted) =
                                item.get("encrypted_content").and_then(Value::as_str)
                            {
                                content.push(ReasoningContent::Encrypted(encrypted.to_string()));
                            }
                            reasoning.push(Reasoning {
                                id: item
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned),
                                content,
                            });
                        }
                        Some("message") => {
                            message_id = item
                                .get("id")
                                .and_then(Value::as_str)
                                .map(MessageId::from);
                        }
                        _ => {}
                    }
                }
                Some("response.completed") => {
                    response_id = chunk
                        .get("response")
                        .and_then(|response| response.get("id"))
                        .and_then(Value::as_str)
                        .map(ResponseId::from);
                }
                Some("response.failed") => {
                    let message = chunk
                        .get("response")
                        .and_then(|response| response.get("error"))
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("OpenAI response failed");
                    Err::<(), runtime::RuntimeError>(
                        AgentCoreError::ModelBackend(message.to_string()).into(),
                    )?;
                }
                _ => {}
            }
        }

        yield ModelEvent::ResponseComplete {
            stop_reason: Some(if saw_tool_call { "tool_use" } else { "stop" }.to_string()),
            message_id: Some(message_id.unwrap_or_else(MessageId::new)),
            continuation: response_id.map(|response_id| ProviderContinuation::OpenAiResponses { response_id }),
            reasoning,
        };
    }))
}

fn build_openai_responses_body(
    model: String,
    request: ModelRequest,
    request_options: &RequestOptions,
) -> Result<Value> {
    let instructions = render_instruction_text(&request.instructions);
    let mut object = Map::new();
    object.insert("model".to_string(), Value::String(model));
    object.insert("stream".to_string(), Value::Bool(true));
    if let Some(instructions) = instructions {
        // Responses treats `instructions` as a request-level prompt frame.
        // Keeping it top-level avoids duplicating stable guidance into the
        // conversation chain when `previous_response_id` is active.
        object.insert("instructions".to_string(), Value::String(instructions));
    }
    if let Some(temperature) = request_options.temperature {
        object.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = request_options.max_tokens {
        object.insert("max_output_tokens".to_string(), json!(max_tokens));
    }
    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                let mut schema = tool_schema(tool);
                if let Some(object) = schema.as_object_mut() {
                    object.insert("strict".to_string(), Value::Bool(true));
                }
                schema
            })
            .collect::<Vec<_>>();
        object.insert("tools".to_string(), Value::Array(tools));
    }
    let input = serialize_openai_input_items(&request.messages)?;
    if !input.is_empty() {
        object.insert("input".to_string(), Value::Array(input));
    }
    merge_top_level_object(
        &mut object,
        request_options.additional_params.as_ref(),
        "OpenAI",
    )?;

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
    if let Some(options) = &request_options.openai_responses {
        if let Some(store) = options.store {
            object.insert("store".to_string(), Value::Bool(store));
        }
        if let Some(ProviderContinuation::OpenAiResponses { response_id }) = &request.continuation {
            if matches!(options.store, Some(false)) {
                return Err(ProviderError::config(
                    "OpenAI `previous_response_id` chaining requires stored responses; do not set store=false when chaining is enabled",
                ));
            }
            object.insert(
                "previous_response_id".to_string(),
                Value::String(response_id.to_string()),
            );
        }
        if let Some(server_compaction) = &options.server_compaction {
            object.insert(
                "context_management".to_string(),
                server_compaction.as_json(),
            );
        }
    }
    Ok(Value::Object(object))
}

fn serialize_openai_input_items(messages: &[Message]) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System | MessageRole::User => {
                let mut content = Vec::new();
                let mut trailing_items = Vec::new();
                for part in &message.parts {
                    if let Some(block) = openai_user_message_block(part) {
                        content.push(block);
                    } else if let Some(item) =
                        openai_input_item_from_part(part, message.role.clone())
                    {
                        trailing_items.push(item);
                    }
                }
                if !content.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": match message.role {
                            MessageRole::System => "system",
                            _ => "user",
                        },
                        "content": content,
                    }));
                }
                items.extend(trailing_items);
            }
            MessageRole::Assistant => {
                let mut content = Vec::new();
                let mut standalone_items = Vec::new();
                for part in &message.parts {
                    if let Some(block) = openai_assistant_message_block(part) {
                        content.push(block);
                    } else if let Some(item) =
                        openai_input_item_from_part(part, message.role.clone())
                    {
                        standalone_items.push(item);
                    }
                }
                if !content.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "id": message.message_id,
                        "status": "completed",
                        "content": content,
                    }));
                }
                items.extend(standalone_items);
            }
            MessageRole::Tool => {
                items.extend(
                    message
                        .parts
                        .iter()
                        .filter_map(|part| openai_input_item_from_part(part, message.role.clone())),
                );
            }
        }
    }
    Ok(items)
}

fn openai_user_message_block(part: &MessagePart) -> Option<Value> {
    match part {
        MessagePart::Text { text } => Some(json!({ "type": "input_text", "text": text })),
        MessagePart::Image {
            mime_type,
            data_base64,
        } => Some(json!({
            "type": "input_image",
            "image_url": data_url(mime_type, data_base64),
        })),
        MessagePart::File {
            file_name,
            mime_type,
            data_base64,
            uri,
        } => {
            if let Some(uri) = uri {
                Some(json!({
                    "type": "input_file",
                    "file_url": uri,
                    "filename": file_name,
                }))
            } else {
                data_base64.as_ref().map(|data_base64| {
                    let file_data = mime_type
                        .as_deref()
                        .map(|mime_type| data_url(mime_type, data_base64))
                        .unwrap_or_else(|| data_base64.clone());
                    json!({
                        "type": "input_file",
                        "file_data": file_data,
                        "filename": file_name,
                    })
                })
            }
        }
        MessagePart::Resource {
            uri,
            text,
            metadata,
            ..
        } => Some(json!({
            "type": "input_text",
            "text": text.clone().unwrap_or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| format!("{uri}\n{metadata}"))
                    .unwrap_or_else(|| uri.clone())
            }),
        })),
        MessagePart::Json { value } => Some(json!({
            "type": "input_text",
            "text": stringify_json(value),
        })),
        MessagePart::ProviderExtension { payload, .. } => Some(json!({
            "type": "input_text",
            "text": stringify_json(payload),
        })),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            (!text.is_empty()).then(|| json!({ "type": "input_text", "text": text }))
        }
        MessagePart::ToolCall { call } => Some(json!({
            "type": "input_text",
            "text": stringify_json(&call.arguments),
        })),
        MessagePart::ToolResult { .. } => None,
    }
}

fn openai_assistant_message_block(part: &MessagePart) -> Option<Value> {
    match part {
        MessagePart::Text { text } => Some(json!({ "type": "output_text", "text": text })),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            (!text.is_empty()).then(|| json!({ "type": "output_text", "text": text }))
        }
        MessagePart::Resource {
            uri,
            text,
            metadata,
            ..
        } => Some(json!({
            "type": "output_text",
            "text": text.clone().unwrap_or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| format!("{uri}\n{metadata}"))
                    .unwrap_or_else(|| uri.clone())
            }),
        })),
        MessagePart::Json { value } => Some(json!({
            "type": "output_text",
            "text": stringify_json(value),
        })),
        MessagePart::ProviderExtension { payload, .. } => Some(json!({
            "type": "output_text",
            "text": stringify_json(payload),
        })),
        _ => None,
    }
}

fn openai_input_item_from_part(part: &MessagePart, role: MessageRole) -> Option<Value> {
    match part {
        MessagePart::ToolCall { call } if matches!(role, MessageRole::Assistant) => Some(json!({
            "type": "function_call",
            "id": call.id,
            "call_id": call.call_id,
            "name": call.tool_name,
            "arguments": call.arguments,
            "status": "completed",
        })),
        MessagePart::ToolResult { result } => Some(json!({
            "type": "function_call_output",
            "call_id": result.call_id,
            "output": result.text_content(),
            "status": "completed",
        })),
        MessagePart::Reasoning { reasoning } if matches!(role, MessageRole::Assistant) => {
            let Some(id) = reasoning.id.clone() else {
                return None;
            };
            let mut summary = Vec::new();
            let mut encrypted_content = None;
            for content in &reasoning.content {
                match content {
                    ReasoningContent::Text { text, .. } | ReasoningContent::Summary(text) => {
                        summary.push(json!({
                            "type": "summary_text",
                            "text": text,
                        }));
                    }
                    ReasoningContent::Encrypted(data) | ReasoningContent::Redacted { data } => {
                        encrypted_content.get_or_insert_with(|| data.clone());
                    }
                }
            }
            let mut item = Map::new();
            item.insert("type".to_string(), Value::String("reasoning".to_string()));
            item.insert("id".to_string(), Value::String(id));
            item.insert("summary".to_string(), Value::Array(summary));
            if let Some(encrypted_content) = encrypted_content {
                item.insert(
                    "encrypted_content".to_string(),
                    Value::String(encrypted_content),
                );
            }
            Some(Value::Object(item))
        }
        _ => None,
    }
}

fn parse_openai_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        Some(value) => value.clone(),
        None => json!({}),
    }
}

fn classify_openai_error(status: u16, body: &str) -> Result<runtime::RuntimeError> {
    let parsed = serde_json::from_str::<Value>(body).ok();
    if parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        == Some("previous_response_not_found")
    {
        return Ok(AgentCoreError::ProviderContinuationLost(
            parsed
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("OpenAI previous_response_id could not be resumed")
                .to_string(),
        )
        .into());
    }

    let message = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|error| {
            let code = error.get("code").and_then(Value::as_str);
            let message = error.get("message").and_then(Value::as_str);
            match (code, message) {
                (Some(code), Some(message)) => Some(format!("{code}: {message}")),
                (_, Some(message)) => Some(message.to_string()),
                _ => None,
            }
        })
        .unwrap_or_else(|| format!("OpenAI Responses request failed with status {status}: {body}"));
    Ok(AgentCoreError::ModelBackend(message).into())
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAiResponsesOptions, OpenAiServerCompaction, build_openai_responses_body,
        classify_openai_error, stream_openai_responses_turn,
    };
    use crate::{PromptCacheRetention, RequestOptions};
    use futures::StreamExt;
    use serde_json::{Value, json};
    use types::{
        AgentCoreError, Message, ModelEvent, ModelRequest, ProviderContinuation, ResponseId, RunId,
        SessionId, TurnId,
    };
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn base_request() -> ModelRequest {
        ModelRequest {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec!["You are a coding agent.".to_string()],
            messages: vec![Message::user("inspect the repo")],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        }
    }

    #[test]
    fn openai_responses_body_uses_top_level_instructions_and_continuation() {
        let mut request = base_request();
        request.continuation = Some(ProviderContinuation::OpenAiResponses {
            response_id: ResponseId::from("resp_123"),
        });

        let body = build_openai_responses_body(
            "gpt-5.4".to_string(),
            request,
            &RequestOptions {
                openai_responses: Some(OpenAiResponsesOptions {
                    chain_previous_response: true,
                    store: Some(true),
                    server_compaction: None,
                }),
                ..RequestOptions::default()
            },
        )
        .unwrap();

        assert_eq!(
            body.get("instructions"),
            Some(&Value::String("You are a coding agent.".to_string()))
        );
        assert_eq!(
            body.get("previous_response_id"),
            Some(&Value::String("resp_123".to_string()))
        );
    }

    #[test]
    fn openai_responses_body_preserves_prompt_cache_and_server_compaction() {
        let body = build_openai_responses_body(
            "gpt-5.4".to_string(),
            base_request(),
            &RequestOptions {
                prompt_cache_key: Some("workspace:main".to_string()),
                prompt_cache_retention: Some(PromptCacheRetention::Hours24),
                openai_responses: Some(OpenAiResponsesOptions {
                    chain_previous_response: false,
                    store: Some(true),
                    server_compaction: Some(OpenAiServerCompaction {
                        compact_threshold: 200_000,
                    }),
                }),
                ..RequestOptions::default()
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
            Some(&json!([{ "type": "compaction", "compact_threshold": 200_000 }]))
        );
    }

    #[test]
    fn previous_response_not_found_maps_to_continuation_loss() {
        let error = classify_openai_error(
            404,
            r#"{"error":{"code":"previous_response_not_found","message":"expired"}}"#,
        )
        .unwrap();

        assert!(matches!(
            error,
            runtime::RuntimeError::AgentCore(AgentCoreError::ProviderContinuationLost(message))
                if message == "expired"
        ));
    }

    #[tokio::test]
    async fn openai_stream_emits_tool_calls_and_continuation() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hel\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\"}}\n\n",
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
            RequestOptions::default(),
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
                if call.tool_name == "read" && call.call_id.as_str() == "call_1"
        ));
        assert!(matches!(
            events.last(),
            Some(Ok(ModelEvent::ResponseComplete {
                message_id: Some(message_id),
                continuation: Some(ProviderContinuation::OpenAiResponses { response_id }),
                ..
            })) if message_id.as_str() == "msg_1" && response_id.as_str() == "resp_1"
        ));
    }
}
