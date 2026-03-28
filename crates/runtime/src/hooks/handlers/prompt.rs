use crate::Result;
use async_trait::async_trait;
use types::{HookContext, HookRegistration, HookResult};

#[async_trait]
pub trait PromptHookEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult>;
}

pub struct NoopPromptHookEvaluator;

#[async_trait]
impl PromptHookEvaluator for NoopPromptHookEvaluator {
    async fn evaluate(
        &self,
        _registration: &HookRegistration,
        _context: HookContext,
    ) -> Result<HookResult> {
        Ok(HookResult::default())
    }
}
