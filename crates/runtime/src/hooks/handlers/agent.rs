use crate::{Result, RuntimeError};
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

pub struct FailClosedAgentHookEvaluator;

#[async_trait]
impl AgentHookEvaluator for FailClosedAgentHookEvaluator {
    async fn evaluate(
        &self,
        registration: &HookRegistration,
        _context: HookContext,
    ) -> Result<HookResult> {
        // Agent hooks are control-plane extensions. Until they are actually
        // wired, failing closed is safer than pretending the hook executed.
        Err(RuntimeError::hook(format!(
            "hook `{}` uses handler `agent`, which is not implemented",
            registration.name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentHookEvaluator, FailClosedAgentHookEvaluator};
    use types::{
        AgentHookHandler, AgentSessionId, HookContext, HookEvent, HookHandler, HookRegistration,
        SessionId,
    };

    #[tokio::test]
    async fn agent_handler_fails_closed() {
        let error = FailClosedAgentHookEvaluator
            .evaluate(
                &HookRegistration {
                    name: "agent-review".into(),
                    event: HookEvent::SubagentStart,
                    matcher: None,
                    handler: HookHandler::Agent(AgentHookHandler {
                        prompt: "review".to_string(),
                        allowed_tools: Vec::new(),
                    }),
                    timeout_ms: None,
                    execution: None,
                },
                HookContext {
                    event: HookEvent::SubagentStart,
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
