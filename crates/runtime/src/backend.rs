use crate::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use types::{ModelEvent, ModelRequest, ToolVisibilityContext};

/// Effective backend capability surface exposed to host runtimes.
///
/// The first group captures model-facing features that hosts may use for
/// routing and tool registration. The provider-managed fields describe turn
/// lifecycle behavior that only the backend adapter can decide after transport
/// and request options are known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelBackendCapabilities {
    pub tool_calls: bool,
    pub vision: bool,
    pub image_generation: bool,
    pub audio_input: bool,
    pub tts: bool,
    pub provider_managed_history: bool,
    pub provider_native_compaction: bool,
}

impl ModelBackendCapabilities {
    #[must_use]
    pub const fn from_model_surface(
        tool_calls: bool,
        vision: bool,
        image_generation: bool,
        audio_input: bool,
        tts: bool,
    ) -> Self {
        Self {
            tool_calls,
            vision,
            image_generation,
            audio_input,
            tts,
            provider_managed_history: false,
            provider_native_compaction: false,
        }
    }

    #[must_use]
    pub const fn text_tool_model_defaults() -> Self {
        Self::from_model_surface(true, false, false, false, false)
    }
}

#[async_trait]
pub trait ModelBackend: Send + Sync {
    fn provider_name(&self) -> &'static str {
        "unknown"
    }

    fn tool_visibility_context(&self) -> ToolVisibilityContext {
        let provider_name = self.provider_name();
        if provider_name == "unknown" {
            ToolVisibilityContext::default()
        } else {
            ToolVisibilityContext::default().with_provider(provider_name)
        }
    }

    fn capabilities(&self) -> ModelBackendCapabilities {
        ModelBackendCapabilities::default()
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>>;
}
