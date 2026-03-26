use crate::{
    OpenAiTransport, Result, RigError, RigOpenAiResponsesOptions, RigProviderCapabilities,
    RigProviderDescriptor, RigProviderKind, build_openai_transport, openai_capabilities,
    stream_openai_responses_turn, to_rig_tool_definition,
};
use agent_core_runtime::{
    ModelBackend, ModelBackendCapabilities, Result as RuntimeResult, RuntimeError,
};
use agent_core_types::{
    AgentCoreError, Message, MessagePart, MessageRole, ModelEvent, ModelRequest, Reasoning,
    ReasoningContent as AgentReasoningContent, ToolCall, ToolCallId, ToolOrigin, new_opaque_id,
};
use async_trait::async_trait;
use futures::{Stream, stream::BoxStream};
use rig::OneOrMany;
use rig::client::{CompletionClient, ProviderClient as RigProviderClientTrait};
use rig::completion::{CompletionModel, CompletionRequest, GetTokenUsage};
use rig::message::{
    AssistantContent, DocumentMediaType, ImageMediaType, Message as RigMessage, MimeType,
    Reasoning as RigReasoning, ReasoningContent as RigReasoningContent,
    ToolResultContent as RigToolResultContent, UserContent,
};
use rig::providers::anthropic;
use rig::streaming::{StreamedAssistantContent, StreamingCompletionResponse};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug)]
pub struct RigBackendDescriptor {
    pub provider: RigProviderDescriptor,
    pub capabilities: RigProviderCapabilities,
}

impl RigBackendDescriptor {
    #[must_use]
    pub fn new(provider: RigProviderDescriptor) -> Self {
        Self {
            provider,
            capabilities: RigProviderCapabilities::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RigRequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub additional_params: Option<Value>,
    pub prompt_cache_key: Option<String>,
    pub prompt_cache_retention: Option<RigPromptCacheRetention>,
    pub openai_responses: Option<RigOpenAiResponsesOptions>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigPromptCacheRetention {
    InMemory,
    Hours24,
}

impl RigPromptCacheRetention {
    pub(crate) fn as_api_value(self) -> &'static str {
        match self {
            Self::InMemory => "in_memory",
            Self::Hours24 => "24h",
        }
    }
}

#[derive(Clone)]
enum RigProviderClient {
    OpenAi(OpenAiTransport),
    Anthropic(anthropic::Client),
}

pub struct RigModelBackend {
    descriptor: RigBackendDescriptor,
    client: RigProviderClient,
    request_options: RigRequestOptions,
}

impl RigModelBackend {
    pub fn new(provider: RigProviderDescriptor) -> Result<Self> {
        Self::from_settings(
            RigBackendDescriptor::new(provider),
            RigRequestOptions::default(),
            None,
        )
    }

    pub fn from_descriptor(descriptor: RigBackendDescriptor) -> Result<Self> {
        Self::from_settings(descriptor, RigRequestOptions::default(), None)
    }

    pub fn from_settings(
        descriptor: RigBackendDescriptor,
        request_options: RigRequestOptions,
        base_url: Option<String>,
    ) -> Result<Self> {
        Self::from_settings_with_api_key(descriptor, request_options, base_url, None)
    }

    pub fn from_settings_with_api_key(
        mut descriptor: RigBackendDescriptor,
        request_options: RigRequestOptions,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self> {
        if matches!(descriptor.provider.provider, RigProviderKind::OpenAi) {
            let capabilities = openai_capabilities(&request_options);
            descriptor.capabilities.provider_managed_history =
                capabilities.provider_managed_history;
            descriptor.capabilities.provider_native_compaction =
                capabilities.provider_native_compaction;
        }
        let client = match descriptor.provider.provider {
            RigProviderKind::OpenAi => RigProviderClient::OpenAi(build_openai_transport(
                base_url.as_deref(),
                api_key.as_deref(),
            )?),
            RigProviderKind::Anthropic => RigProviderClient::Anthropic(build_anthropic_client(
                base_url.as_deref(),
                api_key.as_deref(),
            )?),
        };
        Ok(Self {
            descriptor,
            client,
            request_options,
        })
    }

    #[must_use]
    pub fn descriptor(&self) -> &RigBackendDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn request_options(&self) -> &RigRequestOptions {
        &self.request_options
    }
}

#[async_trait]
impl ModelBackend for RigModelBackend {
    fn capabilities(&self) -> ModelBackendCapabilities {
        match self.descriptor.provider.provider {
            RigProviderKind::OpenAi => openai_capabilities(&self.request_options),
            RigProviderKind::Anthropic => ModelBackendCapabilities::default(),
        }
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        let stream = match &self.client {
            RigProviderClient::OpenAi(transport) => {
                return stream_openai_responses_turn(
                    transport.clone(),
                    self.descriptor.provider.model.clone(),
                    request,
                    self.request_options.clone(),
                )
                .await;
            }
            RigProviderClient::Anthropic(client) => {
                let tool_origins = request
                    .tools
                    .iter()
                    .map(|tool| (tool.name.clone(), tool.origin.clone()))
                    .collect::<BTreeMap<_, _>>();
                let request = to_completion_request(
                    request,
                    &self.request_options,
                    &self.descriptor.provider.provider,
                )
                .map_err(RuntimeError::from)?;
                stream_completion_to_model_events(
                    execute_streaming_completion(
                        client.completion_model(self.descriptor.provider.model.clone()),
                        request,
                    )
                    .await
                    .map_err(RuntimeError::from)?,
                    tool_origins,
                )
            }
        };
        Ok(stream)
    }
}

async fn execute_streaming_completion<M>(
    model: M,
    request: CompletionRequest,
) -> Result<StreamingCompletionResponse<M::StreamingResponse>>
where
    M: CompletionModel,
{
    model
        .stream(request)
        .await
        .map_err(|error| RigError::provider(error.to_string()))
}

fn build_anthropic_client(
    base_url: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<anthropic::Client> {
    let base_url = base_url
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok());
    if let Some(api_key) = api_key_override {
        let mut builder = anthropic::Client::builder().api_key(api_key);
        if let Some(base_url) = base_url.as_deref() {
            builder = builder.base_url(base_url);
        }
        return builder
            .build()
            .map_err(|error| RigError::config(error.to_string()));
    }
    if let Some(base_url) = base_url.as_deref() {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| RigError::config("ANTHROPIC_API_KEY not set"))?;
        return anthropic::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|error| RigError::config(error.to_string()));
    }
    Ok(anthropic::Client::from_env())
}

pub(crate) fn to_completion_request(
    request: ModelRequest,
    request_options: &RigRequestOptions,
    provider_kind: &RigProviderKind,
) -> Result<CompletionRequest> {
    let mut messages = request
        .instructions
        .into_iter()
        .map(RigMessage::system)
        .collect::<Vec<_>>();
    messages.extend(
        request
            .additional_context
            .into_iter()
            .map(RigMessage::system),
    );
    for message in request.messages {
        messages.push(to_rig_message(message)?);
    }
    if messages.is_empty() {
        return Err(RigError::protocol(
            "model request did not contain any messages",
        ));
    }

    Ok(CompletionRequest {
        model: None,
        preamble: None,
        chat_history: OneOrMany::many(messages).map_err(|error| {
            RigError::protocol(format!("failed to build chat history: {error}"))
        })?,
        documents: Vec::new(),
        tools: request
            .tools
            .iter()
            .map(to_rig_tool_definition)
            .collect::<Vec<_>>(),
        temperature: request_options.temperature,
        max_tokens: request_options.max_tokens,
        tool_choice: None,
        additional_params: build_additional_params(request_options, provider_kind)?,
        output_schema: None,
    })
}

fn build_additional_params(
    request_options: &RigRequestOptions,
    provider_kind: &RigProviderKind,
) -> Result<Option<Value>> {
    let mut additional_params = request_options.additional_params.clone();
    if !matches!(provider_kind, RigProviderKind::OpenAi) {
        return Ok(additional_params);
    }
    if request_options.prompt_cache_key.is_none()
        && request_options.prompt_cache_retention.is_none()
    {
        return Ok(additional_params);
    }

    let mut object = match additional_params.take() {
        Some(Value::Object(object)) => object,
        Some(_) => {
            return Err(RigError::config(
                "OpenAI prompt cache controls require additional_params to be a JSON object",
            ));
        }
        None => Map::new(),
    };
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
    Ok(Some(Value::Object(object)))
}

fn to_rig_message(message: Message) -> Result<RigMessage> {
    match message.role {
        MessageRole::System => Ok(RigMessage::system(message.text_content())),
        MessageRole::User => {
            let content = to_user_content(message.parts)?;
            Ok(RigMessage::User {
                content: OneOrMany::many(content).map_err(|error| {
                    RigError::protocol(format!("failed to build user message: {error}"))
                })?,
            })
        }
        MessageRole::Assistant => {
            let content = to_assistant_content(message.parts)?;
            Ok(RigMessage::Assistant {
                id: Some(message.message_id),
                content: OneOrMany::many(content).map_err(|error| {
                    RigError::protocol(format!("failed to build assistant message: {error}"))
                })?,
            })
        }
        MessageRole::Tool => {
            let content = to_tool_result_content(message.parts)?;
            Ok(RigMessage::User {
                content: OneOrMany::many(content).map_err(|error| {
                    RigError::protocol(format!("failed to build tool result message: {error}"))
                })?,
            })
        }
    }
}

fn to_user_content(parts: Vec<MessagePart>) -> Result<Vec<UserContent>> {
    let mut content = Vec::new();
    for part in parts {
        match part {
            MessagePart::Text { text } => content.push(UserContent::text(text)),
            MessagePart::Image {
                mime_type,
                data_base64,
            } => content.push(UserContent::image_base64(
                data_base64,
                ImageMediaType::from_mime_type(&mime_type),
                None,
            )),
            MessagePart::File {
                mime_type,
                data_base64,
                uri,
                ..
            } => {
                let media_type = mime_type
                    .as_deref()
                    .and_then(DocumentMediaType::from_mime_type);
                if let Some(uri) = uri {
                    content.push(UserContent::document_url(uri, media_type));
                } else if let Some(data_base64) = data_base64 {
                    content.push(UserContent::document(data_base64, media_type));
                }
            }
            MessagePart::ToolResult { result } => {
                let converted = to_rig_tool_result_parts(result.parts)?;
                let converted = OneOrMany::many(converted).map_err(|error| {
                    RigError::protocol(format!("failed to build tool result content: {error}"))
                })?;
                content.push(UserContent::tool_result_with_call_id(
                    result.id.0,
                    result.call_id,
                    converted,
                ));
            }
            MessagePart::Reasoning { reasoning } => {
                let text = reasoning.display_text();
                if !text.is_empty() {
                    content.push(UserContent::text(text));
                }
            }
            MessagePart::Resource { uri, text, .. } => {
                content.push(UserContent::document(
                    text.unwrap_or(uri),
                    Some(DocumentMediaType::TXT),
                ));
            }
            MessagePart::Json { value } => content.push(UserContent::text(value.to_string())),
            MessagePart::ProviderExtension { payload, .. } => {
                content.push(UserContent::text(payload.to_string()));
            }
            MessagePart::ToolCall { .. } => {}
        }
    }

    if content.is_empty() {
        return Err(RigError::protocol(
            "message did not contain any user-compatible content",
        ));
    }
    Ok(content)
}

fn to_assistant_content(parts: Vec<MessagePart>) -> Result<Vec<AssistantContent>> {
    let mut content = Vec::new();
    for part in parts {
        match part {
            MessagePart::Text { text } => content.push(AssistantContent::text(text)),
            MessagePart::ToolCall { call } => {
                content.push(AssistantContent::tool_call_with_call_id(
                    call.id.0,
                    call.call_id,
                    call.tool_name,
                    call.arguments,
                ))
            }
            MessagePart::Reasoning { reasoning } => {
                content.push(AssistantContent::Reasoning(to_rig_reasoning(reasoning)))
            }
            MessagePart::Image {
                mime_type,
                data_base64,
            } => content.push(AssistantContent::image_base64(
                data_base64,
                ImageMediaType::from_mime_type(&mime_type),
                None,
            )),
            MessagePart::Resource { uri, text, .. } => {
                content.push(AssistantContent::text(text.unwrap_or(uri)));
            }
            MessagePart::Json { value } => content.push(AssistantContent::text(value.to_string())),
            MessagePart::ProviderExtension { payload, .. } => {
                content.push(AssistantContent::text(payload.to_string()));
            }
            MessagePart::File {
                uri, data_base64, ..
            } => {
                let summary = uri.or(data_base64).unwrap_or_default();
                if !summary.is_empty() {
                    content.push(AssistantContent::text(summary));
                }
            }
            MessagePart::ToolResult { result } => {
                content.push(AssistantContent::text(result.text_content()));
            }
        }
    }

    if content.is_empty() {
        return Err(RigError::protocol(
            "message did not contain any assistant-compatible content",
        ));
    }
    Ok(content)
}

fn to_tool_result_content(parts: Vec<MessagePart>) -> Result<Vec<UserContent>> {
    let mut content = Vec::new();
    for part in parts {
        match part {
            MessagePart::ToolResult { result } => {
                let converted = to_rig_tool_result_parts(result.parts)?;
                let converted = OneOrMany::many(converted).map_err(|error| {
                    RigError::protocol(format!("failed to build user tool result content: {error}"))
                })?;
                content.push(UserContent::tool_result_with_call_id(
                    result.id.0,
                    result.call_id,
                    converted,
                ));
            }
            MessagePart::Reasoning { reasoning } => {
                let text = reasoning.display_text();
                if !text.is_empty() {
                    content.push(UserContent::text(text));
                }
            }
            MessagePart::Text { text } => content.push(UserContent::text(text)),
            MessagePart::Resource { uri, text, .. } => {
                content.push(UserContent::document(
                    text.unwrap_or(uri),
                    Some(DocumentMediaType::TXT),
                ));
            }
            MessagePart::Json { value } => content.push(UserContent::text(value.to_string())),
            MessagePart::ProviderExtension { payload, .. } => {
                content.push(UserContent::text(payload.to_string()));
            }
            MessagePart::Image {
                mime_type,
                data_base64,
            } => content.push(UserContent::image_base64(
                data_base64,
                ImageMediaType::from_mime_type(&mime_type),
                None,
            )),
            MessagePart::File {
                mime_type,
                data_base64,
                uri,
                ..
            } => {
                let media_type = mime_type
                    .as_deref()
                    .and_then(DocumentMediaType::from_mime_type);
                if let Some(uri) = uri {
                    content.push(UserContent::document_url(uri, media_type));
                } else if let Some(data_base64) = data_base64 {
                    content.push(UserContent::document(data_base64, media_type));
                }
            }
            MessagePart::ToolCall { .. } => {}
        }
    }

    if content.is_empty() {
        return Err(RigError::protocol(
            "tool message did not contain any tool-compatible content",
        ));
    }
    Ok(content)
}

fn to_rig_tool_result_parts(parts: Vec<MessagePart>) -> Result<Vec<RigToolResultContent>> {
    let mut content = Vec::new();
    for part in parts {
        match part {
            MessagePart::Text { text } => content.push(RigToolResultContent::text(text)),
            MessagePart::Image {
                mime_type,
                data_base64,
            } => content.push(RigToolResultContent::image_base64(
                data_base64,
                ImageMediaType::from_mime_type(&mime_type),
                None,
            )),
            MessagePart::Resource { uri, text, .. } => {
                content.push(RigToolResultContent::text(text.unwrap_or(uri)));
            }
            MessagePart::Json { value } => {
                content.push(RigToolResultContent::text(value.to_string()))
            }
            MessagePart::ProviderExtension { payload, .. } => {
                content.push(RigToolResultContent::text(payload.to_string()))
            }
            MessagePart::Reasoning { reasoning } => {
                let text = reasoning.display_text();
                if !text.is_empty() {
                    content.push(RigToolResultContent::text(text));
                }
            }
            MessagePart::File {
                uri, data_base64, ..
            } => {
                let summary = uri.or(data_base64).unwrap_or_default();
                if !summary.is_empty() {
                    content.push(RigToolResultContent::text(summary));
                }
            }
            MessagePart::ToolResult { result } => {
                content.push(RigToolResultContent::text(result.text_content()));
            }
            MessagePart::ToolCall { call } => {
                content.push(RigToolResultContent::text(call.arguments.to_string()));
            }
        }
    }

    if content.is_empty() {
        return Err(RigError::protocol(
            "tool result did not contain any MCP-compatible content",
        ));
    }
    Ok(content)
}

#[cfg(test)]
fn response_to_model_events(
    choice: OneOrMany<AssistantContent>,
    tool_origins: BTreeMap<String, ToolOrigin>,
) -> Vec<ModelEvent> {
    let mut assistant_text = String::new();
    let mut events = Vec::new();
    let reasoning = extract_reasoning(choice.iter());

    for content in choice.iter().cloned() {
        match content {
            AssistantContent::Text(text) => {
                if !assistant_text.is_empty() {
                    assistant_text.push('\n');
                }
                assistant_text.push_str(text.text());
            }
            AssistantContent::ToolCall(call) => {
                let tool_name = call.function.name.clone();
                events.push(ModelEvent::ToolCallRequested {
                    call: ToolCall {
                        id: ToolCallId(call.id),
                        call_id: call.call_id.unwrap_or_else(new_opaque_id),
                        tool_name: tool_name.clone(),
                        arguments: call.function.arguments,
                        origin: tool_origins.get(&tool_name).cloned().unwrap_or_else(|| {
                            ToolOrigin::Provider {
                                provider: "rig".to_string(),
                            }
                        }),
                    },
                });
            }
            AssistantContent::Reasoning(_) | AssistantContent::Image(_) => {}
        }
    }

    if !assistant_text.is_empty() {
        events.insert(
            0,
            ModelEvent::TextDelta {
                delta: assistant_text,
            },
        );
    }

    events.push(ModelEvent::ResponseComplete {
        stop_reason: Some(
            if events
                .iter()
                .any(|event| matches!(event, ModelEvent::ToolCallRequested { .. }))
            {
                "tool_use"
            } else {
                "stop"
            }
            .to_string(),
        ),
        message_id: None,
        continuation: None,
        reasoning,
    });
    events
}

fn stream_completion_to_model_events<R>(
    inner: StreamingCompletionResponse<R>,
    tool_origins: BTreeMap<String, ToolOrigin>,
) -> BoxStream<'static, RuntimeResult<ModelEvent>>
where
    R: Clone + Unpin + GetTokenUsage + Send + 'static,
{
    Box::pin(RigStreamingModelEventStream {
        inner,
        tool_origins,
        pending: VecDeque::new(),
        saw_tool_call: false,
        finished: false,
    })
}

struct RigStreamingModelEventStream<R>
where
    R: Clone + Unpin + GetTokenUsage,
{
    inner: StreamingCompletionResponse<R>,
    tool_origins: BTreeMap<String, ToolOrigin>,
    pending: VecDeque<ModelEvent>,
    saw_tool_call: bool,
    finished: bool,
}

impl<R> Stream for RigStreamingModelEventStream<R>
where
    R: Clone + Unpin + GetTokenUsage + Send + 'static,
{
    type Item = RuntimeResult<ModelEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();

        loop {
            if let Some(event) = stream.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }

            match Pin::new(&mut stream.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    if stream.finished {
                        return Poll::Ready(None);
                    }
                    stream.finished = true;
                    return Poll::Ready(Some(Ok(ModelEvent::ResponseComplete {
                        stop_reason: Some(
                            if stream.saw_tool_call {
                                "tool_use"
                            } else {
                                "stop"
                            }
                            .to_string(),
                        ),
                        message_id: stream.inner.message_id.clone(),
                        continuation: None,
                        reasoning: extract_reasoning(stream.inner.choice.iter()),
                    })));
                }
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(
                        AgentCoreError::ModelBackend(error.to_string()).into()
                    )));
                }
                Poll::Ready(Some(Ok(chunk))) => match chunk {
                    StreamedAssistantContent::Text(text) => {
                        stream
                            .pending
                            .push_back(ModelEvent::TextDelta { delta: text.text });
                    }
                    StreamedAssistantContent::ToolCall { tool_call, .. } => {
                        let tool_name = tool_call.function.name.clone();
                        stream.saw_tool_call = true;
                        stream.pending.push_back(ModelEvent::ToolCallRequested {
                            call: ToolCall {
                                id: ToolCallId(tool_call.id),
                                call_id: tool_call.call_id.unwrap_or_else(new_opaque_id),
                                tool_name: tool_name.clone(),
                                arguments: tool_call.function.arguments,
                                origin: stream
                                    .tool_origins
                                    .get(&tool_name)
                                    .cloned()
                                    .unwrap_or_else(|| ToolOrigin::Provider {
                                        provider: "rig".to_string(),
                                    }),
                            },
                        });
                    }
                    StreamedAssistantContent::ToolCallDelta { .. }
                    | StreamedAssistantContent::Reasoning(_)
                    | StreamedAssistantContent::ReasoningDelta { .. }
                    | StreamedAssistantContent::Final(_) => {}
                },
            }
        }
    }
}

fn to_rig_reasoning(reasoning: Reasoning) -> RigReasoning {
    let mut converted = RigReasoning::summaries(Vec::new()).optional_id(reasoning.id);
    converted.content = reasoning
        .content
        .into_iter()
        .map(|item| match item {
            AgentReasoningContent::Text { text, signature } => {
                RigReasoningContent::Text { text, signature }
            }
            AgentReasoningContent::Encrypted(value) => RigReasoningContent::Encrypted(value),
            AgentReasoningContent::Redacted { data } => RigReasoningContent::Redacted { data },
            AgentReasoningContent::Summary(value) => RigReasoningContent::Summary(value),
        })
        .collect();
    converted
}

fn from_rig_reasoning(reasoning: &RigReasoning) -> Reasoning {
    Reasoning {
        id: reasoning.id.clone(),
        content: reasoning
            .content
            .iter()
            .map(|item| match item {
                RigReasoningContent::Text { text, signature } => AgentReasoningContent::Text {
                    text: text.clone(),
                    signature: signature.clone(),
                },
                RigReasoningContent::Encrypted(value) => {
                    AgentReasoningContent::Encrypted(value.clone())
                }
                RigReasoningContent::Redacted { data } => {
                    AgentReasoningContent::Redacted { data: data.clone() }
                }
                RigReasoningContent::Summary(value) => {
                    AgentReasoningContent::Summary(value.clone())
                }
                _ => AgentReasoningContent::Redacted {
                    data: "<unsupported reasoning content>".to_string(),
                },
            })
            .collect(),
    }
}

fn extract_reasoning<'a>(choice: impl IntoIterator<Item = &'a AssistantContent>) -> Vec<Reasoning> {
    choice
        .into_iter()
        .filter_map(|content| match content {
            AssistantContent::Reasoning(reasoning) => Some(from_rig_reasoning(reasoning)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        RigPromptCacheRetention, RigRequestOptions, response_to_model_events,
        to_completion_request, to_rig_message,
    };
    use crate::RigProviderKind;
    use agent_core_types::{
        Message, MessagePart, ModelEvent, ModelRequest, RunId, SessionId, ToolCall, ToolCallId,
        ToolOrigin, ToolOutputMode, ToolResult, ToolSpec, TurnId,
    };
    use rig::OneOrMany;
    use rig::completion::{AssistantContent, Message as RigMessage};
    use rig::message::{DocumentSourceKind, ToolResultContent, UserContent};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn completion_request_includes_system_messages_and_tools() {
        let request = ModelRequest {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec!["Keep it short".to_string()],
            messages: vec![
                Message::user("read file"),
                Message::assistant_parts(vec![MessagePart::ToolCall {
                    call: ToolCall {
                        id: ToolCallId("tool-1".to_string()),
                        call_id: "call_tool_1".to_string(),
                        tool_name: "read".to_string(),
                        arguments: json!({"path":"README.md"}),
                        origin: ToolOrigin::Local,
                    },
                }]),
                Message::tool_result(
                    ToolResult::text(ToolCallId("tool-1".to_string()), "read", "hello")
                        .with_call_id("call_tool_1"),
                ),
            ],
            tools: vec![ToolSpec {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
                output_mode: ToolOutputMode::Text,
                origin: ToolOrigin::Local,
                annotations: BTreeMap::new(),
            }],
            additional_context: vec!["repo: nanoclaw".to_string()],
            continuation: None,
            metadata: json!({}),
        };

        let converted = to_completion_request(
            request,
            &RigRequestOptions {
                additional_params: Some(json!({"metadata":{"tier":"priority"}})),
                ..RigRequestOptions::default()
            },
            &RigProviderKind::OpenAi,
        )
        .unwrap();
        assert_eq!(converted.tools.len(), 1);
        assert_eq!(converted.chat_history.len(), 5);
        assert_eq!(
            converted.additional_params,
            Some(json!({"metadata":{"tier":"priority"}}))
        );
    }

    #[test]
    fn completion_request_includes_openai_prompt_cache_controls() {
        let request = ModelRequest {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec!["Keep it short".to_string()],
            messages: vec![Message::user("inspect the workspace")],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        };

        let converted = to_completion_request(
            request,
            &RigRequestOptions {
                additional_params: Some(json!({"metadata":{"tier":"priority"}})),
                prompt_cache_key: Some("workspace:main".to_string()),
                prompt_cache_retention: Some(RigPromptCacheRetention::Hours24),
                ..RigRequestOptions::default()
            },
            &RigProviderKind::OpenAi,
        )
        .unwrap();

        assert_eq!(
            converted.additional_params,
            Some(json!({
                "metadata": {"tier":"priority"},
                "prompt_cache_key": "workspace:main",
                "prompt_cache_retention": "24h"
            }))
        );
    }

    #[test]
    fn completion_request_omits_openai_prompt_cache_controls_for_anthropic() {
        let request = ModelRequest {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec!["Keep it short".to_string()],
            messages: vec![Message::user("inspect the workspace")],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        };

        let converted = to_completion_request(
            request,
            &RigRequestOptions {
                prompt_cache_key: Some("workspace:main".to_string()),
                prompt_cache_retention: Some(RigPromptCacheRetention::Hours24),
                ..RigRequestOptions::default()
            },
            &RigProviderKind::Anthropic,
        )
        .unwrap();

        assert_eq!(converted.additional_params, None);
    }

    #[test]
    fn completion_request_rejects_non_object_additional_params_when_prompt_cache_controls_are_set()
    {
        let request = ModelRequest {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            instructions: vec!["Keep it short".to_string()],
            messages: vec![Message::user("inspect the workspace")],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({}),
        };

        let error = to_completion_request(
            request,
            &RigRequestOptions {
                additional_params: Some(json!(["invalid"])),
                prompt_cache_key: Some("workspace:main".to_string()),
                ..RigRequestOptions::default()
            },
            &RigProviderKind::OpenAi,
        )
        .unwrap_err();

        assert!(error.to_string().contains("prompt cache controls"));
    }

    #[test]
    fn response_events_preserve_tool_origin() {
        let choice = OneOrMany::many(vec![
            AssistantContent::text("first"),
            AssistantContent::tool_call("provider-id", "grep", json!({"pattern":"todo"})),
        ])
        .unwrap();

        let events = response_to_model_events(
            choice,
            [(
                "grep".to_string(),
                ToolOrigin::Mcp {
                    server_name: "fs".to_string(),
                },
            )]
            .into_iter()
            .collect(),
        );

        assert!(matches!(events[0], ModelEvent::TextDelta { .. }));
        assert!(matches!(
            events[1],
            ModelEvent::ToolCallRequested {
                call: ToolCall {
                    origin: ToolOrigin::Mcp { .. },
                    ..
                }
            }
        ));
    }

    #[test]
    fn response_events_default_missing_tool_origin_to_provider() {
        let choice = OneOrMany::many(vec![AssistantContent::tool_call(
            "provider-id",
            "grep",
            json!({}),
        )])
        .unwrap();

        let events = response_to_model_events(choice, BTreeMap::new());

        assert!(matches!(
            &events[0],
            ModelEvent::ToolCallRequested {
                call: ToolCall {
                    origin: ToolOrigin::Provider { provider },
                    ..
                }
            } if provider == "rig"
        ));
        assert!(matches!(
            events.last(),
            Some(ModelEvent::ResponseComplete { stop_reason: Some(reason), .. }) if reason == "tool_use"
        ));
    }

    #[test]
    fn response_events_preserve_call_id_when_present() {
        let choice = OneOrMany::many(vec![AssistantContent::tool_call_with_call_id(
            "provider-id",
            "call_123".to_string(),
            "grep",
            json!({"pattern":"todo"}),
        )])
        .unwrap();

        let events = response_to_model_events(choice, BTreeMap::new());

        assert!(matches!(
            &events[0],
            ModelEvent::ToolCallRequested {
                call: ToolCall {
                    id,
                    call_id,
                    ..
                }
            } if id.0 == "provider-id" && call_id == "call_123"
        ));
    }

    #[test]
    fn user_message_maps_rich_parts_to_rig_user_content() {
        let message = Message::new(
            agent_core_types::MessageRole::User,
            vec![
                MessagePart::text("hello"),
                MessagePart::Resource {
                    uri: "fixture://guide".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    text: Some("guide text".to_string()),
                    metadata: Some(json!({"scope":"fixture"})),
                },
                MessagePart::Json {
                    value: json!({"kind":"note"}),
                },
                MessagePart::ProviderExtension {
                    provider: "fixture".to_string(),
                    kind: "annotation".to_string(),
                    payload: json!({"level":"debug"}),
                },
                MessagePart::File {
                    file_name: Some("report.txt".to_string()),
                    mime_type: Some("text/plain".to_string()),
                    data_base64: None,
                    uri: Some("https://example.com/report.txt".to_string()),
                },
                MessagePart::ToolResult {
                    result: ToolResult::text(
                        ToolCallId("tool-1".to_string()),
                        "grep",
                        "match line",
                    ),
                },
            ],
        );

        let converted = to_rig_message(message).unwrap();

        let RigMessage::User { content } = converted else {
            panic!("expected user message");
        };
        let content = content.iter().cloned().collect::<Vec<_>>();
        assert_eq!(content.len(), 6);
        assert!(matches!(&content[0], UserContent::Text(text) if text.text == "hello"));
        assert!(matches!(
            &content[1],
            UserContent::Document(document)
                if document.media_type == Some(rig::message::DocumentMediaType::TXT)
                    && matches!(&document.data, DocumentSourceKind::String(text) if text == "guide text")
        ));
        assert!(
            matches!(&content[2], UserContent::Text(text) if text.text == "{\"kind\":\"note\"}")
        );
        assert!(matches!(
            &content[3],
            UserContent::Text(text) if text.text == "{\"level\":\"debug\"}"
        ));
        assert!(matches!(
            &content[4],
            UserContent::Document(document)
                if document.media_type == Some(rig::message::DocumentMediaType::TXT)
                    && matches!(&document.data, DocumentSourceKind::Url(url) if url == "https://example.com/report.txt")
        ));
        assert!(matches!(
            &content[5],
            UserContent::ToolResult(result)
                if result.id == "tool-1"
                    && matches!(result.content.iter().next(), Some(ToolResultContent::Text(text)) if text.text == "match line")
        ));
    }

    #[test]
    fn assistant_message_maps_rich_parts_to_rig_assistant_content() {
        let message = Message::assistant_parts(vec![
            MessagePart::text("hello"),
            MessagePart::ToolCall {
                call: ToolCall {
                    id: ToolCallId("tool-1".to_string()),
                    call_id: "call_tool_1".to_string(),
                    tool_name: "grep".to_string(),
                    arguments: json!({"pattern":"todo"}),
                    origin: ToolOrigin::Local,
                },
            },
            MessagePart::Resource {
                uri: "fixture://guide".to_string(),
                mime_type: Some("text/plain".to_string()),
                text: Some("guide text".to_string()),
                metadata: None,
            },
            MessagePart::Json {
                value: json!({"kind":"note"}),
            },
            MessagePart::ProviderExtension {
                provider: "fixture".to_string(),
                kind: "annotation".to_string(),
                payload: json!({"level":"debug"}),
            },
            MessagePart::File {
                file_name: Some("report.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                data_base64: None,
                uri: Some("file:///tmp/report.txt".to_string()),
            },
            MessagePart::ToolResult {
                result: ToolResult::text(ToolCallId("tool-2".to_string()), "read", "done"),
            },
        ])
        .with_message_id("msg_123");

        let converted = to_rig_message(message).unwrap();

        let RigMessage::Assistant { id, content } = converted else {
            panic!("expected assistant message");
        };
        assert_eq!(id.as_deref(), Some("msg_123"));
        let content = content.iter().cloned().collect::<Vec<_>>();
        assert_eq!(content.len(), 7);
        assert!(matches!(&content[0], AssistantContent::Text(text) if text.text == "hello"));
        assert!(matches!(
            &content[1],
            AssistantContent::ToolCall(call)
                if call.id == "tool-1"
                    && call.call_id.as_deref() == Some("call_tool_1")
                    && call.function.name == "grep"
                    && call.function.arguments == json!({"pattern":"todo"})
        ));
        assert!(matches!(&content[2], AssistantContent::Text(text) if text.text == "guide text"));
        assert!(
            matches!(&content[3], AssistantContent::Text(text) if text.text == "{\"kind\":\"note\"}")
        );
        assert!(matches!(
            &content[4],
            AssistantContent::Text(text) if text.text == "{\"level\":\"debug\"}"
        ));
        assert!(matches!(
            &content[5],
            AssistantContent::Text(text) if text.text == "file:///tmp/report.txt"
        ));
        assert!(matches!(&content[6], AssistantContent::Text(text) if text.text == "done"));
    }

    #[test]
    fn tool_message_rejects_empty_tool_compatible_content() {
        let message = Message::new(
            agent_core_types::MessageRole::Tool,
            vec![MessagePart::ToolCall {
                call: ToolCall {
                    id: ToolCallId("tool-1".to_string()),
                    call_id: "call_tool_1".to_string(),
                    tool_name: "grep".to_string(),
                    arguments: json!({"pattern":"todo"}),
                    origin: ToolOrigin::Local,
                },
            }],
        );

        let error = to_rig_message(message).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("tool message did not contain any tool-compatible content")
        );
    }
}
