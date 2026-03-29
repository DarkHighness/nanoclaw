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
use store::SessionStore;
use tools::{ToolExecutionContext, ToolRegistry};
use tracing::info;
use types::{HookContext, HookRegistration, Message, SessionEventKind, TurnId};

pub struct AgentRuntime {
    backend: Arc<dyn ModelBackend>,
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn SessionStore>,
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
        store: Arc<dyn SessionStore>,
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
    pub fn session_id(&self) -> types::SessionId {
        self.session.session_id.clone()
    }

    #[must_use]
    pub fn agent_session_id(&self) -> types::AgentSessionId {
        self.session.agent_session_id.clone()
    }

    #[must_use]
    pub fn visible_transcript_snapshot(&self) -> Vec<types::Message> {
        self.visible_transcript()
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

    #[must_use]
    pub fn token_ledger(&self) -> types::TokenLedgerSnapshot {
        self.session.token_ledger.clone()
    }

    pub async fn end_session(&mut self, reason: Option<String>) -> Result<()> {
        self.append_event(None, None, SessionEventKind::SessionEnd { reason })
            .await
    }

    pub async fn start_new_session(&mut self) -> Result<()> {
        const NEW_SESSION_REASON: &str = "operator_new_session";

        if self.session.has_activity() {
            self.append_event(
                None,
                None,
                SessionEventKind::SessionEnd {
                    reason: Some(NEW_SESSION_REASON.to_string()),
                },
            )
            .await?;
        }

        self.session = RuntimeSession::new(types::SessionId::new(), types::AgentSessionId::new());
        self.clear_pending_request_effects();
        self.tool_loop_detector.reset();

        let hooks = self.hook_registrations.clone();
        self.start_agent_session(&TurnId::new(), &hooks, NEW_SESSION_REASON)
            .await
    }

    pub async fn resume_session(&mut self, session: RuntimeSession) -> Result<()> {
        const RESUME_SWITCH_REASON: &str = "operator_resume_switch";
        const RESUME_START_REASON: &str = "resume";

        if self.session.has_activity() {
            self.append_event(
                None,
                None,
                SessionEventKind::SessionEnd {
                    reason: Some(RESUME_SWITCH_REASON.to_string()),
                },
            )
            .await?;
        }

        let mut session = session;
        session.agent_session_id = types::AgentSessionId::new();
        session.agent_session_started = false;
        session.provider_continuation = None;
        session.provider_transcript_cursor = 0;
        session.token_ledger = types::TokenLedgerSnapshot::default();
        self.session = session;
        self.clear_pending_request_effects();
        self.tool_loop_detector.reset();

        let hooks = self.hook_registrations.clone();
        self.start_agent_session(&TurnId::new(), &hooks, RESUME_START_REASON)
            .await
    }

    pub(super) async fn start_agent_session(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        reason: &str,
    ) -> Result<()> {
        if self.session.agent_session_started {
            return Ok(());
        }

        let session_start_hooks = self
            .run_hooks(
                hooks,
                HookContext {
                    event: types::HookEvent::SessionStart,
                    session_id: self.session.session_id.clone(),
                    agent_session_id: self.session.agent_session_id.clone(),
                    turn_id: None,
                    fields: [("reason".to_string(), reason.to_string())]
                        .into_iter()
                        .collect(),
                    payload: serde_json::json!({ "reason": reason }),
                },
            )
            .await?;
        let session_start_effects = self
            .apply_hook_effects(turn_id, session_start_hooks, None, None)
            .await?;
        if let Some(reason) = session_start_effects.blocked_reason("session start blocked") {
            return Err(types::AgentCoreError::HookBlocked(reason).into());
        }
        self.append_event(
            None,
            None,
            SessionEventKind::SessionStart {
                reason: Some(reason.to_string()),
            },
        )
        .await?;
        self.session.agent_session_started = true;
        Ok(())
    }

    pub(super) async fn rotate_agent_session(
        &mut self,
        turn_id: &TurnId,
        hooks: &[HookRegistration],
        end_reason: &str,
        start_reason: &str,
    ) -> Result<()> {
        let previous_agent_session_id = self.session.agent_session_id.clone();
        self.append_event(
            None,
            None,
            SessionEventKind::SessionEnd {
                reason: Some(end_reason.to_string()),
            },
        )
        .await?;

        let next_agent_session_id = types::AgentSessionId::new();
        info!(
            session_id = %self.session.session_id,
            previous_agent_session_id = %previous_agent_session_id,
            next_agent_session_id = %next_agent_session_id,
            reason = start_reason,
            "rotated root agent session"
        );

        self.session.agent_session_id = next_agent_session_id;
        self.session.agent_session_started = false;
        self.session.token_ledger = types::TokenLedgerSnapshot::default();
        self.reset_provider_continuation();
        self.start_agent_session(turn_id, hooks, start_reason).await
    }

    pub async fn run_user_prompt(&mut self, prompt: impl Into<String>) -> Result<RunTurnOutcome> {
        let mut observer = NoopRuntimeObserver;
        self.run_user_prompt_with_observer(prompt, &mut observer)
            .await
    }

    pub async fn compact_now(&mut self, instructions: Option<String>) -> Result<bool> {
        let mut observer = NoopRuntimeObserver;
        self.compact_now_with_observer(instructions, &mut observer)
            .await
    }

    pub async fn compact_now_with_observer(
        &mut self,
        instructions: Option<String>,
        observer: &mut dyn RuntimeObserver,
    ) -> Result<bool> {
        self.compact_visible_history(&TurnId::new(), "manual", instructions, None, observer)
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
            self.session.session_id.clone(),
            self.session.agent_session_id.clone(),
            turn_id.clone(),
        );
        self.store.append(event).await?;
        self.append_event(
            Some(turn_id),
            None,
            SessionEventKind::SteerApplied {
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
            session_id = %self.session.session_id,
            agent_session_id = %self.session.agent_session_id,
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
