use super::*;

/// Session memory maintenance is host policy, not generic runtime behavior.
/// Keep the note/episodic capture pipeline in one module so session control
/// code can trigger refreshes without owning the model prompts and file writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompactionWorkingSnapshot {
    pub(super) session_id: SessionId,
    pub(super) agent_session_id: AgentSessionId,
    pub(super) summary: String,
    pub(super) summary_message_id: MessageId,
}

#[derive(Clone, Debug)]
pub(super) struct SessionMemoryRefreshContext {
    pub(super) session_id: SessionId,
    pub(super) agent_session_id: AgentSessionId,
    pub(super) visible_transcript: Vec<Message>,
    pub(super) context_tokens: usize,
    pub(super) completed_turn_count: usize,
    pub(super) tool_call_count: usize,
    pub(super) compaction_summary_message_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
struct SessionMemoryRefreshJob {
    context: SessionMemoryRefreshContext,
    transcript_delta_text: String,
    epoch: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SessionEpisodicCaptureState {
    pub(super) active_session_id: Option<SessionId>,
    pub(super) capture_in_flight: bool,
    pub(super) capture_started_at: Option<Instant>,
    pub(super) capture_epoch: u64,
    pub(super) last_captured_message_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
struct SessionEpisodicCaptureJob {
    context: SessionMemoryRefreshContext,
    transcript_delta_text: String,
    epoch: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SideQuestionContextSnapshot {
    pub(super) session_id: SessionId,
    pub(super) agent_session_id: AgentSessionId,
    pub(super) instructions: Vec<String>,
    pub(super) transcript: Vec<Message>,
    pub(super) tools: Vec<ToolSpec>,
}

impl CodeAgentSession {
    pub(super) fn latest_compaction_working_snapshot(
        &self,
        runtime: &AgentRuntime,
        observer: &SessionEventObserver,
    ) -> Option<CompactionWorkingSnapshot> {
        let summary = observer.latest_compaction_summary()?;
        let summary_message_id = observer.latest_compaction_summary_message_id()?;
        Some(CompactionWorkingSnapshot {
            session_id: runtime.session_id(),
            agent_session_id: runtime.agent_session_id(),
            summary,
            summary_message_id,
        })
    }

    pub(super) fn side_question_context_from_runtime(
        runtime: &AgentRuntime,
        pending_prompt: Option<Message>,
    ) -> SideQuestionContextSnapshot {
        let mut transcript = runtime.visible_transcript_snapshot();
        if let Some(pending_prompt) = pending_prompt {
            transcript.push(pending_prompt);
        }
        SideQuestionContextSnapshot {
            session_id: runtime.session_id(),
            agent_session_id: runtime.agent_session_id(),
            instructions: runtime.base_instructions_snapshot(),
            transcript,
            tools: runtime.tool_specs(),
        }
    }

    pub(super) fn store_side_question_context(&self, context: SideQuestionContextSnapshot) {
        *self.side_question_context.write().unwrap() = Some(context);
    }

    pub(super) fn session_memory_refresh_context(
        &self,
        runtime: &AgentRuntime,
        observer: &SessionEventObserver,
    ) -> Option<SessionMemoryRefreshContext> {
        let completed_turn_count = observer.completed_turn_count();
        let compaction_summary_message_id = observer.latest_compaction_summary_message_id();
        if completed_turn_count == 0 && compaction_summary_message_id.is_none() {
            return None;
        }

        Some(SessionMemoryRefreshContext {
            session_id: runtime.session_id(),
            agent_session_id: runtime.agent_session_id(),
            visible_transcript: runtime.visible_transcript_snapshot(),
            context_tokens: runtime
                .token_ledger()
                .context_window
                .map(|usage| usage.used_tokens)
                .unwrap_or_default(),
            completed_turn_count,
            tool_call_count: observer.requested_tool_call_count(),
            compaction_summary_message_id,
        })
    }

    pub(super) async fn sync_session_memory_after_runtime_activity(
        &self,
        context: Option<SessionMemoryRefreshContext>,
        snapshot: Option<CompactionWorkingSnapshot>,
    ) {
        if snapshot.is_some() {
            self.persist_compaction_working_snapshot(snapshot).await;
        }
        let Some(context) = context else {
            return;
        };

        if let Some(summary_message_id) = context.compaction_summary_message_id.clone() {
            self.mark_session_memory_refreshed(&context, Some(summary_message_id));
        }
        if context.completed_turn_count > 0 {
            // Claude-style capture and structured session memory serve different
            // purposes: the daily log keeps append-only raw facts for later
            // consolidation, while the session note stays a bounded working
            // handoff. Run both in the background without merging them.
            self.maybe_capture_session_episodic_memory(context.clone());
            let force_refresh = context.compaction_summary_message_id.is_some();
            self.maybe_refresh_session_memory_note(context, force_refresh);
        }
    }

    pub(super) fn mark_session_memory_refreshed(
        &self,
        context: &SessionMemoryRefreshContext,
        up_to_message_id: Option<MessageId>,
    ) {
        self.mark_session_memory_refreshed_if_current(context, up_to_message_id, None);
    }

    fn mark_session_memory_refreshed_if_current(
        &self,
        context: &SessionMemoryRefreshContext,
        up_to_message_id: Option<MessageId>,
        expected_epoch: Option<u64>,
    ) {
        let last_message_id = up_to_message_id.or_else(|| {
            context
                .visible_transcript
                .last()
                .map(|message| message.message_id.clone())
        });
        let mut state = self
            .session_memory_refresh
            .lock()
            .expect("session memory refresh state");
        if state.active_session_id.as_ref() != Some(&context.session_id) {
            return;
        }
        if expected_epoch.is_some_and(|epoch| state.refresh_epoch != epoch) {
            return;
        }
        state.initialized = true;
        state.refresh_in_flight = false;
        state.refresh_started_at = None;
        state.tokens_at_last_update = context.context_tokens;
        state.tool_calls_since_update = 0;
        state.last_summarized_message_id = last_message_id;
    }

    fn clear_session_memory_refresh_in_flight(
        &self,
        context: &SessionMemoryRefreshContext,
        expected_epoch: Option<u64>,
    ) {
        let mut state = self
            .session_memory_refresh
            .lock()
            .expect("session memory refresh state");
        if state.active_session_id.as_ref() != Some(&context.session_id) {
            return;
        }
        if expected_epoch.is_some_and(|epoch| state.refresh_epoch != epoch) {
            return;
        }
        state.refresh_in_flight = false;
        state.refresh_started_at = None;
    }

    fn mark_session_episodic_capture_completed_if_current(
        &self,
        context: &SessionMemoryRefreshContext,
        expected_epoch: Option<u64>,
    ) {
        let last_message_id = context
            .visible_transcript
            .last()
            .map(|message| message.message_id.clone());
        let mut state = self
            .session_episodic_capture
            .lock()
            .expect("session episodic capture state");
        if state.active_session_id.as_ref() != Some(&context.session_id) {
            return;
        }
        if expected_epoch.is_some_and(|epoch| state.capture_epoch != epoch) {
            return;
        }
        state.capture_in_flight = false;
        state.capture_started_at = None;
        state.last_captured_message_id = last_message_id;
    }

    fn clear_session_episodic_capture_in_flight(
        &self,
        context: &SessionMemoryRefreshContext,
        expected_epoch: Option<u64>,
    ) {
        let mut state = self
            .session_episodic_capture
            .lock()
            .expect("session episodic capture state");
        if state.active_session_id.as_ref() != Some(&context.session_id) {
            return;
        }
        if expected_epoch.is_some_and(|epoch| state.capture_epoch != epoch) {
            return;
        }
        state.capture_in_flight = false;
        state.capture_started_at = None;
    }

    pub(super) fn maybe_capture_session_episodic_memory(
        &self,
        context: SessionMemoryRefreshContext,
    ) {
        if self.memory_backend.is_none() || self.session_memory_model_backend.is_none() {
            return;
        }

        let (last_captured_message_id, epoch): (Option<MessageId>, u64) = {
            let mut state = self
                .session_episodic_capture
                .lock()
                .expect("session episodic capture state");
            if state.active_session_id.as_ref() != Some(&context.session_id) {
                state.active_session_id = Some(context.session_id.clone());
                state.capture_in_flight = false;
                state.capture_started_at = None;
                state.capture_epoch = state.capture_epoch.wrapping_add(1);
                state.last_captured_message_id = None;
            }
            if state.capture_in_flight {
                let capture_is_stale =
                    state.capture_started_at.is_some_and(|started_at: Instant| {
                        started_at.elapsed()
                            >= Duration::from_millis(SESSION_MEMORY_STALE_THRESHOLD_MS)
                    });
                if !capture_is_stale {
                    return;
                }
                state.capture_in_flight = false;
                state.capture_started_at = None;
                state.capture_epoch = state.capture_epoch.wrapping_add(1);
            }
            state.capture_in_flight = true;
            state.capture_started_at = Some(Instant::now());
            state.capture_epoch = state.capture_epoch.wrapping_add(1);
            (state.last_captured_message_id.clone(), state.capture_epoch)
        };

        let transcript_delta = unsummarized_transcript_delta(
            &context.visible_transcript,
            last_captured_message_id.as_ref(),
        );
        if transcript_delta.is_empty() {
            self.mark_session_episodic_capture_completed_if_current(&context, Some(epoch));
            return;
        }
        let transcript_delta_text = render_session_memory_transcript_delta(&transcript_delta);
        if transcript_delta_text.trim().is_empty() {
            self.mark_session_episodic_capture_completed_if_current(&context, Some(epoch));
            return;
        }

        let session = self.clone();
        tokio::spawn(async move {
            session
                .run_session_episodic_capture_job(SessionEpisodicCaptureJob {
                    context,
                    transcript_delta_text,
                    epoch,
                })
                .await;
        });
    }

    pub(super) fn maybe_refresh_session_memory_note(
        &self,
        context: SessionMemoryRefreshContext,
        force_refresh: bool,
    ) {
        if self.memory_backend.is_none() || self.session_memory_model_backend.is_none() {
            return;
        }

        let (last_summarized_message_id, epoch) = {
            let mut state = self
                .session_memory_refresh
                .lock()
                .expect("session memory refresh state");
            if state.active_session_id.as_ref() != Some(&context.session_id) {
                state.active_session_id = Some(context.session_id.clone());
                state.initialized = false;
                state.refresh_in_flight = false;
                state.refresh_started_at = None;
                state.tokens_at_last_update = 0;
                state.tool_calls_since_update = 0;
                state.last_summarized_message_id = None;
                state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
            }
            state.tool_calls_since_update = state
                .tool_calls_since_update
                .saturating_add(context.tool_call_count);
            if state.refresh_in_flight {
                let refresh_is_stale = state.refresh_started_at.is_some_and(|started_at| {
                    started_at.elapsed() >= Duration::from_millis(SESSION_MEMORY_STALE_THRESHOLD_MS)
                });
                if !refresh_is_stale {
                    return;
                }
                state.refresh_in_flight = false;
                state.refresh_started_at = None;
                state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
            }
            let should_refresh = if force_refresh {
                true
            } else if !state.initialized {
                context.context_tokens >= SESSION_MEMORY_MIN_TOKENS_TO_INIT
            } else {
                context
                    .context_tokens
                    .saturating_sub(state.tokens_at_last_update)
                    >= SESSION_MEMORY_MIN_TOKENS_BETWEEN_UPDATES
                    || state.tool_calls_since_update >= SESSION_MEMORY_TOOL_CALLS_BETWEEN_UPDATES
            };
            if !should_refresh {
                return;
            }
            state.refresh_in_flight = true;
            state.refresh_started_at = Some(Instant::now());
            state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
            (
                state.last_summarized_message_id.clone(),
                state.refresh_epoch,
            )
        };

        let transcript_delta = unsummarized_transcript_delta(
            &context.visible_transcript,
            last_summarized_message_id.as_ref(),
        );
        if transcript_delta.is_empty() {
            self.mark_session_memory_refreshed_if_current(&context, None, Some(epoch));
            return;
        }
        let transcript_delta_text = render_session_memory_transcript_delta(&transcript_delta);
        if transcript_delta_text.trim().is_empty() {
            self.mark_session_memory_refreshed_if_current(&context, None, Some(epoch));
            return;
        }

        let session = self.clone();
        tokio::spawn(async move {
            session
                .run_session_memory_refresh_job(SessionMemoryRefreshJob {
                    context,
                    transcript_delta_text,
                    epoch,
                })
                .await;
        });
    }

    async fn run_session_memory_refresh_job(&self, job: SessionMemoryRefreshJob) {
        let Some(memory_backend) = self.memory_backend.as_ref() else {
            self.clear_session_memory_refresh_in_flight(&job.context, Some(job.epoch));
            return;
        };
        let Some(model_backend) = self.session_memory_model_backend.as_ref() else {
            self.clear_session_memory_refresh_in_flight(&job.context, Some(job.epoch));
            return;
        };

        let current_note = self
            .load_session_memory_note_body(&job.context.session_id)
            .await
            .unwrap_or_else(|_| default_session_memory_note());
        let prompt =
            build_session_memory_update_prompt(&current_note, job.transcript_delta_text.as_str());
        let updated = match self
            .run_session_memory_update(
                model_backend.as_ref(),
                &job.context,
                Message::user(prompt),
                Vec::new(),
            )
            .await
        {
            Ok(updated) => updated,
            Err(error) => {
                self.clear_session_memory_refresh_in_flight(&job.context, Some(job.epoch));
                warn!(error = %error, "failed to refresh structured session memory note");
                return;
            }
        };

        let rendered = render_session_memory_note(&updated);
        if let Err(error) = self
            .write_session_memory_note(
                memory_backend.as_ref(),
                &job.context.session_id,
                &job.context.agent_session_id,
                rendered,
                job.context
                    .visible_transcript
                    .last()
                    .map(|message| message.message_id.clone()),
                vec!["session-note".to_string(), "incremental".to_string()],
            )
            .await
        {
            self.clear_session_memory_refresh_in_flight(&job.context, Some(job.epoch));
            warn!(error = %error, "failed to persist refreshed structured session memory note");
            return;
        }

        self.mark_session_memory_refreshed_if_current(&job.context, None, Some(job.epoch));
    }

    async fn run_session_episodic_capture_job(&self, job: SessionEpisodicCaptureJob) {
        let Some(memory_backend) = self.memory_backend.as_ref() else {
            self.clear_session_episodic_capture_in_flight(&job.context, Some(job.epoch));
            return;
        };
        let Some(model_backend) = self.session_memory_model_backend.as_ref() else {
            self.clear_session_episodic_capture_in_flight(&job.context, Some(job.epoch));
            return;
        };

        let prompt = build_session_episodic_capture_prompt(&job.transcript_delta_text);
        let response = match self
            .run_session_episodic_capture(
                model_backend.as_ref(),
                &job.context,
                Message::user(prompt),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.clear_session_episodic_capture_in_flight(&job.context, Some(job.epoch));
                warn!(error = %error, "failed to capture episodic daily-log memory");
                return;
            }
        };

        let entries = parse_session_episodic_capture_entries(&response);
        if entries.is_empty() {
            self.mark_session_episodic_capture_completed_if_current(&job.context, Some(job.epoch));
            return;
        }

        if let Err(error) = self
            .write_session_episodic_daily_log(memory_backend.as_ref(), &job.context, &entries)
            .await
        {
            self.clear_session_episodic_capture_in_flight(&job.context, Some(job.epoch));
            warn!(error = %error, "failed to persist episodic daily-log memory");
            return;
        }

        self.mark_session_episodic_capture_completed_if_current(&job.context, Some(job.epoch));
    }

    async fn load_session_memory_note_body(&self, session_id: &SessionId) -> Result<String> {
        let path = session_memory_note_absolute_path(self.workspace_root(), session_id);
        match fs::read_to_string(path).await {
            Ok(text) => Ok(parse_session_memory_note_snapshot(&text).body),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(default_session_memory_note())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn run_session_memory_update(
        &self,
        backend: &dyn ModelBackend,
        context: &SessionMemoryRefreshContext,
        prompt: Message,
        tools: Vec<ToolSpec>,
    ) -> Result<String> {
        let request = ModelRequest {
            session_id: context.session_id.clone(),
            agent_session_id: context.agent_session_id.clone(),
            turn_id: TurnId::new(),
            instructions: Vec::new(),
            messages: vec![prompt],
            tools,
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({ "code_agent": { "purpose": "session_memory_update" } }),
        };
        let mut stream = timeout(
            Duration::from_millis(SESSION_MEMORY_UPDATE_TIMEOUT_MS),
            backend.stream_turn(request),
        )
        .await
        .map_err(|_| anyhow::anyhow!("session memory update timed out before model start"))??;

        let mut text = String::new();
        while let Some(event) = timeout(
            Duration::from_millis(SESSION_MEMORY_UPDATE_TIMEOUT_MS),
            stream.next(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("session memory update timed out while streaming"))?
        {
            match event? {
                ModelEvent::TextDelta { delta } => text.push_str(&delta),
                ModelEvent::ResponseComplete { .. } => {}
                ModelEvent::ToolCallRequested { call } => {
                    return Err(anyhow::anyhow!(
                        "session memory update unexpectedly requested tool `{}`",
                        call.tool_name
                    ));
                }
                ModelEvent::Error { message } => {
                    return Err(anyhow::anyhow!(message));
                }
            }
        }

        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!(
                "session memory update returned empty output"
            ));
        }
        Ok(trimmed)
    }

    async fn run_session_episodic_capture(
        &self,
        backend: &dyn ModelBackend,
        context: &SessionMemoryRefreshContext,
        prompt: Message,
    ) -> Result<String> {
        let request = ModelRequest {
            session_id: context.session_id.clone(),
            agent_session_id: context.agent_session_id.clone(),
            turn_id: TurnId::new(),
            instructions: Vec::new(),
            messages: vec![prompt],
            tools: Vec::new(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({ "code_agent": { "purpose": "session_episodic_capture" } }),
        };
        let mut stream = timeout(
            Duration::from_millis(SESSION_MEMORY_UPDATE_TIMEOUT_MS),
            backend.stream_turn(request),
        )
        .await
        .map_err(|_| anyhow::anyhow!("session episodic capture timed out before model start"))??;

        let mut text = String::new();
        while let Some(event) = timeout(
            Duration::from_millis(SESSION_MEMORY_UPDATE_TIMEOUT_MS),
            stream.next(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("session episodic capture timed out while streaming"))?
        {
            match event? {
                ModelEvent::TextDelta { delta } => text.push_str(&delta),
                ModelEvent::ResponseComplete { .. } => {}
                ModelEvent::ToolCallRequested { call } => {
                    return Err(anyhow::anyhow!(
                        "session episodic capture unexpectedly requested tool `{}`",
                        call.tool_name
                    ));
                }
                ModelEvent::Error { message } => {
                    return Err(anyhow::anyhow!(message));
                }
            }
        }

        Ok(text.trim().to_string())
    }

    async fn write_session_memory_note(
        &self,
        memory_backend: &dyn MemoryBackend,
        session_id: &SessionId,
        agent_session_id: &AgentSessionId,
        note: String,
        last_summarized_message_id: Option<MessageId>,
        tags: Vec<String>,
    ) -> Result<()> {
        memory_backend
            .record(MemoryRecordRequest {
                scope: MemoryScope::Working,
                title: "Session continuation snapshot".to_string(),
                content: note,
                mode: MemoryRecordMode::Replace,
                memory_type: Some(MemoryType::Project),
                description: Some(
                    "Latest structured session note for the current runtime session.".to_string(),
                ),
                layer: Some("session".to_string()),
                tags,
                session_id: Some(session_id.clone()),
                agent_session_id: Some(agent_session_id.clone()),
                agent_name: None,
                task_id: None,
            })
            .await?;
        // The generic memory backend owns note file writes, but the session
        // continuity boundary is host-specific. Patch the same file's
        // frontmatter immediately after the managed write so resume and future
        // compaction decisions read one durable source of truth.
        let path = session_memory_note_absolute_path(self.workspace_root(), session_id);
        let text = fs::read_to_string(&path).await?;
        let patched =
            upsert_session_memory_note_frontmatter(&text, last_summarized_message_id.as_ref());
        if patched != text {
            fs::write(path, patched).await?;
        }
        Ok(())
    }

    async fn write_session_episodic_daily_log(
        &self,
        memory_backend: &dyn MemoryBackend,
        context: &SessionMemoryRefreshContext,
        entries: &[String],
    ) -> Result<()> {
        let content = entries
            .iter()
            .map(|entry| format!("- {}", entry.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        memory_backend
            .record(MemoryRecordRequest {
                scope: MemoryScope::Episodic,
                title: "Session daily log capture".to_string(),
                content,
                mode: MemoryRecordMode::Append,
                memory_type: None,
                description: Some(
                    "Append-only episodic capture for later memory consolidation.".to_string(),
                ),
                layer: Some("daily-log".to_string()),
                tags: vec!["daily-log".to_string(), "session-capture".to_string()],
                session_id: Some(context.session_id.clone()),
                agent_session_id: Some(context.agent_session_id.clone()),
                agent_name: None,
                task_id: None,
            })
            .await?;
        Ok(())
    }

    async fn run_side_question(
        &self,
        backend: &dyn ModelBackend,
        snapshot: &SideQuestionContextSnapshot,
        prompt: Message,
    ) -> Result<String> {
        let mut messages = snapshot.transcript.clone();
        messages.push(prompt);
        let request = ModelRequest {
            session_id: snapshot.session_id.clone(),
            agent_session_id: snapshot.agent_session_id.clone(),
            turn_id: TurnId::new(),
            instructions: snapshot.instructions.clone(),
            messages,
            // Keep the tool surface identical to the parent context so the
            // cacheable prefix stays close to the main request, then block any
            // attempted tool call at execution time.
            tools: snapshot.tools.clone(),
            additional_context: Vec::new(),
            continuation: None,
            metadata: json!({ "code_agent": { "purpose": "side_question" } }),
        };
        let mut stream = timeout(
            Duration::from_millis(SESSION_MEMORY_UPDATE_TIMEOUT_MS),
            backend.stream_turn(request),
        )
        .await
        .map_err(|_| anyhow::anyhow!("side question timed out before model start"))??;

        let mut text = String::new();
        while let Some(event) = timeout(
            Duration::from_millis(SESSION_MEMORY_UPDATE_TIMEOUT_MS),
            stream.next(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("side question timed out while streaming"))?
        {
            match event? {
                ModelEvent::TextDelta { delta } => text.push_str(&delta),
                ModelEvent::ResponseComplete { .. } => {}
                ModelEvent::ToolCallRequested { call } => {
                    return Err(anyhow::anyhow!(
                        "side question unexpectedly requested tool `{}`",
                        call.tool_name
                    ));
                }
                ModelEvent::Error { message } => {
                    return Err(anyhow::anyhow!(message));
                }
            }
        }

        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("side question returned an empty response"));
        }
        Ok(trimmed)
    }

    pub(super) async fn reset_session_memory_refresh_state(
        &self,
        context: &SideQuestionContextSnapshot,
    ) {
        let note_path =
            session_memory_note_absolute_path(self.workspace_root(), &context.session_id);
        let note_text = fs::read_to_string(&note_path).await.ok();
        let note_snapshot = note_text
            .as_deref()
            .map(parse_session_memory_note_snapshot)
            .filter(|snapshot| !snapshot.body.is_empty());
        let mut state = self
            .session_memory_refresh
            .lock()
            .expect("session memory refresh state");
        state.active_session_id = Some(context.session_id.clone());
        state.initialized = note_snapshot.is_some();
        state.refresh_in_flight = false;
        state.refresh_started_at = None;
        state.refresh_epoch = state.refresh_epoch.wrapping_add(1);
        state.tokens_at_last_update = 0;
        state.tool_calls_since_update = 0;
        state.last_summarized_message_id =
            note_snapshot.and_then(|snapshot| snapshot.last_summarized_message_id);
        drop(state);

        let mut capture_state = self
            .session_episodic_capture
            .lock()
            .expect("session episodic capture state");
        capture_state.active_session_id = Some(context.session_id.clone());
        capture_state.capture_in_flight = false;
        capture_state.capture_started_at = None;
        capture_state.capture_epoch = capture_state.capture_epoch.wrapping_add(1);
        capture_state.last_captured_message_id = context
            .transcript
            .last()
            .map(|message| message.message_id.clone());
    }

    pub(super) async fn persist_compaction_working_snapshot(
        &self,
        snapshot: Option<CompactionWorkingSnapshot>,
    ) {
        let Some(memory_backend) = self.memory_backend.as_ref() else {
            return;
        };
        let Some(snapshot) = snapshot else {
            return;
        };
        let summary = snapshot.summary.trim();
        if summary.is_empty() {
            return;
        }
        let note = render_session_memory_note(summary);

        // Persist the latest compaction handoff as working memory so later
        // recall can recover session continuity without mutating base prompts.
        // The host renders a stable Claude-style note skeleton here so future
        // updates replace section content instead of drifting into ad hoc
        // compaction-specific Markdown shapes.
        if let Err(error) = self
            .write_session_memory_note(
                memory_backend.as_ref(),
                &snapshot.session_id,
                &snapshot.agent_session_id,
                note,
                Some(snapshot.summary_message_id.clone()),
                vec![
                    "compaction".to_string(),
                    "continuation".to_string(),
                    "session-note".to_string(),
                ],
            )
            .await
        {
            warn!(error = %error, "failed to persist working memory snapshot after compaction");
        }
    }

    pub async fn answer_side_question(&self, question: &str) -> Result<SideQuestionOutcome> {
        let Some(model_backend) = self.session_memory_model_backend.as_ref() else {
            return Err(anyhow::anyhow!(
                "side questions are unavailable without a model backend"
            ));
        };
        let snapshot = self
            .side_question_context
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("side question context is unavailable"))?;
        let prompt = wrap_side_question(question);
        let response = self
            .run_side_question(model_backend.as_ref(), &snapshot, Message::user(prompt))
            .await?;
        Ok(SideQuestionOutcome {
            question: question.trim().to_string(),
            response,
        })
    }
}

fn unsummarized_transcript_delta(
    transcript: &[Message],
    last_summarized_message_id: Option<&MessageId>,
) -> Vec<Message> {
    let start_index = last_summarized_message_id
        .and_then(|message_id| {
            transcript
                .iter()
                .position(|message| &message.message_id == message_id)
                .map(|index| index + 1)
        })
        .unwrap_or(0);
    transcript[start_index..]
        .iter()
        .filter(|message| {
            !message
                .metadata
                .contains_key(WORKSPACE_MEMORY_RECALL_METADATA_KEY)
        })
        .cloned()
        .collect()
}

fn render_session_memory_transcript_delta(messages: &[Message]) -> String {
    messages
        .iter()
        .map(session_history::message_to_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn wrap_side_question(question: &str) -> String {
    format!(
        concat!(
            "<system-reminder>This is a side question from the user. Answer it directly in one response.\n\n",
            "IMPORTANT CONTEXT:\n",
            "- You are a separate lightweight query that must not interrupt the main work.\n",
            "- You share the current conversation context but are not continuing the main task.\n",
            "- Do not say you are interrupted or that you will go do more work later.\n\n",
            "CRITICAL CONSTRAINTS:\n",
            "- Do not call tools.\n",
            "- Do not promise follow-up actions.\n",
            "- If the answer is not available from the current context, say so plainly.\n",
            "- Keep the answer focused on the side question itself.</system-reminder>\n\n",
            "{question}"
        ),
        question = question.trim(),
    )
}
