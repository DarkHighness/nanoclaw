use crate::Result;
use async_trait::async_trait;
use types::{HookContext, HookRegistration, HookResult};

#[async_trait]
pub trait AgentHookEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult>;
}

pub struct NoopAgentHookEvaluator;

#[async_trait]
impl AgentHookEvaluator for NoopAgentHookEvaluator {
    async fn evaluate(
        &self,
        _registration: &HookRegistration,
        _context: HookContext,
    ) -> Result<HookResult> {
        Ok(HookResult::default())
    }
}
