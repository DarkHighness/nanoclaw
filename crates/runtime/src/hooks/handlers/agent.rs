use crate::Result;
use async_trait::async_trait;
use types::{HookContext, HookOutput, ToolName};

#[async_trait]
pub trait AgentHookEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        prompt: &str,
        allowed_tools: &[ToolName],
        context: HookContext,
    ) -> Result<HookOutput>;
}

pub struct NoopAgentHookEvaluator;

#[async_trait]
impl AgentHookEvaluator for NoopAgentHookEvaluator {
    async fn evaluate(
        &self,
        _prompt: &str,
        _allowed_tools: &[ToolName],
        _context: HookContext,
    ) -> Result<HookOutput> {
        Ok(HookOutput::default())
    }
}
