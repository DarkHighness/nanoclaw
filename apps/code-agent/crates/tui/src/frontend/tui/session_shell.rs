use super::*;

impl CodeAgentTui {
    pub(super) fn startup_state(&self) -> TuiState {
        self.startup_state_from_snapshot(&self.query(UIQuery::StartupSnapshot))
    }

    pub(super) fn startup_state_from_snapshot(
        &self,
        snapshot: &SessionStartupSnapshot,
    ) -> TuiState {
        let workspace_root = snapshot.workspace_root.clone();
        let input_history = input_history::load_input_history(&workspace_root);
        let mut state = TuiState {
            session: state::SessionSummary {
                workspace_name: snapshot.workspace_name.clone(),
                active_session_ref: snapshot.active_session_ref.clone(),
                root_agent_session_id: snapshot.root_agent_session_id.clone(),
                provider_label: snapshot.provider_label.clone(),
                model: snapshot.model.clone(),
                model_reasoning_effort: snapshot.model_reasoning_effort.clone(),
                supported_model_reasoning_efforts: snapshot
                    .supported_model_reasoning_efforts
                    .clone(),
                supports_image_input: snapshot.supports_image_input,
                workspace_root: workspace_root.clone(),
                git: state::git_snapshot(&workspace_root, snapshot.host_process_surfaces_allowed),
                tool_names: snapshot.tool_names.clone(),
                store_label: snapshot.store_label.clone(),
                store_warning: snapshot.store_warning.clone(),
                stored_session_count: snapshot.stored_session_count,
                default_sandbox_summary: snapshot.default_sandbox_summary.clone(),
                sandbox_summary: snapshot.sandbox_summary.clone(),
                permission_mode: snapshot.permission_mode,
                host_process_surfaces_allowed: snapshot.host_process_surfaces_allowed,
                startup_diagnostics: snapshot.startup_diagnostics.clone(),
                queued_commands: 0,
                token_ledger: Default::default(),
                statusline: snapshot.statusline.clone(),
            },
            theme: active_theme_id(),
            themes: crate::theme::theme_summaries(),
            status: "Ready for your next instruction".to_string(),
            follow_transcript: true,
            ..TuiState::default()
        };
        state.set_input_history(
            input_history.entries,
            input_history.prompts,
            input_history.commands,
        );
        state.push_activity("session ready");
        state
    }

    pub(super) fn sync_session_summary_from_snapshot(&mut self, snapshot: &SessionStartupSnapshot) {
        let git = state::git_snapshot(
            &snapshot.workspace_root,
            snapshot.host_process_surfaces_allowed,
        );
        self.ui_state.mutate(|state| {
            state.session.workspace_name = snapshot.workspace_name.clone();
            state.session.active_session_ref = snapshot.active_session_ref.clone();
            state.session.root_agent_session_id = snapshot.root_agent_session_id.clone();
            state.session.provider_label = snapshot.provider_label.clone();
            state.session.model = snapshot.model.clone();
            state.session.model_reasoning_effort = snapshot.model_reasoning_effort.clone();
            state.session.supported_model_reasoning_efforts =
                snapshot.supported_model_reasoning_efforts.clone();
            state.session.supports_image_input = snapshot.supports_image_input;
            state.session.workspace_root = snapshot.workspace_root.clone();
            state.session.git = git;
            state.session.tool_names = snapshot.tool_names.clone();
            state.session.store_label = snapshot.store_label.clone();
            state.session.store_warning = snapshot.store_warning.clone();
            state.session.stored_session_count = snapshot.stored_session_count;
            state.session.default_sandbox_summary = snapshot.default_sandbox_summary.clone();
            state.session.sandbox_summary = snapshot.sandbox_summary.clone();
            state.session.permission_mode = snapshot.permission_mode;
            state.session.host_process_surfaces_allowed = snapshot.host_process_surfaces_allowed;
            state.session.startup_diagnostics = snapshot.startup_diagnostics.clone();
            state.session.statusline = snapshot.statusline.clone();
        });
    }

    pub(super) fn replace_after_session_operation(
        &mut self,
        outcome: SessionOperationOutcome,
        dropped_commands: usize,
    ) {
        let aborted_operator_task = self.abort_operator_task();
        let previous = self.ui_state.snapshot();
        let show_tool_details = previous.show_tool_details;
        let statusline = previous.session.statusline.clone();
        let mut startup = self.startup_state_from_snapshot(&outcome.startup);
        startup.show_tool_details = show_tool_details;
        startup.session.statusline = statusline;
        startup.session.queued_commands = 0;
        startup.show_transcript_pane();
        startup.follow_transcript = true;
        startup.transcript_scroll = u16::MAX;

        match outcome.action {
            SessionOperationAction::StartedFresh => {
                startup.status = "Started new session".to_string();
                startup.push_activity(format!(
                    "started new session {}",
                    preview_id(&outcome.session_ref)
                ));
            }
            SessionOperationAction::AlreadyAttached => {
                startup.transcript = format_visible_transcript_lines(&outcome.transcript);
                let requested = outcome
                    .requested_agent_session_ref
                    .as_deref()
                    .unwrap_or(outcome.active_agent_session_ref.as_str());
                startup.inspector_title = "Resume".to_string();
                startup.inspector_scroll = 0;
                startup.inspector = format_session_operation_outcome(&outcome);
                startup.status = format!(
                    "Agent session {} is already attached",
                    preview_id(requested)
                );
                startup.push_activity(format!("resume no-op {}", preview_id(requested)));
            }
            SessionOperationAction::Reattached => {
                startup.transcript = format_visible_transcript_lines(&outcome.transcript);
                startup.inspector_title = "Resume".to_string();
                startup.inspector_scroll = 0;
                startup.inspector = format_session_operation_outcome(&outcome);
                startup.status = format!(
                    "Reattached session {} as {}",
                    preview_id(&outcome.session_ref),
                    preview_id(&outcome.active_agent_session_ref)
                );
                startup.push_activity(format!(
                    "resumed session {} as {}",
                    preview_id(&outcome.session_ref),
                    preview_id(&outcome.active_agent_session_ref)
                ));
            }
        }

        if dropped_commands > 0 {
            startup.push_activity(format!("discarded {} queued command(s)", dropped_commands));
        }
        if aborted_operator_task {
            startup.push_activity("aborted pending live-task operator wait after session switch");
        }
        self.ui_state.replace(startup);
    }

    pub(super) fn sync_runtime_control_state(&self) {
        let pending: Vec<crate::interaction::PendingControlSummary> =
            self.query(UIQuery::PendingControls);
        self.ui_state.mutate(|state| {
            state.session.queued_commands = pending.len();
            state.sync_pending_controls(pending);
        });
    }

    pub(super) fn apply_backend_events(&mut self) {
        for event in self
            .dispatch::<Vec<SessionEvent>>(UICommand::DrainEvents)
            .unwrap_or_default()
        {
            self.event_renderer.apply_event(event);
        }
    }

    pub(super) fn abort_operator_task(&mut self) -> bool {
        if let Some(task) = self.operator_task.take() {
            task.abort();
            true
        } else {
            false
        }
    }

    pub(super) fn abort_turn_task(&mut self) -> bool {
        if let Some(task) = self.turn_task.take() {
            task.abort();
            true
        } else {
            false
        }
    }
}
