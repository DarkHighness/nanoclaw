use agent_core_types::{HookContext, HookOutput};
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait AgentHookEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        prompt: &str,
        allowed_tools: &[String],
        context: HookContext,
    ) -> Result<HookOutput>;
}

pub struct NoopAgentHookEvaluator;

#[async_trait]
impl AgentHookEvaluator for NoopAgentHookEvaluator {
    async fn evaluate(
        &self,
        _prompt: &str,
        _allowed_tools: &[String],
        _context: HookContext,
    ) -> Result<HookOutput> {
        Ok(HookOutput::default())
    }
}
