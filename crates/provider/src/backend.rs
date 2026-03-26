use crate::{
    AnthropicTransport, OpenAiResponsesOptions, OpenAiTransport, OpenAiTransportMode,
    ProviderCapabilities, ProviderDescriptor, ProviderKind, Result, build_anthropic_transport,
    build_openai_transport, openai_capabilities, stream_anthropic_turn, stream_openai_turn,
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
    pub capabilities: ProviderCapabilities,
}

impl BackendDescriptor {
    #[must_use]
    pub fn new(provider: ProviderDescriptor) -> Self {
        Self {
            provider,
            capabilities: ProviderCapabilities::default(),
        }
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
        mut descriptor: BackendDescriptor,
        request_options: RequestOptions,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<Self> {
        if matches!(descriptor.provider.kind, ProviderKind::OpenAi) {
            let capabilities = openai_capabilities(&request_options);
            descriptor.capabilities.provider_managed_history =
                capabilities.provider_managed_history;
            descriptor.capabilities.provider_native_compaction =
                capabilities.provider_native_compaction;
        }

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
        match self.descriptor.provider.kind {
            ProviderKind::OpenAi => openai_capabilities(&self.request_options),
            ProviderKind::Anthropic => ModelBackendCapabilities::default(),
        }
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
    use runtime::ModelBackend;

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
}
