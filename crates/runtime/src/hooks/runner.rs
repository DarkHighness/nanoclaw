use crate::{
    AgentHookEvaluator, CommandHookExecutor, DefaultCommandHookExecutor, DefaultWasmHookExecutor,
    HttpHookExecutor, NoopAgentHookEvaluator, NoopPromptHookEvaluator, PromptHookEvaluator,
    ReqwestHttpHookExecutor, Result, WasmHookExecutor, matches_hook,
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
            prompt_evaluator: Arc::new(NoopPromptHookEvaluator),
            agent_evaluator: Arc::new(NoopAgentHookEvaluator),
            wasm_executor: Arc::new(DefaultWasmHookExecutor),
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
