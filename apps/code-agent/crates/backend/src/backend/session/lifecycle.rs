use super::*;
use agent::runtime::{RuntimeSession, fork_runtime_session, seed_runtime_session_events};

impl CodeAgentSession {
    pub async fn end_session(&self, reason: Option<String>) -> RuntimeResult<()> {
        self.user_inputs.cancel("session ended");
        self.permission_requests.cancel("session ended");
        let mut runtime = self.runtime.lock().await;
        runtime.end_session(reason).await
    }

    pub async fn apply_control(&self, command: RuntimeCommand) -> Result<()> {
        let _turn_guard = self.begin_active_turn()?;
        let mut runtime = self.runtime.lock().await;
        if let RuntimeCommand::Prompt { message, .. } = &command {
            self.store_side_question_context(Self::side_question_context_from_runtime(
                &runtime,
                Some(message.clone()),
            ));
        }
        let mut observer = SessionEventObserver::new(self.events.clone());
        runtime
            .apply_control_with_observer(command, &mut observer)
            .await
            .map_err(anyhow::Error::from)?;
        let snapshot = self.latest_compaction_working_snapshot(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(snapshot)
            .await;
        Ok(())
    }

    pub async fn queue_prompt_command(
        &self,
        message: Message,
        submitted_prompt: Option<agent::types::SubmittedPromptSnapshot>,
    ) -> Result<String> {
        let queued = self
            .control_plane
            .push_prompt_with_snapshot(message, submitted_prompt);
        Ok(queued.id.to_string())
    }

    pub async fn run_one_shot_prompt(&self, prompt: &str) -> Result<RunTurnOutcome> {
        let _turn_guard = self.begin_active_turn()?;
        let mut runtime = self.runtime.lock().await;
        self.store_side_question_context(Self::side_question_context_from_runtime(
            &runtime,
            Some(Message::user(prompt)),
        ));
        let mut observer = SessionEventObserver::new(self.events.clone());
        let outcome = runtime
            .run_user_prompt_with_observer(prompt, &mut observer)
            .await
            .map_err(anyhow::Error::from)?;
        let snapshot = self.latest_compaction_working_snapshot(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(snapshot)
            .await;
        Ok(outcome)
    }

    pub async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        let compacted = runtime
            .compact_now_with_observer(notes, &mut observer)
            .await?;
        let snapshot = self.latest_compaction_working_snapshot(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(snapshot)
            .await;
        Ok(compacted)
    }

    pub async fn apply_session_operation(
        &self,
        operation: SessionOperation,
    ) -> Result<SessionOperationOutcome> {
        match operation {
            SessionOperation::StartFresh => self.start_fresh_session().await,
            SessionOperation::ResumeAgentSession { agent_session_ref } => {
                self.resume_existing_agent_session(&agent_session_ref).await
            }
        }
    }

    async fn start_fresh_session(&self) -> Result<SessionOperationOutcome> {
        let (session_ref, agent_session_ref, side_question_context) = {
            let mut runtime = self.runtime.lock().await;
            runtime
                .start_new_session()
                .await
                .map_err(anyhow::Error::from)?;
            (
                runtime.session_id().to_string(),
                runtime.agent_session_id().to_string(),
                Self::side_question_context_from_runtime(&runtime, None),
            )
        };
        self.store_side_question_context(side_question_context.clone());
        self.reset_session_memory_refresh_state(&side_question_context)
            .await;
        self.worktree_manager
            .sync_attached_session(
                SessionId::from(session_ref.clone()),
                AgentSessionId::from(agent_session_ref.clone()),
            )
            .await
            .map_err(anyhow::Error::from)?;
        self.set_runtime_session_refs(session_ref, agent_session_ref);
        self.refresh_stored_session_count().await?;
        Ok(self
            .build_session_operation_outcome(SessionOperationAction::StartedFresh, None)
            .await)
    }

    pub(super) fn begin_active_turn(&self) -> Result<ActiveTurnGuard> {
        if self
            .runtime_turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(anyhow::anyhow!("another turn is already running"));
        }
        Ok(ActiveTurnGuard {
            active: self.runtime_turn_active.clone(),
        })
    }

    pub fn drain_events(&self) -> Vec<SessionEvent> {
        self.events.drain()
    }

    async fn resume_existing_agent_session(
        &self,
        agent_session_ref: &str,
    ) -> Result<SessionOperationOutcome> {
        let summary = self
            .resolve_agent_session_reference_from_operator_input(agent_session_ref)
            .await?;
        match &summary.resume_support {
            ResumeSupport::AttachedToActiveRuntime => {
                return Ok(self
                    .build_session_operation_outcome(
                        SessionOperationAction::AlreadyAttached,
                        Some(summary.agent_session_ref.clone()),
                    )
                    .await);
            }
            ResumeSupport::NotYetSupported { reason } => {
                return Err(anyhow::anyhow!(reason.clone()));
            }
            ResumeSupport::Reattachable => {}
        }

        let session_id = SessionId::from(summary.session_ref.clone());
        let target_agent_session_id = AgentSessionId::from(summary.agent_session_ref.clone());
        let events = self.store.events(&session_id).await?;
        let mut runtime_session =
            session_resume::reconstruct_runtime_session(&events, &target_agent_session_id)?;
        runtime_session.agent_session_id = AgentSessionId::new();
        self.activate_reconstructed_session(runtime_session).await?;
        Ok(self
            .build_session_operation_outcome(
                SessionOperationAction::Reattached,
                Some(summary.agent_session_ref.clone()),
            )
            .await)
    }

    pub async fn resume_persisted_session(&self, session_ref: &str) -> Result<()> {
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        if session_id.as_str() == self.startup_snapshot().active_session_ref {
            return Ok(());
        }
        let runtime_session = self
            .load_runtime_session_for_root_agent(&session_id)
            .await?;
        self.activate_reconstructed_session(runtime_session).await
    }

    pub async fn fork_persisted_session(&self, session_ref: &str) -> Result<()> {
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        let parent_session = self
            .load_runtime_session_for_root_agent(&session_id)
            .await?;
        let forked_session =
            fork_runtime_session(&parent_session, SessionId::new(), AgentSessionId::new());
        self.persist_forked_session_seed(&forked_session).await?;
        self.activate_reconstructed_session(forked_session).await
    }

    async fn active_visible_transcript(&self) -> Vec<Message> {
        self.runtime.lock().await.visible_transcript_snapshot()
    }

    pub(super) fn sync_runtime_session_refs(&self, runtime: &AgentRuntime) {
        self.set_runtime_session_refs(
            runtime.session_id().to_string(),
            runtime.agent_session_id().to_string(),
        );
    }

    fn set_runtime_session_refs(&self, session_ref: String, agent_session_ref: String) {
        let mut startup = self.startup.write().unwrap();
        startup.active_session_ref = session_ref;
        startup.root_agent_session_id = agent_session_ref;
    }

    async fn build_session_operation_outcome(
        &self,
        action: SessionOperationAction,
        requested_agent_session_ref: Option<String>,
    ) -> SessionOperationOutcome {
        let startup = self.startup_snapshot();
        let transcript = self.active_visible_transcript().await;
        SessionOperationOutcome {
            action,
            session_ref: startup.active_session_ref.clone(),
            active_agent_session_ref: startup.root_agent_session_id.clone(),
            requested_agent_session_ref,
            startup,
            transcript,
        }
    }

    async fn load_runtime_session_for_root_agent(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeSession> {
        let root_agent_session_id = self
            .store
            .agent_session_ids(session_id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                anyhow::anyhow!("session {session_id} has no persisted agent sessions")
            })?;
        let events = self.store.events(session_id).await?;
        let mut runtime_session =
            session_resume::reconstruct_runtime_session(&events, &root_agent_session_id)?;
        runtime_session.agent_session_id = AgentSessionId::new();
        Ok(runtime_session)
    }

    async fn activate_reconstructed_session(&self, runtime_session: RuntimeSession) -> Result<()> {
        let (active_session_ref, active_agent_session_ref, side_question_context) = {
            let mut runtime = self.runtime.lock().await;
            runtime
                .resume_session(runtime_session)
                .await
                .map_err(anyhow::Error::from)?;
            (
                runtime.session_id().to_string(),
                runtime.agent_session_id().to_string(),
                Self::side_question_context_from_runtime(&runtime, None),
            )
        };
        self.store_side_question_context(side_question_context.clone());
        self.reset_session_memory_refresh_state(&side_question_context)
            .await;
        self.worktree_manager
            .sync_attached_session(
                SessionId::from(active_session_ref.clone()),
                AgentSessionId::from(active_agent_session_ref.clone()),
            )
            .await
            .map_err(anyhow::Error::from)?;
        self.set_runtime_session_refs(active_session_ref, active_agent_session_ref);
        self.refresh_stored_session_count().await?;
        Ok(())
    }

    async fn persist_forked_session_seed(&self, session: &RuntimeSession) -> Result<()> {
        let events = seed_runtime_session_events(session).map_err(anyhow::Error::from)?;
        if events.is_empty() {
            return Ok(());
        }
        self.store.append_batch(events).await?;
        Ok(())
    }
}
