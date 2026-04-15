use super::*;

/// Session memory maintenance is host policy, not generic runtime behavior.
/// Keep the compaction-driven working-note path in one module so session
/// control code can trigger refreshes without owning the model prompts and
/// file writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CompactionWorkingSnapshot {
    pub(super) session_id: SessionId,
    pub(super) agent_session_id: AgentSessionId,
    pub(super) summary: String,
    pub(super) summary_message_id: MessageId,
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
    fn memory_backend_snapshot(&self) -> Option<Arc<dyn MemoryBackend>> {
        self.memory_backend.read().unwrap().clone()
    }

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

    pub(super) async fn sync_session_memory_after_runtime_activity(
        &self,
        snapshot: Option<CompactionWorkingSnapshot>,
    ) {
        self.persist_compaction_working_snapshot(snapshot).await;
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
        persist_session_memory_note(
            self.workspace_root(),
            memory_backend,
            session_id,
            agent_session_id,
            note,
            last_summarized_message_id.as_ref(),
            tags,
        )
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
        let note_snapshot =
            load_session_memory_note_snapshot(self.workspace_root(), &context.session_id)
                .await
                .ok()
                .flatten()
                .filter(|snapshot| !snapshot.body.is_empty());
        let mut state = self
            .session_memory_refresh
            .lock()
            .expect("session memory refresh state");
        state.active_session_id = Some(context.session_id.clone());
        state.initialized = note_snapshot.is_some();
        state.last_summarized_message_id =
            note_snapshot.and_then(|snapshot| snapshot.last_summarized_message_id);
    }

    pub(super) async fn persist_compaction_working_snapshot(
        &self,
        snapshot: Option<CompactionWorkingSnapshot>,
    ) {
        let Some(memory_backend) = self.memory_backend_snapshot() else {
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
            return;
        }

        let mut state = self
            .session_memory_refresh
            .lock()
            .expect("session memory refresh state");
        state.active_session_id = Some(snapshot.session_id.clone());
        state.initialized = true;
        state.last_summarized_message_id = Some(snapshot.summary_message_id);
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
