use crate::{ProviderError, RequestOptions, Result};
use agent_env::vars;
use async_stream::try_stream;
use eventsource_stream::Eventsource;
use futures::{SinkExt, StreamExt, TryStreamExt, stream::BoxStream};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
use tracing::debug;
use types::{
    AgentCoreError, MessageId, ModelEvent, ModelRequest, ProviderContinuation, ResponseId,
    TokenUsage,
};

mod message_codec;
mod payload;

use message_codec::{
    parse_openai_message_id, parse_openai_reasoning_item, parse_openai_tool_call_item,
};
pub(crate) use payload::{build_openai_realtime_request_event, build_openai_responses_body};

const OPENAI_MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default)]
pub struct OpenAiResponsesOptions {
    pub chain_previous_response: bool,
    pub store: Option<bool>,
    pub server_compaction: Option<OpenAiServerCompaction>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenAiTransportMode {
    #[default]
    ResponsesHttp,
    RealtimeWebSocket,
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

    pub(crate) fn realtime_ws_url(&self, model: &str) -> Result<String> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| ProviderError::config_with_source("invalid OpenAI base URL", error))?;
        let path = format!("{}/realtime", url.path().trim_end_matches('/'));
        url.set_path(&path);
        match url.scheme() {
            "https" => {
                url.set_scheme("wss")
                    .map_err(|_| ProviderError::config("failed to set wss scheme"))?;
            }
            "http" => {
                url.set_scheme("ws")
                    .map_err(|_| ProviderError::config("failed to set ws scheme"))?;
            }
            "wss" | "ws" => {}
            other => {
                return Err(ProviderError::config(format!(
                    "unsupported OpenAI base URL scheme `{other}` for websocket transport"
                )));
            }
        }
        url.query_pairs_mut().append_pair("model", model);
        Ok(url.to_string())
    }
}

pub(crate) fn build_openai_transport(
    base_url: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<OpenAiTransport> {
    let base_url = base_url
        .map(ToOwned::to_owned)
        .or_else(|| agent_env::get_non_empty(vars::OPENAI_BASE_URL))
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let api_key = api_key_override
        .map(ToOwned::to_owned)
        .or_else(|| agent_env::get_non_empty(vars::OPENAI_API_KEY))
        .ok_or_else(|| ProviderError::config(format!("{} not set", vars::OPENAI_API_KEY.key)))?;
    Ok(OpenAiTransport {
        api_key,
        base_url,
        http_client: reqwest::Client::new(),
    })
}

pub(crate) fn openai_capabilities(options: &RequestOptions) -> runtime::ModelBackendCapabilities {
    let openai_options = options.openai_responses.as_ref();
    let websocket_transport = matches!(
        options.openai_transport,
        Some(OpenAiTransportMode::RealtimeWebSocket)
    );
    runtime::ModelBackendCapabilities {
        provider_managed_history: !websocket_transport
            && openai_options.is_some_and(|options| options.chain_previous_response),
        provider_native_compaction: !websocket_transport
            && openai_options.is_some_and(|options| options.server_compaction.is_some()),
        ..runtime::ModelBackendCapabilities::default()
    }
}

pub(crate) async fn stream_openai_turn(
    transport: OpenAiTransport,
    model: String,
    request: ModelRequest,
    request_options: RequestOptions,
) -> runtime::Result<BoxStream<'static, runtime::Result<ModelEvent>>> {
    if matches!(
        request_options.openai_transport,
        Some(OpenAiTransportMode::RealtimeWebSocket)
    ) {
        stream_openai_realtime_turn(transport, model, request, request_options).await
    } else {
        stream_openai_responses_turn(transport, model, request, request_options).await
    }
}

pub(crate) async fn stream_openai_responses_turn(
    transport: OpenAiTransport,
    model: String,
    request: ModelRequest,
    request_options: RequestOptions,
) -> runtime::Result<BoxStream<'static, runtime::Result<ModelEvent>>> {
    debug!(
        transport = "responses_http",
        model = %model,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        "starting OpenAI Responses turn"
    );
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
        let response = send_openai_responses_request(&http_client, &url, &api_key, &body).await?;

        let status = response.status();
        let mut stream = if status.is_success() {
            response
                .bytes_stream()
                .eventsource()
                .map_err(|error| {
                    runtime::RuntimeError::from(ProviderError::request_with_source(
                        "failed to decode OpenAI Responses event stream",
                        error,
                    ))
                })
        } else {
            let retry_after = retry_after_delay(response.headers());
            let body = response.text().await.map_err(|error| {
                runtime::RuntimeError::from(ProviderError::request_with_source(
                    "failed to read OpenAI Responses error body",
                    error,
                ))
            })?;
            Err::<(), runtime::RuntimeError>(classify_openai_error(
                status.as_u16(),
                &body,
                retry_after,
            )?)?;
            unreachable!();
        };

        let mut saw_tool_call = false;
        let mut reasoning = Vec::new();
        let mut message_id = None;
        let mut response_id = None;
        let mut usage = None;

        while let Some(event) = stream.next().await {
            let event = event?;
            if event.data == "[DONE]" {
                break;
            }

            let chunk: Value = serde_json::from_str(&event.data).map_err(|error| {
                runtime::RuntimeError::from(ProviderError::protocol_with_source(
                    "failed to parse OpenAI Responses event payload",
                    error,
                ))
            })?;
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
                    if let Some(call) = parse_openai_tool_call_item(item, &tool_origins) {
                        saw_tool_call = true;
                        yield ModelEvent::ToolCallRequested { call };
                        continue;
                    }
                    if let Some(parsed_reasoning) = parse_openai_reasoning_item(item) {
                        reasoning.push(parsed_reasoning);
                    }
                    if let Some(parsed_message_id) = parse_openai_message_id(item) {
                        message_id = Some(parsed_message_id);
                    }
                }
                Some("response.completed") => {
                    usage = parse_openai_usage(
                        chunk.get("response").and_then(|response| response.get("usage"))
                    );
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
            usage,
            reasoning,
        };
    }))
}

pub(crate) async fn stream_openai_realtime_turn(
    transport: OpenAiTransport,
    model: String,
    request: ModelRequest,
    request_options: RequestOptions,
) -> runtime::Result<BoxStream<'static, runtime::Result<ModelEvent>>> {
    debug!(
        transport = "realtime_websocket",
        model = %model,
        message_count = request.messages.len(),
        tool_count = request.tools.len(),
        "starting OpenAI realtime turn"
    );
    if request.continuation.is_some() {
        // Realtime websocket sessions are currently modeled as single-turn exchanges in this substrate path.
        // That means Responses continuation ids cannot be resumed over this transport yet.
        return Err(
            ProviderError::config(
                "OpenAI realtime websocket mode does not support `previous_response_id` continuation in this implementation",
            )
            .into(),
        );
    }

    let tool_origins = request
        .tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.origin.clone()))
        .collect::<BTreeMap<_, _>>();
    let websocket_model = model.clone();
    let request_event = build_openai_realtime_request_event(model, request, &request_options)
        .map_err(runtime::RuntimeError::from)?;
    let ws_url = transport.realtime_ws_url(&websocket_model)?;
    let mut ws_request = ws_url.into_client_request().map_err(|error| {
        ProviderError::request_with_source(
            "failed to build OpenAI realtime websocket request",
            error,
        )
    })?;
    ws_request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", transport.api_key).parse().map_err(
            |error: reqwest::header::InvalidHeaderValue| {
                ProviderError::request_with_source(
                    "failed to encode OpenAI realtime authorization header",
                    error,
                )
            },
        )?,
    );
    ws_request.headers_mut().insert(
        "OpenAI-Beta",
        "realtime=v1"
            .parse()
            .map_err(|error: reqwest::header::InvalidHeaderValue| {
                ProviderError::request_with_source(
                    "failed to encode OpenAI realtime beta header",
                    error,
                )
            })?,
    );

    Ok(Box::pin(try_stream! {
        let (ws_stream, _) = connect_async(ws_request)
            .await
            .map_err(|error| {
                runtime::RuntimeError::from(ProviderError::request_with_source(
                    "failed to establish OpenAI realtime websocket connection",
                    error,
                ))
            })?;
        let (mut ws_sink, mut ws_source) = ws_stream.split();

        ws_sink
            .send(WsMessage::Text(request_event.to_string().into()))
            .await
            .map_err(|error| {
                runtime::RuntimeError::from(ProviderError::request_with_source(
                    "failed to send OpenAI realtime request event",
                    error,
                ))
            })?;

        let mut saw_tool_call = false;
        let mut reasoning = Vec::new();
        let mut message_id = None;
        let mut response_id = None;
        let mut emitted_tool_call_ids = HashSet::<String>::new();
        let mut usage = None;

        while let Some(frame) = ws_source.next().await {
            let frame = frame
                .map_err(|error| {
                    runtime::RuntimeError::from(ProviderError::request_with_source(
                        "failed to read OpenAI realtime websocket frame",
                        error,
                    ))
                })?;
            let WsMessage::Text(text) = frame else {
                if matches!(frame, WsMessage::Close(_)) {
                    break;
                }
                continue;
            };

            let chunk: Value = serde_json::from_str(&text).map_err(|error| {
                runtime::RuntimeError::from(ProviderError::protocol_with_source(
                    "failed to parse OpenAI realtime event payload",
                    error,
                ))
            })?;
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
                    if let Some(call) = parse_openai_tool_call_item(item, &tool_origins) {
                        saw_tool_call = true;
                        emitted_tool_call_ids.insert(call.id.to_string());
                        yield ModelEvent::ToolCallRequested { call };
                        continue;
                    }
                    if let Some(parsed_reasoning) = parse_openai_reasoning_item(item) {
                        reasoning.push(parsed_reasoning);
                    }
                    if let Some(parsed_message_id) = parse_openai_message_id(item) {
                        message_id = Some(parsed_message_id);
                    }
                }
                Some("response.done") => {
                    let Some(response) = chunk.get("response") else {
                        break;
                    };
                    if response
                        .get("status")
                        .and_then(Value::as_str)
                        == Some("failed")
                    {
                        let message = response
                            .get("error")
                            .and_then(|error| error.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("OpenAI realtime response failed");
                        Err::<(), runtime::RuntimeError>(
                            AgentCoreError::ModelBackend(message.to_string()).into(),
                        )?;
                    }
                    response_id = response
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ResponseId::from);
                    usage = parse_openai_usage(response.get("usage"));

                    if let Some(items) = response.get("output").and_then(Value::as_array) {
                        for item in items {
                            if let Some(call) = parse_openai_tool_call_item(item, &tool_origins) {
                                let call_id = call.id.to_string();
                                if emitted_tool_call_ids.insert(call_id) {
                                    saw_tool_call = true;
                                    yield ModelEvent::ToolCallRequested { call };
                                }
                                continue;
                            }
                            if message_id.is_none() {
                                message_id = parse_openai_message_id(item);
                            }
                            if let Some(parsed_reasoning) = parse_openai_reasoning_item(item) {
                                reasoning.push(parsed_reasoning);
                            }
                        }
                    }
                    break;
                }
                Some("error") => {
                    let message = chunk
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("OpenAI realtime socket error");
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
            continuation: response_id
                .map(|response_id| ProviderContinuation::OpenAiResponses { response_id }),
            usage,
            reasoning,
        };
    }))
}

async fn send_openai_responses_request(
    http_client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> runtime::Result<reqwest::Response> {
    http_client
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/event-stream")
        .json(body)
        .send()
        .await
        .map_err(|error| {
            runtime::RuntimeError::from(ProviderError::request_with_source(
                "failed to send OpenAI Responses request",
                error,
            ))
        })
}

fn is_retryable_openai_responses_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

fn retry_after_delay(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(seconds).min(OPENAI_MAX_RETRY_AFTER))
}

fn parse_openai_usage(usage: Option<&Value>) -> Option<TokenUsage> {
    let usage = usage?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut usage = TokenUsage::from_input_output(input_tokens, output_tokens, cache_read_tokens);
    usage.reasoning_tokens = reasoning_tokens;
    (!usage.is_zero()).then_some(usage)
}

fn classify_openai_error(
    status: u16,
    body: &str,
    retry_after: Option<Duration>,
) -> Result<runtime::RuntimeError> {
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
    Ok(runtime::RuntimeError::model_backend_request(
        message,
        status,
        is_retryable_openai_responses_status(status),
        retry_after,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAiResponsesOptions, OpenAiServerCompaction, OpenAiTransportMode,
        build_openai_realtime_request_event, build_openai_responses_body, classify_openai_error,
        stream_openai_realtime_turn, stream_openai_responses_turn,
    };
    use crate::{PromptCacheRetention, RequestOptions};
    use futures::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use types::{
        AgentCoreError, AgentSessionId, CallId, Message, MessagePart, ModelEvent, ModelRequest,
        ProviderContinuation, Reasoning, ReasoningContent, ReasoningId, ResponseId, SessionId,
        TokenUsage, ToolCall, ToolCallId, ToolFreeformFormat, ToolName, ToolOrigin, ToolOutputMode,
        ToolResult, ToolSpec, TurnId,
    };
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn base_request() -> ModelRequest {
        ModelRequest {
            session_id: SessionId::new(),
            agent_session_id: AgentSessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec!["You are a coding agent.".to_string()],
            messages: vec![Message::user("inspect the repo")],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        }
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) {
        let mut buffer = Vec::new();
        let mut header_end = None;

        loop {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(
                read > 0,
                "client closed before sending a complete HTTP request"
            );
            buffer.extend_from_slice(&chunk[..read]);

            if header_end.is_none() {
                header_end = buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| index + 4);
            }

            let Some(header_end) = header_end else {
                continue;
            };
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if buffer.len() >= header_end + content_length {
                return;
            }
        }
    }

    #[test]
    fn openai_responses_body_uses_top_level_instructions_and_continuation() {
        let mut request = base_request();
        request.additional_context = vec!["hook additional context".to_string()];
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
            Some(&Value::String(
                "You are a coding agent.\n\nhook additional context".to_string()
            ))
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
    fn openai_responses_body_explicitly_opts_out_of_default_strict_tools() {
        let mut request = base_request();
        request.tools.push(ToolSpec::function(
            "close_agent",
            "Cancel a child agent",
            json!({
                "properties": {
                    "agent_id": {"type": "string"},
                    "metadata": {
                        "properties": {
                            "reason": {"type": "string"}
                        }
                    }
                },
                "required": ["agent_id"]
            }),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            types::ToolSource::Builtin,
        ));

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["tools"][0]["strict"], json!(false));
        assert_eq!(
            body["tools"][0]["parameters"]["required"],
            json!(["agent_id"])
        );
    }

    #[test]
    fn openai_responses_body_projects_dual_transport_patch_files_as_custom_tools_on_gpt5() {
        let mut request = base_request();
        request.tools.push(
            ToolSpec::function(
                "patch_files",
                "Apply staged patch files",
                json!({"type":"object","properties":{"operations":{"type":"array"}}}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                types::ToolSource::Builtin,
            )
            .with_freeform_format(ToolFreeformFormat::grammar("lark", "start: begin_patch"))
            .with_freeform_availability(types::ToolAvailability {
                provider_allowlist: vec!["openai".to_string()],
                model_allowlist: vec!["gpt-5*".to_string()],
                ..Default::default()
            }),
        );

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["tools"][0]["type"], json!("custom"));
        assert_eq!(body["tools"][0]["name"], json!("patch_files"));
        assert_eq!(body["tools"][0]["format"]["type"], json!("grammar"));
        assert!(body["tools"][0].get("strict").is_none());
    }

    #[test]
    fn openai_responses_body_projects_dual_transport_patch_files_as_function_on_non_gpt5() {
        let mut request = base_request();
        request.tools.push(
            ToolSpec::function(
                "patch_files",
                "Apply staged patch files",
                json!({"type":"object","properties":{"operations":{"type":"array"}}}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                types::ToolSource::Builtin,
            )
            .with_freeform_format(ToolFreeformFormat::grammar("lark", "start: begin_patch"))
            .with_freeform_availability(types::ToolAvailability {
                provider_allowlist: vec!["openai".to_string()],
                model_allowlist: vec!["gpt-5*".to_string()],
                ..Default::default()
            }),
        );

        let body = build_openai_responses_body(
            "gpt-4.1-mini".to_string(),
            request,
            &RequestOptions::default(),
        )
        .unwrap();

        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["name"], json!("patch_files"));
        assert_eq!(body["tools"][0]["strict"], json!(false));
    }

    #[test]
    fn openai_responses_body_reencodes_tool_call_arguments_for_replay() {
        let call = ToolCall {
            id: ToolCallId::from("fc_123"),
            call_id: CallId::from("call_123"),
            tool_name: ToolName::from("read"),
            arguments: json!({"path":"README.md","line_count":1}),
            origin: ToolOrigin::Local,
        };
        let result = ToolResult::text(call.id.clone(), call.tool_name.clone(), "ok")
            .with_call_id(call.call_id.clone());

        let mut request = base_request();
        request.messages = vec![
            Message::user("inspect the repo"),
            Message::assistant_parts(vec![
                MessagePart::text("working"),
                MessagePart::ToolCall { call },
            ]),
            Message::tool_result(result),
        ];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][1]["type"], json!("message"));
        assert_eq!(body["input"][1]["role"], json!("assistant"));
        assert!(body["input"][1].get("id").is_none());
        assert!(body["input"][1].get("status").is_none());
        assert_eq!(body["input"][1]["content"][0]["type"], json!("output_text"));
        assert_eq!(body["input"][1]["content"][0]["text"], json!("working"));
        assert_eq!(body["input"][2]["type"], json!("function_call"));
        assert_eq!(body["input"][2]["id"], json!("fc_123"));
        assert_eq!(body["input"][2]["call_id"], json!("call_123"));
        assert!(body["input"][2].get("status").is_none());
        let replay_arguments = body["input"][2]["arguments"]
            .as_str()
            .expect("function_call replay arguments should be encoded as a string");
        let parsed_arguments: Value = serde_json::from_str(replay_arguments)
            .expect("replay arguments should stay valid JSON");
        assert_eq!(parsed_arguments, json!({"path":"README.md","line_count":1}));
        assert_eq!(body["input"][3]["type"], json!("function_call_output"));
        assert_eq!(body["input"][3]["call_id"], json!("call_123"));
        assert!(body["input"][3].get("status").is_none());
    }

    #[test]
    fn openai_responses_body_replays_reasoning_items_for_tool_loops() {
        let mut request = base_request();
        request.messages = vec![
            Message::user("inspect the repo"),
            Message::assistant_parts(vec![
                MessagePart::Reasoning {
                    reasoning: Reasoning {
                        id: Some(ReasoningId::from("rs_123")),
                        content: vec![
                            ReasoningContent::Summary("Need to inspect README first.".to_string()),
                            ReasoningContent::Encrypted("enc_123".to_string()),
                        ],
                    },
                },
                MessagePart::ToolCall {
                    call: ToolCall {
                        id: ToolCallId::from("fc_123"),
                        call_id: CallId::from("call_123"),
                        tool_name: ToolName::from("read"),
                        arguments: json!({"path":"README.md"}),
                        origin: ToolOrigin::Local,
                    },
                },
            ]),
            Message::tool_result(
                ToolResult::text("fc_123".into(), "read", "README heading")
                    .with_call_id("call_123"),
            ),
        ];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][1]["type"], json!("reasoning"));
        assert_eq!(body["input"][1]["id"], json!("rs_123"));
        assert_eq!(
            body["input"][1]["summary"],
            json!([{ "type": "summary_text", "text": "Need to inspect README first." }])
        );
        assert_eq!(body["input"][1]["encrypted_content"], json!("enc_123"));
        assert_eq!(body["input"][2]["type"], json!("function_call"));
        assert_eq!(body["input"][3]["type"], json!("function_call_output"));
    }

    #[test]
    fn openai_responses_body_replays_patch_files_freeform_calls_as_custom_calls() {
        let mut request = base_request();
        request.tools.push(
            ToolSpec::function(
                "patch_files",
                "Apply staged patch files",
                json!({"type":"object","properties":{"operations":{"type":"array"}}}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                types::ToolSource::Builtin,
            )
            .with_freeform_format(ToolFreeformFormat::grammar("lark", "start: begin_patch"))
            .with_freeform_availability(types::ToolAvailability {
                provider_allowlist: vec!["openai".to_string()],
                model_allowlist: vec!["gpt-5*".to_string()],
                ..Default::default()
            }),
        );
        let call = ToolCall {
            id: ToolCallId::from("fc_123"),
            call_id: CallId::from("call_123"),
            tool_name: ToolName::from("patch_files"),
            arguments: Value::String("*** Begin Patch\n*** End Patch".to_string()),
            origin: ToolOrigin::Local,
        };
        request.messages = vec![
            Message::user("apply the patch"),
            Message::assistant_parts(vec![MessagePart::ToolCall { call }]),
            Message::tool_result(
                ToolResult::text("fc_123".into(), "patch_files", "ok").with_call_id("call_123"),
            ),
        ];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][1]["type"], json!("custom_tool_call"));
        assert_eq!(body["input"][1]["call_id"], json!("call_123"));
        assert_eq!(body["input"][1]["name"], json!("patch_files"));
        assert_eq!(
            body["input"][1]["input"],
            json!("*** Begin Patch\n*** End Patch")
        );
        assert_eq!(body["input"][2]["type"], json!("custom_tool_call_output"));
        assert_eq!(body["input"][2]["call_id"], json!("call_123"));
    }

    #[test]
    fn openai_responses_body_serializes_remote_image_parts_as_input_images() {
        let mut request = base_request();
        request.messages = vec![Message::new(
            types::MessageRole::User,
            vec![
                MessagePart::image_url("https://example.com/failure.png"),
                MessagePart::text("Describe the failure"),
            ],
        )];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][0]["type"], json!("message"));
        assert_eq!(body["input"][0]["content"][0]["type"], json!("input_image"));
        assert_eq!(
            body["input"][0]["content"][0]["image_url"],
            json!("https://example.com/failure.png")
        );
        assert_eq!(body["input"][0]["content"][1]["type"], json!("input_text"));
    }

    #[test]
    fn openai_responses_body_serializes_reference_parts_as_input_text() {
        let mut request = base_request();
        request.messages = vec![Message::new(
            types::MessageRole::User,
            vec![MessagePart::reference(
                "mention",
                Some("workspace".to_string()),
                Some("app://workspace/snapshot".to_string()),
                None,
            )],
        )];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][0]["content"][0]["type"], json!("input_text"));
        assert_eq!(body["input"][0]["content"][0]["text"], json!("workspace"));
    }

    #[test]
    fn openai_responses_body_keeps_local_file_parts_as_inline_input_files() {
        let mut request = base_request();
        request.messages = vec![Message::new(
            types::MessageRole::User,
            vec![MessagePart::File {
                file_name: Some("report.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                data_base64: Some("cGRm".to_string()),
                uri: Some("docs/report.pdf".to_string()),
            }],
        )];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][0]["type"], json!("message"));
        assert_eq!(body["input"][0]["content"][0]["type"], json!("input_file"));
        assert_eq!(
            body["input"][0]["content"][0]["file_data"],
            json!("data:application/pdf;base64,cGRm")
        );
        assert!(body["input"][0]["content"][0].get("file_url").is_none());
    }

    #[test]
    fn openai_responses_body_serializes_remote_file_parts_as_file_urls() {
        let mut request = base_request();
        request.messages = vec![Message::new(
            types::MessageRole::User,
            vec![MessagePart::File {
                file_name: Some("report.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                data_base64: None,
                uri: Some("https://example.com/docs/report.pdf".to_string()),
            }],
        )];

        let body =
            build_openai_responses_body("gpt-5.4".to_string(), request, &RequestOptions::default())
                .unwrap();

        assert_eq!(body["input"][0]["content"][0]["type"], json!("input_file"));
        assert_eq!(
            body["input"][0]["content"][0]["file_url"],
            json!("https://example.com/docs/report.pdf")
        );
        assert!(body["input"][0]["content"][0].get("file_data").is_none());
    }

    #[test]
    fn previous_response_not_found_maps_to_continuation_loss() {
        let error = classify_openai_error(
            404,
            r#"{"error":{"code":"previous_response_not_found","message":"expired"}}"#,
            None,
        )
        .unwrap();

        assert!(matches!(
            error,
            runtime::RuntimeError::AgentCore(AgentCoreError::ProviderContinuationLost(message))
                if message == "expired"
        ));
    }

    #[test]
    fn retryable_openai_status_preserves_retry_metadata() {
        let error = classify_openai_error(
            429,
            r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#,
            Some(Duration::from_secs(5)),
        )
        .unwrap();

        assert_eq!(error.model_backend_status_code(), Some(429));
        assert_eq!(
            error.model_backend_retry_hint(),
            Some(runtime::ModelBackendRetryHint {
                status_code: 429,
                retry_after: Some(Duration::from_secs(5)),
            })
        );
    }

    #[test]
    fn realtime_request_event_uses_response_create_envelope() {
        let mut request = base_request();
        request.additional_context = vec!["hook additional context".to_string()];
        let event = build_openai_realtime_request_event(
            "gpt-realtime".to_string(),
            request,
            &RequestOptions::default(),
        )
        .unwrap();

        assert_eq!(
            event.get("type"),
            Some(&Value::String("response.create".to_string()))
        );
        assert_eq!(
            event
                .get("response")
                .and_then(|response| response.get("model")),
            Some(&Value::String("gpt-realtime".to_string()))
        );
        assert_eq!(
            event
                .get("response")
                .and_then(|response| response.get("modalities")),
            Some(&Value::Array(vec![Value::String("text".to_string())]))
        );
        assert_eq!(
            event
                .get("response")
                .and_then(|response| response.get("instructions")),
            Some(&Value::String(
                "You are a coding agent.\n\nhook additional context".to_string()
            ))
        );
    }

    #[test]
    fn realtime_request_event_projects_dual_transport_patch_files_as_custom_tools_on_gpt5() {
        let mut request = base_request();
        request.tools.push(
            ToolSpec::function(
                "patch_files",
                "Apply staged patch files",
                json!({"type":"object","properties":{"operations":{"type":"array"}}}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                types::ToolSource::Builtin,
            )
            .with_freeform_format(ToolFreeformFormat::grammar("lark", "start: begin_patch"))
            .with_freeform_availability(types::ToolAvailability {
                provider_allowlist: vec!["openai".to_string()],
                model_allowlist: vec!["gpt-5*".to_string()],
                ..Default::default()
            }),
        );

        let event = build_openai_realtime_request_event(
            "gpt-5.4".to_string(),
            request,
            &RequestOptions::default(),
        )
        .unwrap();

        assert_eq!(event["response"]["tools"][0]["type"], json!("custom"));
        assert_eq!(event["response"]["tools"][0]["name"], json!("patch_files"));
        assert_eq!(
            event["response"]["tools"][0]["format"]["type"],
            json!("grammar")
        );
    }

    #[tokio::test]
    async fn realtime_stream_emits_tool_calls_and_continuation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();

            let request_frame = websocket.next().await.unwrap().unwrap();
            let request_text = match request_frame {
                WsMessage::Text(text) => text.to_string(),
                other => panic!("expected text websocket frame, got {other:?}"),
            };
            let request_json: Value = serde_json::from_str(&request_text).unwrap();
            assert_eq!(request_json["type"], json!("response.create"));

            websocket
                .send(WsMessage::Text(
                    json!({"type":"response.output_text.delta","delta":"hel"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            websocket
                .send(WsMessage::Text(
                    json!({
                        "type":"response.output_item.done",
                        "item":{
                            "type":"function_call",
                            "id":"fc_ws_1",
                            "call_id":"call_ws_1",
                            "name":"read",
                            "arguments":"{\"path\":\"README.md\"}"
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            websocket
                .send(WsMessage::Text(
                    json!({
                        "type":"response.done",
                        "response":{
                            "id":"resp_ws_1",
                            "status":"completed",
                            "output":[{"type":"message","id":"msg_ws_1"}]
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let stream = stream_openai_realtime_turn(
            super::OpenAiTransport {
                api_key: "test-key".to_string(),
                base_url: format!("http://{addr}/v1"),
                http_client: reqwest::Client::new(),
            },
            "gpt-realtime".to_string(),
            base_request(),
            RequestOptions {
                openai_transport: Some(OpenAiTransportMode::RealtimeWebSocket),
                ..RequestOptions::default()
            },
        )
        .await
        .unwrap();

        let events = stream.collect::<Vec<_>>().await;
        server.await.unwrap();

        assert!(matches!(
            &events[0],
            Ok(ModelEvent::TextDelta { delta }) if delta == "hel"
        ));
        assert!(matches!(
            &events[1],
            Ok(ModelEvent::ToolCallRequested { call })
                if call.tool_name == ToolName::from("read") && call.call_id.as_str() == "call_ws_1"
        ));
        assert!(matches!(
            events.last(),
            Some(Ok(ModelEvent::ResponseComplete {
                message_id: Some(message_id),
                continuation: Some(ProviderContinuation::OpenAiResponses { response_id }),
                ..
            })) if message_id.as_str() == "msg_ws_1" && response_id.as_str() == "resp_ws_1"
        ));
    }

    #[tokio::test]
    async fn openai_stream_emits_tool_calls_and_continuation() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hel\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"README.md\\\"}\"}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"id\":\"msg_1\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":120,\"output_tokens\":30,\"input_tokens_details\":{\"cached_tokens\":20},\"output_tokens_details\":{\"reasoning_tokens\":12}}}}\n\n",
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
                if call.tool_name == ToolName::from("read") && call.call_id.as_str() == "call_1"
        ));
        assert!(matches!(
            events.last(),
            Some(Ok(ModelEvent::ResponseComplete {
                message_id: Some(message_id),
                continuation: Some(ProviderContinuation::OpenAiResponses { response_id }),
                usage: Some(usage),
                ..
            })) if message_id.as_str() == "msg_1"
                && response_id.as_str() == "resp_1"
                && *usage == TokenUsage {
                    reasoning_tokens: 12,
                    ..TokenUsage::from_input_output(120, 30, 20)
                }
                && usage.prefix_cache_hit_rate_basis_points() == Some(1667)
        ));
    }

    #[tokio::test]
    async fn openai_stream_surfaces_retryable_transient_502_before_streaming() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_http_request(&mut stream).await;

            let body = r#"{"error":{"code":"server_error","message":"temporary upstream issue"}}"#;
            let response = format!(
                "HTTP/1.1 502 Bad Gateway\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
        });

        let stream = stream_openai_responses_turn(
            super::OpenAiTransport {
                api_key: "test-key".to_string(),
                base_url: format!("http://{addr}"),
                http_client: reqwest::Client::new(),
            },
            "gpt-5.4".to_string(),
            base_request(),
            RequestOptions::default(),
        )
        .await
        .unwrap();
        let events = stream.collect::<Vec<_>>().await;
        server.await.unwrap();

        let error = match events.as_slice() {
            [Err(error)] => error,
            other => panic!("expected one retryable provider error event, got {other:?}"),
        };
        assert_eq!(error.model_backend_status_code(), Some(502));
        assert_eq!(
            error.model_backend_retry_hint(),
            Some(runtime::ModelBackendRetryHint {
                status_code: 502,
                retry_after: None,
            })
        );
    }
}
