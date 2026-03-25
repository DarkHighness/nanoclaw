use crate::{
    AgentHookEvaluator, CommandHookExecutor, DefaultCommandHookExecutor, HttpHookExecutor,
    NoopAgentHookEvaluator, NoopPromptHookEvaluator, PromptHookEvaluator, ReqwestHttpHookExecutor,
    Result, matches_hook,
};
use agent_core_types::{
    GateDecision, HookContext, HookDecision, HookHandler, HookOutput, HookRegistration,
    PermissionBehavior, PermissionDecision,
};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

#[derive(Default)]
pub struct HookAggregate {
    pub system_messages: Vec<String>,
    pub additional_context: Vec<String>,
    pub permission_decision: Option<PermissionDecision>,
    pub permission_behavior: Option<PermissionBehavior>,
    pub gate_decision: Option<GateDecision>,
    pub gate_reason: Option<String>,
    pub continue_allowed: bool,
    pub stop_reason: Option<String>,
}

impl HookAggregate {
    pub fn absorb(&mut self, output: HookOutput) {
        if let Some(message) = output.system_message {
            self.system_messages.push(message);
        }
        self.additional_context.extend(output.additional_context);
        self.continue_allowed &= output.r#continue;
        if self.stop_reason.is_none() {
            self.stop_reason = output.stop_reason;
        }
        if let Some(decision) = output.decision {
            match decision {
                HookDecision::PreToolUse {
                    permission_decision,
                } => {
                    self.permission_decision =
                        Some(match (self.permission_decision, permission_decision) {
                            (Some(PermissionDecision::Deny), _) | (_, PermissionDecision::Deny) => {
                                PermissionDecision::Deny
                            }
                            (Some(PermissionDecision::Ask), _) | (_, PermissionDecision::Ask) => {
                                PermissionDecision::Ask
                            }
                            _ => PermissionDecision::Allow,
                        });
                }
                HookDecision::PermissionRequest { behavior, reason } => {
                    self.permission_behavior = Some(match (self.permission_behavior, behavior) {
                        (Some(PermissionBehavior::Deny), _) | (_, PermissionBehavior::Deny) => {
                            PermissionBehavior::Deny
                        }
                        _ => PermissionBehavior::Allow,
                    });
                    if self.gate_reason.is_none() {
                        self.gate_reason = reason;
                    }
                }
                HookDecision::Gate { decision, reason } => {
                    if matches!(decision, GateDecision::Block) {
                        self.gate_decision = Some(GateDecision::Block);
                    } else if self.gate_decision.is_none() {
                        self.gate_decision = Some(GateDecision::Allow);
                    }
                    if self.gate_reason.is_none() {
                        self.gate_reason = reason;
                    }
                }
                HookDecision::Elicitation { .. } => {}
            }
        }
    }
}

pub struct HookRunner {
    command_executor: Arc<dyn CommandHookExecutor>,
    http_executor: Arc<dyn HttpHookExecutor>,
    prompt_evaluator: Arc<dyn PromptHookEvaluator>,
    agent_evaluator: Arc<dyn AgentHookEvaluator>,
    async_tx: mpsc::UnboundedSender<HookOutput>,
    async_rx: Mutex<mpsc::UnboundedReceiver<HookOutput>>,
}

impl Default for HookRunner {
    fn default() -> Self {
        let (async_tx, async_rx) = mpsc::unbounded_channel();
        Self {
            command_executor: Arc::new(DefaultCommandHookExecutor::default()),
            http_executor: Arc::new(ReqwestHttpHookExecutor::default()),
            prompt_evaluator: Arc::new(NoopPromptHookEvaluator),
            agent_evaluator: Arc::new(NoopAgentHookEvaluator),
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
    ) -> Self {
        let (async_tx, async_rx) = mpsc::unbounded_channel();
        Self {
            command_executor,
            http_executor,
            prompt_evaluator,
            agent_evaluator,
            async_tx,
            async_rx: Mutex::new(async_rx),
        }
    }

    pub async fn drain_async_context(&self) -> HookAggregate {
        let mut aggregate = HookAggregate::default();
        let mut guard = self.async_rx.lock().await;
        while let Ok(output) = guard.try_recv() {
            aggregate.absorb(output);
        }
        aggregate
    }

    pub async fn run(
        &self,
        registrations: &[HookRegistration],
        context: HookContext,
    ) -> Result<HookAggregate> {
        let mut aggregate = HookAggregate {
            continue_allowed: true,
            ..HookAggregate::default()
        };
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
                    let command_text = command.command.clone();
                    let context = context.clone();
                    tokio::spawn(async move {
                        if let Ok(output) = command_executor.execute(&command_text, context).await {
                            let _ = tx.send(HookOutput {
                                decision: None,
                                ..output
                            });
                        }
                    });
                }
                HookHandler::Command(command) => {
                    aggregate.absorb(
                        self.command_executor
                            .execute(&command.command, context.clone())
                            .await?,
                    );
                }
                HookHandler::Http(http) => {
                    aggregate.absorb(
                        self.http_executor
                            .execute(
                                &http.method,
                                &http.url,
                                &http.headers.clone().into_iter().collect(),
                                context.clone(),
                            )
                            .await?,
                    );
                }
                HookHandler::Prompt(prompt) => {
                    aggregate.absorb(
                        self.prompt_evaluator
                            .evaluate(&prompt.prompt, context.clone())
                            .await?,
                    );
                }
                HookHandler::Agent(agent) => {
                    aggregate.absorb(
                        self.agent_evaluator
                            .evaluate(&agent.prompt, &agent.allowed_tools, context.clone())
                            .await?,
                    );
                }
            }
        }
        Ok(aggregate)
    }
}
