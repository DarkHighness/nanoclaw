use crate::{
    AnthropicTransport, OpenAiResponsesOptions, OpenAiTransport, OpenAiTransportMode,
    ProviderDescriptor, ProviderKind, Result, build_anthropic_transport, build_openai_transport,
    openai_capabilities, stream_anthropic_turn, stream_openai_turn,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use runtime::{ModelBackend, ModelBackendCapabilities, Result as RuntimeResult};
use serde_json::Value;
use tracing::debug;
use types::ModelRequest;

#[derive(Clone, Debug)]
pub struct BackendDescriptor {
    pub provider: ProviderDescriptor,
    pub capabilities: ModelBackendCapabilities,
}

impl BackendDescriptor {
    #[must_use]
    pub fn new(provider: ProviderDescriptor) -> Self {
        Self {
            provider,
            capabilities: ModelBackendCapabilities::text_tool_model_defaults(),
        }
    }

    #[must_use]
    pub fn with_capabilities(mut self, capabilities: ModelBackendCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn resolved_for_request(mut self, request_options: &RequestOptions) -> Self {
        if matches!(self.provider.kind, ProviderKind::OpenAi) {
            let capabilities = openai_capabilities(request_options);
            // Transport/runtime features are resolved here because they depend
            // on request options such as Responses chaining and websocket mode,
            // not just on the declared model lane.
            self.capabilities.provider_managed_history = capabilities.provider_managed_history;
            self.capabilities.provider_native_compaction = capabilities.provider_native_compaction;
        }
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub additional_params: Option<Value>,
    pub prompt_cache_key: Option<String>,
    pub prompt_cache_retention: Option<PromptCacheRetention>,
    pub openai_transport: Option<OpenAiTransportMode>,
    pub openai_responses: Option<OpenAiResponsesOptions>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptCacheRetention {
    InMemory,
    Hours24,
}

impl PromptCacheRetention {
    #[must_use]
    pub fn as_api_value(self) -> &'static str {
        match self {
            Self::InMemory => "in_memory",
            Self::Hours24 => "24h",
        }
    }
}

#[derive(Clone, Debug)]
enum ProviderTransport {
    OpenAi(OpenAiTransport),
    Anthropic(AnthropicTransport),
}

#[derive(Debug)]
pub struct ProviderBackend {
    descriptor: BackendDescriptor,
    transport: ProviderTransport,
    request_options: RequestOptions,
}

impl ProviderBackend {
    pub fn new(provider: ProviderDescriptor) -> Result<Self> {
        Self::from_settings(
            BackendDescriptor::new(provider),
            RequestOptions::default(),
            None,
        )
    }

    pub fn from_descriptor(descriptor: BackendDescriptor) -> Result<Self> {
        Self::from_settings(descriptor, RequestOptions::default(), None)
    }

    pub fn from_settings(
        descriptor: BackendDescriptor,
        request_options: RequestOptions,
        base_url: Option<String>,
    ) -> Result<Self> {
        Self::from_settings_with_api_key(descriptor, request_options, base_url, None)
    }

    pub fn from_settings_with_api_key(
        descriptor: BackendDescriptor,
        request_options: RequestOptions,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self> {
        let descriptor = descriptor.resolved_for_request(&request_options);

        let transport = match descriptor.provider.kind {
            ProviderKind::OpenAi => ProviderTransport::OpenAi(build_openai_transport(
                base_url.as_deref(),
                api_key.as_deref(),
            )?),
            ProviderKind::Anthropic => ProviderTransport::Anthropic(build_anthropic_transport(
                base_url.as_deref(),
                api_key.as_deref(),
            )?),
        };

        Ok(Self {
            descriptor,
            transport,
            request_options,
        })
    }

    #[must_use]
    pub fn descriptor(&self) -> &BackendDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn request_options(&self) -> &RequestOptions {
        &self.request_options
    }
}

#[async_trait]
impl ModelBackend for ProviderBackend {
    fn capabilities(&self) -> ModelBackendCapabilities {
        self.descriptor.capabilities
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<types::ModelEvent>>> {
        debug!(
            provider = ?self.descriptor.provider.kind,
            model = %self.descriptor.provider.model,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            "starting provider stream turn"
        );
        match &self.transport {
            ProviderTransport::OpenAi(transport) => {
                stream_openai_turn(
                    transport.clone(),
                    self.descriptor.provider.model.clone(),
                    request,
                    self.request_options.clone(),
                )
                .await
            }
            ProviderTransport::Anthropic(transport) => {
                stream_anthropic_turn(
                    transport.clone(),
                    self.descriptor.provider.model.clone(),
                    request,
                    self.request_options.clone(),
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackendDescriptor, OpenAiResponsesOptions, PromptCacheRetention, ProviderBackend,
        ProviderDescriptor, RequestOptions,
    };
    use crate::{OpenAiServerCompaction, OpenAiTransportMode};
    use runtime::{ModelBackend, ModelBackendCapabilities};

    #[test]
    fn openai_backend_surfaces_provider_managed_history_capabilities() {
        let backend = ProviderBackend::from_settings(
            BackendDescriptor::new(ProviderDescriptor::openai("gpt-5.4")),
            RequestOptions {
                prompt_cache_key: Some("workspace:main".to_string()),
                prompt_cache_retention: Some(PromptCacheRetention::Hours24),
                openai_responses: Some(OpenAiResponsesOptions {
                    chain_previous_response: true,
                    store: Some(true),
                    server_compaction: Some(OpenAiServerCompaction {
                        compact_threshold: 200_000,
                    }),
                }),
                ..RequestOptions::default()
            },
            Some("https://example.invalid/v1".to_string()),
        )
        .unwrap_err();

        assert!(backend.to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn websocket_transport_disables_openai_continuation_capabilities() {
        let backend = ProviderBackend::from_settings_with_api_key(
            BackendDescriptor::new(ProviderDescriptor::openai("gpt-realtime")),
            RequestOptions {
                openai_transport: Some(OpenAiTransportMode::RealtimeWebSocket),
                openai_responses: Some(OpenAiResponsesOptions {
                    chain_previous_response: true,
                    store: Some(true),
                    server_compaction: Some(OpenAiServerCompaction {
                        compact_threshold: 200_000,
                    }),
                }),
                ..RequestOptions::default()
            },
            Some("https://example.invalid/v1".to_string()),
            Some("test-key".to_string()),
        )
        .unwrap();

        let capabilities = backend.capabilities();
        assert!(!capabilities.provider_managed_history);
        assert!(!capabilities.provider_native_compaction);
    }

    #[test]
    fn backend_descriptor_preserves_declared_model_surface() {
        let backend = ProviderBackend::from_settings_with_api_key(
            BackendDescriptor::new(ProviderDescriptor::openai("gpt-5.4")).with_capabilities(
                ModelBackendCapabilities::from_model_surface(false, true, true, false, true),
            ),
            RequestOptions {
                openai_responses: Some(OpenAiResponsesOptions {
                    chain_previous_response: true,
                    store: Some(true),
                    server_compaction: None,
                }),
                ..RequestOptions::default()
            },
            Some("https://example.invalid/v1".to_string()),
            Some("test-key".to_string()),
        )
        .unwrap();

        let capabilities = backend.capabilities();
        assert!(!capabilities.tool_calls);
        assert!(capabilities.vision);
        assert!(capabilities.image_generation);
        assert!(!capabilities.audio_input);
        assert!(capabilities.tts);
        assert!(capabilities.provider_managed_history);
    }
}
