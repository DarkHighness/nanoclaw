use crate::{
    AgentHookEvaluator, CommandHookExecutor, DefaultCommandHookExecutor, DefaultWasmHookExecutor,
    FailClosedAgentHookEvaluator, FailClosedPromptHookEvaluator, HttpHookExecutor,
    PromptHookEvaluator, ReqwestHttpHookExecutor, Result, WasmHookExecutor, matches_hook,
};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use types::{HookContext, HookHandler, HookRegistration, HookResult};

#[derive(Clone, Debug, PartialEq)]
pub struct HookInvocation {
    pub registration: HookRegistration,
    pub output: HookResult,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HookInvocationBatch {
    pub invocations: Vec<HookInvocation>,
}

impl HookInvocationBatch {
    pub fn push(&mut self, registration: HookRegistration, output: HookResult) {
        self.invocations.push(HookInvocation {
            registration,
            output,
        });
    }
}

pub struct HookRunner {
    command_executor: Arc<dyn CommandHookExecutor>,
    http_executor: Arc<dyn HttpHookExecutor>,
    prompt_evaluator: Arc<dyn PromptHookEvaluator>,
    agent_evaluator: Arc<dyn AgentHookEvaluator>,
    wasm_executor: Arc<dyn WasmHookExecutor>,
    async_tx: mpsc::Sender<HookInvocation>,
    async_rx: Mutex<mpsc::Receiver<HookInvocation>>,
}

const ASYNC_HOOK_BUFFER_CAPACITY: usize = 64;

impl Default for HookRunner {
    fn default() -> Self {
        let (async_tx, async_rx) = mpsc::channel(ASYNC_HOOK_BUFFER_CAPACITY);
        Self {
            command_executor: Arc::new(DefaultCommandHookExecutor::default()),
            http_executor: Arc::new(ReqwestHttpHookExecutor::default()),
            prompt_evaluator: Arc::new(FailClosedPromptHookEvaluator),
            agent_evaluator: Arc::new(FailClosedAgentHookEvaluator),
            wasm_executor: Arc::new(DefaultWasmHookExecutor::default()),
            async_tx,
            async_rx: Mutex::new(async_rx),
        }
    }
}

impl HookRunner {
    #[must_use]
    pub fn with_services(
        command_executor: Arc<dyn CommandHookExecutor>,
        http_executor: Arc<dyn HttpHookExecutor>,
        prompt_evaluator: Arc<dyn PromptHookEvaluator>,
        agent_evaluator: Arc<dyn AgentHookEvaluator>,
        wasm_executor: Arc<dyn WasmHookExecutor>,
    ) -> Self {
        let (async_tx, async_rx) = mpsc::channel(ASYNC_HOOK_BUFFER_CAPACITY);
        Self {
            command_executor,
            http_executor,
            prompt_evaluator,
            agent_evaluator,
            wasm_executor,
            async_tx,
            async_rx: Mutex::new(async_rx),
        }
    }

    pub async fn drain_async_invocations(&self) -> HookInvocationBatch {
        let mut batch = HookInvocationBatch::default();
        let mut guard = self.async_rx.lock().expect("hook async receiver lock");
        while let Ok(invocation) = guard.try_recv() {
            batch.invocations.push(invocation);
        }
        batch
    }

    pub async fn run(
        &self,
        registrations: &[HookRegistration],
        context: HookContext,
    ) -> Result<HookInvocationBatch> {
        let mut batch = HookInvocationBatch::default();
        for registration in registrations
            .iter()
            .filter(|registration| registration.event == context.event)
        {
            if !matches_hook(registration, &context)? {
                continue;
            }
            match &registration.handler {
                HookHandler::Command(command) if command.asynchronous => {
                    let tx = self.async_tx.clone();
                    let command_executor = self.command_executor.clone();
                    let registration = registration.clone();
                    let context = context.clone();
                    tokio::spawn(async move {
                        if let Ok(output) = command_executor.execute(&registration, context).await {
                            let _ = tx
                                .send(HookInvocation {
                                    registration,
                                    output,
                                })
                                .await;
                        }
                    });
                }
                HookHandler::Command(_) => {
                    let output = self
                        .command_executor
                        .execute(registration, context.clone())
                        .await?;
                    batch.push(registration.clone(), output);
                }
                HookHandler::Http(_) => {
                    let output = self
                        .http_executor
                        .execute(registration, context.clone())
                        .await?;
                    batch.push(registration.clone(), output);
                }
                HookHandler::Prompt(_) => {
                    let output = self
                        .prompt_evaluator
                        .evaluate(registration, context.clone())
                        .await?;
                    batch.push(registration.clone(), output);
                }
                HookHandler::Agent(_) => {
                    let output = self
                        .agent_evaluator
                        .evaluate(registration, context.clone())
                        .await?;
                    batch.push(registration.clone(), output);
                }
                HookHandler::Wasm(_) => {
                    let output = self
                        .wasm_executor
                        .execute(registration, context.clone())
                        .await?;
                    batch.push(registration.clone(), output);
                }
            }
        }
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::HookRunner;
    use types::{
        AgentHookHandler, HookContext, HookEvent, HookHandler, HookRegistration, PromptHookHandler,
        RunId, SessionId,
    };

    #[tokio::test]
    async fn prompt_hooks_fail_closed_by_default() {
        let runner = HookRunner::default();
        let error = runner
            .run(
                &[HookRegistration {
                    name: "prompt-hook".to_string(),
                    event: HookEvent::UserPromptSubmit,
                    matcher: None,
                    handler: HookHandler::Prompt(PromptHookHandler {
                        prompt: "unused".to_string(),
                    }),
                    timeout_ms: None,
                    execution: None,
                }],
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not implemented"));
    }

    #[tokio::test]
    async fn agent_hooks_fail_closed_by_default() {
        let runner = HookRunner::default();
        let error = runner
            .run(
                &[HookRegistration {
                    name: "agent-hook".to_string(),
                    event: HookEvent::SubagentStart,
                    matcher: None,
                    handler: HookHandler::Agent(AgentHookHandler {
                        prompt: "review".to_string(),
                        allowed_tools: Vec::new(),
                    }),
                    timeout_ms: None,
                    execution: None,
                }],
                HookContext {
                    event: HookEvent::SubagentStart,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
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
