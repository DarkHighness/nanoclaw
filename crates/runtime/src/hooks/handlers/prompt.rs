use crate::{Result, RuntimeError};
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

pub struct FailClosedPromptHookEvaluator;

#[async_trait]
impl PromptHookEvaluator for FailClosedPromptHookEvaluator {
    async fn evaluate(
        &self,
        registration: &HookRegistration,
        _context: HookContext,
    ) -> Result<HookResult> {
        // Prompt hooks are intentionally fail-closed until a real evaluator
        // exists. Silently succeeding here would make plugin manifests look
        // active while skipping their declared control path entirely.
        Err(RuntimeError::hook(format!(
            "hook `{}` uses handler `prompt`, which is not implemented",
            registration.name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{FailClosedPromptHookEvaluator, PromptHookEvaluator};
    use types::{
        AgentSessionId, HookContext, HookEvent, HookHandler, HookRegistration, PromptHookHandler,
        SessionId,
    };

    #[tokio::test]
    async fn prompt_handler_fails_closed() {
        let error = FailClosedPromptHookEvaluator
            .evaluate(
                &HookRegistration {
                    name: "prompt-gate".into(),
                    event: HookEvent::UserPromptSubmit,
                    matcher: None,
                    handler: HookHandler::Prompt(PromptHookHandler {
                        prompt: "unused".to_string(),
                    }),
                    timeout_ms: None,
                    execution: None,
                },
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_1"),
                    agent_session_id: AgentSessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not implemented"));
    }
}
