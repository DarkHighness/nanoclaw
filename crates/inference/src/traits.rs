use crate::{ExpandedQuery, RerankDocument, RerankJudgment, Result};
use async_trait::async_trait;

#[async_trait]
pub trait EmbeddingClient: Send + Sync {
    async fn embed(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[async_trait]
pub trait QueryExpansionClient: Send + Sync {
    async fn expand(&self, model: &str, query: &str, variants: usize)
    -> Result<Vec<ExpandedQuery>>;
}

#[async_trait]
pub trait RerankClient: Send + Sync {
    async fn rerank(
        &self,
        model: &str,
        query: &str,
        documents: &[RerankDocument],
    ) -> Result<Vec<RerankJudgment>>;
}
