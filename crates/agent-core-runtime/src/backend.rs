use crate::Result;
use agent_core_types::{ModelEvent, ModelRequest};
use async_trait::async_trait;
use futures::stream::BoxStream;

#[async_trait]
pub trait ModelBackend: Send + Sync {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>>;
}
