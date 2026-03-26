use crate::Result;
use agent_core_types::{ModelEvent, ModelRequest};
use async_trait::async_trait;
use futures::stream::BoxStream;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelBackendCapabilities {
    pub provider_managed_history: bool,
    pub provider_native_compaction: bool,
}

#[async_trait]
pub trait ModelBackend: Send + Sync {
    fn capabilities(&self) -> ModelBackendCapabilities {
        ModelBackendCapabilities::default()
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>>;
}
