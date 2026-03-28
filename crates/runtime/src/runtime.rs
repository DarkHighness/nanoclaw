mod event_log;
mod history;
mod hook_effects;
mod provider_state;
mod tool_flow;
mod turn_loop;
mod turn_start;

use crate::{
    CompactionConfig, ConversationCompactor, HookInvocationBatch, HookRunner, LoopDetectionConfig,
    ModelBackend, NoopRuntimeObserver, Result, RuntimeCommand, RuntimeObserver,
    RuntimeProgressEvent, RuntimeSession, ToolApprovalHandler, ToolApprovalPolicy,
    ToolLoopDetector, append_transcript_message,
};
use skills::SkillCatalog;
use std::sync::Arc;
use store::RunStore;
use tools::{ToolExecutionContext, ToolRegistry};
use tracing::info;
use types::{HookContext, HookRegistration, Message, RunEventKind, TurnId};

pub struct AgentRuntime {
    backend: Arc<dyn ModelBackend>,
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn RunStore>,
    tool_registry: ToolRegistry,
    tool_context: ToolExecutionContext,
    tool_approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
    conversation_compactor: Arc<dyn ConversationCompactor>,
    compaction_config: CompactionConfig,
    tool_loop_detector: ToolLoopDetector,
    base_instructions: Vec<String>,
    hook_registrations: Vec<HookRegistration>,
    pending_additional_context: Vec<String>,
    pending_injected_instructions: Vec<String>,
    session: RuntimeSession,
}

pub struct RunTurnOutcome {
    pub turn_id: TurnId,
    pub assistant_text: String,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        backend: Arc<dyn ModelBackend>,
        hook_runner: Arc<HookRunner>,
        store: Arc<dyn RunStore>,
        tool_registry: ToolRegistry,
        tool_context: ToolExecutionContext,
        tool_approval_handler: Arc<dyn ToolApprovalHandler>,
        tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
        conversation_compactor: Arc<dyn ConversationCompactor>,
        compaction_config: CompactionConfig,
        loop_detection_config: LoopDetectionConfig,
        base_instructions: Vec<String>,
        hook_registrations: Vec<HookRegistration>,
        _skill_catalog: SkillCatalog,
        session: RuntimeSession,
    ) -> Self {
        Self {
            backend,
            hook_runner,
            store,
            tool_registry,
            tool_context,
            tool_approval_handler,
            tool_approval_policy,
            conversation_compactor,
            compaction_config,
            tool_loop_detector: ToolLoopDetector::new(loop_detection_config),
            base_instructions,
            hook_registrations,
            pending_additional_context: Vec::new(),
            pending_injected_instructions: Vec::new(),
            session,
        }
    }

    #[must_use]
    pub fn run_id(&self) -> types::RunId {
        self.session.run_id.clone()
    }

    #[must_use]
    pub fn session_id(&self) -> types::SessionId {
        self.session.session_id.clone()
    }

    #[must_use]
    pub fn tool_registry_names(&self) -> Vec<String> {
        self.tool_registry
            .names()
            .into_iter()
            .map(|name| name.to_string())
            .collect()
    }

    #[must_use]
    pub fn tool_specs(&self) -> Vec<types::ToolSpec> {
        self.tool_registry.specs()
    }

    pub async fn end_session(&mut self, reason: Option<String>) -> Result<()> {
        self.append_event(None, None, RunEventKind::SessionEnd { reason })
            .await
    }

    pub async fn run_user_prompt(&mut self, prompt: impl Into<String>) -> Result<RunTurnOutcome> {
        let mut observer = NoopRuntimeObserver;
        self.run_user_prompt_with_observer(prompt, &mut observer)
            .await
    }

    pub async fn compact_now(&mut self, instructions: Option<String>) -> Result<bool> {
        let mut observer = NoopRuntimeObserver;
        self.compact_visible_history(&TurnId::new(), "manual", instructions, &mut observer)
            .await
    }

    pub async fn steer(
        &mut self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> Result<()> {
        let mut observer = NoopRuntimeObserver;
        self.steer_with_observer(message, reason, &mut observer)
            .await
    }

    pub async fn steer_with_observer(
        &mut self,
        message: impl Into<String>,
        reason: Option<String>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<()> {
        let message = message.into();
        let turn_id = TurnId::new();
        // Steering is append-only runtime control: it becomes a new system message
        // in the transcript instead of mutating the fixed preamble or prior history.
        let event = append_transcript_message(
            &mut self.session.transcript,
            Message::system(message.clone()),
            self.session.run_id.clone(),
            self.session.session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        self.append_event(
            Some(turn_id),
            None,
            RunEventKind::SteerApplied {
                message: message.clone(),
                reason: reason.clone(),
            },
        )
        .await?;
        observer.on_event(RuntimeProgressEvent::SteerApplied { message, reason })?;
        Ok(())
    }

    pub async fn apply_control(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<Option<RunTurnOutcome>> {
        let mut observer = NoopRuntimeObserver;
        self.apply_control_with_observer(command, &mut observer)
            .await
    }

    pub async fn apply_control_with_observer(
        &mut self,
        command: RuntimeCommand,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<Option<RunTurnOutcome>> {
        match command {
            RuntimeCommand::Prompt { prompt } => self
                .run_user_prompt_with_observer(prompt, observer)
                .await
                .map(Some),
            RuntimeCommand::Steer { message, reason } => {
                self.steer_with_observer(message, reason, observer).await?;
                Ok(None)
            }
        }
    }

    pub async fn run_user_prompt_with_observer(
        &mut self,
        prompt: impl Into<String>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<RunTurnOutcome> {
        let prompt = prompt.into();
        let turn_id = TurnId::new();
        let hooks = self.hook_registrations.clone();
        let instructions = self.base_instructions.clone();
        info!(
            run_id = %self.session.run_id,
            session_id = %self.session.session_id,
            turn_id = %turn_id,
            prompt_chars = prompt.chars().count(),
            "starting user turn"
        );
        self.prepare_user_turn(&turn_id, &hooks, &instructions, &prompt, observer)
            .await?;
        self.run_turn_loop(&turn_id, &hooks, &instructions, observer)
            .await
    }

    async fn run_hooks(
        &self,
        hooks: &[HookRegistration],
        context: HookContext,
    ) -> Result<HookInvocationBatch> {
        self.hook_runner.run(hooks, context).await
    }
}

#[cfg(test)]
mod tests;
