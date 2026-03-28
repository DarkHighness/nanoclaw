use crate::{
    ProviderError, RequestOptions, Result, coerce_object_schema, merge_top_level_object,
    message_part_text, render_instruction_text, tool_result_roundtrip_text,
};
use agent_env::vars;
use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures::{StreamExt, TryStreamExt, stream::BoxStream};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use runtime::Result as RuntimeResult;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use tracing::debug;
use types::{
    AgentCoreError, CallId, MessageId, MessagePart, MessageRole, ModelEvent, ModelRequest,
    Reasoning, ReasoningContent, ReasoningId, TokenUsage, ToolCall, ToolCallId, ToolName,
    ToolOrigin, ToolResult,
};

const DEFAULT_ANTHROPIC_MAX_TOKENS: u64 = 4_096;

#[derive(Clone, Debug)]
pub(crate) struct AnthropicTransport {
    pub(crate) api_key: String,
    pub(crate) base_url: String,
    pub(crate) http_client: reqwest::Client,
}

impl AnthropicTransport {
    pub(crate) fn messages_url(&self) -> String {
        format!("{}/messages", self.base_url.trim_end_matches('/'))
    }
}

pub(crate) fn build_anthropic_transport(
    base_url: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<AnthropicTransport> {
    let base_url = base_url
        .map(ToOwned::to_owned)
        .or_else(|| agent_env::get_non_empty(vars::ANTHROPIC_BASE_URL))
        .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
    let api_key = api_key_override
        .map(ToOwned::to_owned)
        .or_else(|| agent_env::get_non_empty(vars::ANTHROPIC_API_KEY))
        .ok_or_else(|| ProviderError::config(format!("{} not set", vars::ANTHROPIC_API_KEY.key)))?;
    Ok(AnthropicTransport {
        api_key,
        base_url,
        http_client: reqwest::Client::new(),
    })
}

pub(crate) async fn stream_anthropic_turn(
    transport: AnthropicTransport,
    model: String,
    request: ModelRequest,
    request_options: RequestOptions,
) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
    debug!(
        provider = "anthropic",
        model = %model,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        "starting Anthropic stream turn"
    );
    let tool_origins = request
        .tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.origin.clone()))
        .collect::<BTreeMap<_, _>>();
    let body = build_anthropic_messages_body(model, request, &request_options)
        .map_err(runtime::RuntimeError::from)?;
    let url = transport.messages_url();
    let api_key = transport.api_key.clone();
    let http_client = transport.http_client.clone();

    Ok(Box::pin(try_stream! {
        let response = http_client
            .post(url)
            .header("anthropic-version", "2023-06-01")
            .header(AUTHORIZATION, format!("Bearer {api_key}"))
            .header("x-api-key", api_key)
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
            Err::<(), runtime::RuntimeError>(classify_anthropic_error(status.as_u16(), &body)?)?;
            unreachable!();
        };

        let mut message_id = None;
        let mut stop_reason = None;
        let mut reasoning = Vec::new();
        let mut blocks = BTreeMap::<u64, AnthropicBlockState>::new();
        let mut usage = TokenUsage::default();

        while let Some(event) = stream.next().await {
            let event = event?;
            if event.data == "[DONE]" {
                break;
            }
            let payload: Value = serde_json::from_str(&event.data)
                .map_err(runtime::RuntimeError::from)?;
            let kind = payload
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();

            match kind {
                "message_start" => {
                    message_id = payload
                        .get("message")
                        .and_then(|value| value.get("id"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    apply_anthropic_usage(
                        &mut usage,
                        payload
                            .get("message")
                            .and_then(|value| value.get("usage")),
                    );
                }
                "message_delta" => {
                    stop_reason = payload
                        .get("delta")
                        .and_then(|value| value.get("stop_reason"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .or(stop_reason);
                    apply_anthropic_usage(&mut usage, payload.get("usage"));
                }
                "content_block_start" => {
                    let index = payload
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| ProviderError::protocol("Anthropic stream missing block index"))?;
                    let block = payload
                        .get("content_block")
                        .ok_or_else(|| ProviderError::protocol("Anthropic stream missing content_block"))?;
                    blocks.insert(index, AnthropicBlockState::from_start(block)?);
                }
                "content_block_delta" => {
                    let index = payload
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| ProviderError::protocol("Anthropic stream missing block index"))?;
                    let delta = payload
                        .get("delta")
                        .ok_or_else(|| ProviderError::protocol("Anthropic stream missing delta"))?;
                    if let Some(block) = blocks.get_mut(&index) {
                        if let Some(text) = block.apply_delta(delta)? {
                            yield ModelEvent::TextDelta { delta: text };
                        }
                    }
                }
                "content_block_stop" => {
                    let index = payload
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| ProviderError::protocol("Anthropic stream missing block index"))?;
                    if let Some(block) = blocks.remove(&index) {
                        match block.finish(tool_origins.clone())? {
                            Some(AnthropicBlockOutput::ToolCall(call)) => {
                                yield ModelEvent::ToolCallRequested { call };
                            }
                            Some(AnthropicBlockOutput::Reasoning(item)) => reasoning.push(item),
                            None => {}
                        }
                    }
                }
                "error" => {
                    let message = payload
                        .get("error")
                        .and_then(|value| value.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("Anthropic stream error");
                    Err::<(), runtime::RuntimeError>(AgentCoreError::ModelBackend(message.to_string()).into())?;
                }
                "message_stop" => break,
                _ => {}
            }
        }

        yield ModelEvent::ResponseComplete {
            stop_reason,
            message_id: Some(message_id.map(MessageId::from).unwrap_or_else(MessageId::new)),
            continuation: None,
            usage: (!usage.is_zero()).then_some(usage),
            reasoning,
        };
    }))
}

fn apply_anthropic_usage(target: &mut TokenUsage, usage: Option<&Value>) {
    let Some(usage) = usage else {
        return;
    };
    if let Some(input_tokens) = usage.get("input_tokens").and_then(Value::as_u64) {
        let cache_read_tokens = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(target.cache_read_tokens);
        target.input_tokens = input_tokens;
        target.cache_read_tokens = cache_read_tokens;
        target.prefill_tokens = input_tokens.saturating_sub(cache_read_tokens);
    }
    if let Some(output_tokens) = usage.get("output_tokens").and_then(Value::as_u64) {
        target.output_tokens = output_tokens;
        target.decode_tokens = output_tokens;
    }
}

fn build_anthropic_messages_body(
    model: String,
    request: ModelRequest,
    request_options: &RequestOptions,
) -> Result<Value> {
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(model));
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "max_tokens".to_string(),
        Value::Number(serde_json::Number::from(
            request_options
                .max_tokens
                .unwrap_or(DEFAULT_ANTHROPIC_MAX_TOKENS),
        )),
    );
    if let Some(temperature) = request_options.temperature
        && let Some(value) = serde_json::Number::from_f64(temperature)
    {
        body.insert("temperature".to_string(), Value::Number(value));
    }

    let mut system_blocks = request
        .instructions
        .iter()
        .filter_map(|text| render_instruction_text(std::slice::from_ref(text)))
        .map(|text| json!({"type":"text","text": text}))
        .collect::<Vec<_>>();
    system_blocks.extend(
        request
            .additional_context
            .iter()
            .filter_map(|text| render_instruction_text(std::slice::from_ref(text)))
            .map(|text| json!({"type":"text","text": text})),
    );

    let mut messages = Vec::new();
    for message in request.messages {
        match message.role {
            MessageRole::System => {
                if let Some(text) = render_instruction_text(&[message.text_content()]) {
                    // Anthropic exposes system guidance only as a top-level
                    // field. Transcript system messages are therefore folded
                    // into the current system block instead of being emitted as
                    // in-band chat messages.
                    system_blocks.push(json!({"type":"text","text": text}));
                }
            }
            MessageRole::User => messages.push(anthropic_message("user", message.parts)?),
            MessageRole::Assistant => messages.push(anthropic_message("assistant", message.parts)?),
            MessageRole::Tool => messages.push(anthropic_tool_message(message.parts)?),
        }
    }

    if messages.is_empty() {
        return Err(ProviderError::protocol(
            "Anthropic request did not contain any user or assistant messages",
        ));
    }

    body.insert("messages".to_string(), Value::Array(messages));
    if !system_blocks.is_empty() {
        body.insert("system".to_string(), Value::Array(system_blocks));
    }
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": coerce_object_schema(&tool.input_schema),
                        })
                    })
                    .collect(),
            ),
        );
    }
    merge_top_level_object(
        &mut body,
        request_options.additional_params.as_ref(),
        "Anthropic",
    )?;
    Ok(Value::Object(body))
}

fn anthropic_message(role: &str, parts: Vec<MessagePart>) -> Result<Value> {
    let mut content = Vec::new();
    for part in parts {
        match part {
            MessagePart::Text { text } => content.push(json!({"type":"text","text": text})),
            MessagePart::Image {
                mime_type,
                data_base64,
            } => content.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": mime_type,
                    "data": data_base64,
                }
            })),
            MessagePart::ToolCall { call } => content.push(json!({
                "type": "tool_use",
                "id": call.call_id,
                "name": call.tool_name,
                "input": call.arguments,
            })),
            MessagePart::Reasoning { reasoning } => {
                let text = reasoning.display_text();
                if !text.is_empty() {
                    content.push(json!({"type":"text","text": text}));
                }
            }
            MessagePart::ToolResult { result } => content.push(tool_result_block(result)),
            other => {
                if let Some(text) = message_part_text(&other) {
                    content.push(json!({"type":"text","text": text}));
                }
            }
        }
    }
    if content.is_empty() {
        return Err(ProviderError::protocol(format!(
            "Anthropic {role} message did not contain any supported content"
        )));
    }
    Ok(json!({
        "role": role,
        "content": content,
    }))
}

fn anthropic_tool_message(parts: Vec<MessagePart>) -> Result<Value> {
    let mut content = Vec::new();
    for part in parts {
        match part {
            MessagePart::ToolResult { result } => content.push(tool_result_block(result)),
            other => {
                if let Some(text) = message_part_text(&other) {
                    content.push(json!({"type":"text","text": text}));
                }
            }
        }
    }
    if content.is_empty() {
        return Err(ProviderError::protocol(
            "Anthropic tool message did not contain any supported content",
        ));
    }
    Ok(json!({
        "role": "user",
        "content": content,
    }))
}

fn tool_result_block(result: ToolResult) -> Value {
    // Anthropic tool results do not expose a separate structured payload field.
    // Keep plain text results compact, but serialize the stable round-trip
    // envelope whenever a richer local result would otherwise be flattened away.
    let text = tool_result_roundtrip_text(&result);
    json!({
        "type": "tool_result",
        "tool_use_id": result.call_id,
        "is_error": result.is_error,
        "content": [
            {
                "type": "text",
                "text": text,
            }
        ]
    })
}

fn classify_anthropic_error(status: u16, body: &str) -> Result<runtime::RuntimeError> {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Anthropic request failed with status {status}: {body}"));
    Ok(AgentCoreError::ModelBackend(message).into())
}

enum AnthropicBlockState {
    Text,
    ToolUse {
        id: ToolCallId,
        name: ToolName,
        input_json: String,
    },
    Thinking {
        id: Option<ReasoningId>,
        text: String,
        signature: Option<String>,
    },
}

enum AnthropicBlockOutput {
    ToolCall(ToolCall),
    Reasoning(Reasoning),
}

impl AnthropicBlockState {
    fn from_start(block: &Value) -> Result<Self> {
        match block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text" => Ok(Self::Text),
            "tool_use" => Ok(Self::ToolUse {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProviderError::protocol("Anthropic tool_use block missing id"))?
                    .into(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::protocol("Anthropic tool_use block missing name")
                    })?
                    .into(),
                input_json: block
                    .get("input")
                    .filter(|value| !value.is_null() && *value != &json!({}))
                    .map(Value::to_string)
                    .unwrap_or_default(),
            }),
            "thinking" => Ok(Self::Thinking {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ReasoningId::from),
                text: String::new(),
                signature: None,
            }),
            other => Err(ProviderError::protocol(format!(
                "unsupported Anthropic content block type `{other}`"
            ))),
        }
    }

    fn apply_delta(&mut self, delta: &Value) -> Result<Option<String>> {
        match self {
            Self::Text => {
                if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                    return Ok(delta
                        .get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned));
                }
                Ok(None)
            }
            Self::ToolUse { input_json, .. } => {
                if delta.get("type").and_then(Value::as_str) == Some("input_json_delta")
                    && let Some(partial) = delta.get("partial_json").and_then(Value::as_str)
                {
                    input_json.push_str(partial);
                }
                Ok(None)
            }
            Self::Thinking {
                text, signature, ..
            } => match delta
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "thinking_delta" => {
                    if let Some(partial) = delta.get("thinking").and_then(Value::as_str) {
                        text.push_str(partial);
                    }
                    Ok(None)
                }
                "signature_delta" => {
                    *signature = delta
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    Ok(None)
                }
                _ => Ok(None),
            },
        }
    }

    fn finish(
        self,
        tool_origins: BTreeMap<ToolName, ToolOrigin>,
    ) -> Result<Option<AnthropicBlockOutput>> {
        match self {
            Self::Text => Ok(None),
            Self::ToolUse {
                id,
                name,
                input_json,
            } => {
                let arguments = if input_json.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&input_json).map_err(ProviderError::Json)?
                };
                let origin = tool_origins.get(name.as_str()).cloned().unwrap_or_else(|| {
                    ToolOrigin::Provider {
                        provider: "anthropic".to_string(),
                    }
                });
                Ok(Some(AnthropicBlockOutput::ToolCall(ToolCall {
                    call_id: CallId::from(&id),
                    id,
                    tool_name: name,
                    arguments,
                    origin,
                })))
            }
            Self::Thinking {
                id,
                text,
                signature,
            } => {
                if text.is_empty() && signature.is_none() {
                    return Ok(None);
                }
                Ok(Some(AnthropicBlockOutput::Reasoning(Reasoning {
                    id,
                    content: vec![ReasoningContent::Text { text, signature }],
                })))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_anthropic_messages_body, stream_anthropic_turn};
    use crate::{AnthropicTransport, ProviderDescriptor, RequestOptions};
    use futures::StreamExt;
    use serde_json::{Value, json};
    use types::{
        Message, ModelEvent, ModelRequest, RunId, SessionId, TokenUsage, ToolName, ToolOrigin,
        ToolOutputMode, ToolSpec, TurnId,
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
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "Read a file".to_string(),
                input_schema: json!({"properties":{"path":{"type":"string"}}}),
                output_mode: ToolOutputMode::Text,
                output_schema: None,
                origin: ToolOrigin::Local,
                annotations: Default::default(),
            }],
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        }
    }

    #[test]
    fn anthropic_body_moves_system_guidance_to_top_level_field() {
        let body = build_anthropic_messages_body(
            "claude-sonnet-4-6".to_string(),
            base_request(),
            &RequestOptions::default(),
        )
        .unwrap();

        assert_eq!(
            body.get("system"),
            Some(&json!([{ "type": "text", "text": "You are a coding agent." }]))
        );
        assert_eq!(
            body.get("messages")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn anthropic_stream_emits_text_and_tool_calls() {
        let server = MockServer::start().await;
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":120,\"cache_read_input_tokens\":20}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":30}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
            "data: [DONE]\n\n"
        );
        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("accept", "text/event-stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .mount(&server)
            .await;

        let stream = stream_anthropic_turn(
            AnthropicTransport {
                api_key: "test-key".to_string(),
                base_url: server.uri(),
                http_client: reqwest::Client::new(),
            },
            ProviderDescriptor::anthropic("claude-sonnet-4-6").model,
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
                if call.tool_name == ToolName::from("read")
                    && call.call_id.as_str() == "toolu_1"
                    && call.arguments == json!({"path":"README.md"})
        ));
        assert!(matches!(
            &events[2],
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some(reason),
                message_id: Some(message_id),
                continuation: None,
                usage: Some(usage),
                ..
            }) if reason == "tool_use"
                && message_id.as_str() == "msg_1"
                && *usage == TokenUsage::from_input_output(120, 30, 20)
        ));
    }
}
