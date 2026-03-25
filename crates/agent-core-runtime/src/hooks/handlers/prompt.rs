use crate::Result;
use agent_core_types::{HookContext, HookOutput};
use async_trait::async_trait;

#[async_trait]
pub trait PromptHookEvaluator: Send + Sync {
    async fn evaluate(&self, prompt: &str, context: HookContext) -> Result<HookOutput>;
}

pub struct NoopPromptHookEvaluator;

#[async_trait]
impl PromptHookEvaluator for NoopPromptHookEvaluator {
    async fn evaluate(&self, _prompt: &str, _context: HookContext) -> Result<HookOutput> {
        Ok(HookOutput::default())
    }
}
