use super::*;

impl CodeAgentTui {
    pub(super) fn query<T: UIQueryValue>(&self, query: UIQuery) -> T {
        self.session.query(query)
    }

    pub(super) fn dispatch<T: UIResultValue>(&self, command: UICommand) -> Result<T> {
        self.session.dispatch(command)
    }

    pub(super) async fn run_ui<T: UIAsyncValue>(&self, command: UIAsyncCommand) -> Result<T> {
        self.session.run(command).await
    }

    pub(super) fn workspace_root_buf(&self) -> std::path::PathBuf {
        self.query(UIQuery::WorkspaceRoot)
    }

    pub(super) fn startup_snapshot(&self) -> SessionStartupSnapshot {
        self.query(UIQuery::StartupSnapshot)
    }

    pub(super) fn host_process_surfaces_allowed(&self) -> bool {
        self.query(UIQuery::HostProcessSurfacesAllowed)
    }

    pub(super) fn approval_prompt(&self) -> Option<ApprovalPrompt> {
        self.query(UIQuery::ApprovalPrompt)
    }

    pub(super) fn permission_request_prompt(&self) -> Option<PermissionRequestPrompt> {
        self.query(UIQuery::PermissionRequestPrompt)
    }

    pub(super) fn user_input_prompt(&self) -> Option<UserInputPrompt> {
        self.query(UIQuery::UserInputPrompt)
    }

    pub(super) fn pending_controls(&self) -> Vec<crate::interaction::PendingControlSummary> {
        self.query(UIQuery::PendingControls)
    }

    pub(super) fn queued_command_count(&self) -> usize {
        self.query(UIQuery::QueuedCommandCount)
    }

    pub(super) fn startup_diagnostics(&self) -> StartupDiagnosticsSnapshot {
        self.query(UIQuery::StartupDiagnostics)
    }

    pub(super) fn permission_grant_profiles(&self) -> (PermissionProfile, PermissionProfile) {
        self.query(UIQuery::PermissionGrantProfiles)
    }

    pub(super) fn skills(&self) -> Vec<crate::interaction::SkillSummary> {
        self.query(UIQuery::Skills)
    }

    pub(super) fn resolve_approval(&self, decision: crate::interaction::ApprovalDecision) -> bool {
        self.dispatch(UICommand::ResolveApproval(decision))
            .unwrap_or(false)
    }

    pub(super) fn resolve_permission_request(&self, decision: PermissionRequestDecision) -> bool {
        self.dispatch(UICommand::ResolvePermissionRequest(decision))
            .unwrap_or(false)
    }

    pub(super) fn resolve_user_input(&self, submission: UserInputSubmission) -> bool {
        self.dispatch(UICommand::ResolveUserInput(submission))
            .unwrap_or(false)
    }

    pub(super) fn cancel_user_input(&self, reason: impl Into<String>) -> bool {
        self.dispatch(UICommand::CancelUserInput {
            reason: reason.into(),
        })
        .unwrap_or(false)
    }

    pub(super) fn remove_pending_control(
        &self,
        control_ref: &str,
    ) -> Result<crate::interaction::PendingControlSummary> {
        self.dispatch(UICommand::RemovePendingControl {
            control_ref: control_ref.to_string(),
        })
    }

    pub(super) fn update_pending_control(
        &self,
        control_ref: &str,
        content: &str,
    ) -> Result<crate::interaction::PendingControlSummary> {
        self.dispatch(UICommand::UpdatePendingControl {
            control_ref: control_ref.to_string(),
            content: content.to_string(),
        })
    }

    pub(super) fn schedule_runtime_steer(
        &self,
        message: impl Into<String>,
        reason: Option<crate::interaction::PendingControlReason>,
    ) -> Result<String> {
        self.dispatch(UICommand::ScheduleRuntimeSteer {
            message: message.into(),
            reason: reason.map(|value| value.runtime_value()),
        })
    }

    pub(super) fn take_pending_steers(
        &self,
    ) -> Result<Vec<crate::interaction::PendingControlSummary>> {
        self.dispatch(UICommand::TakePendingSteers)
    }

    pub(super) fn cycle_model_reasoning_effort_result(
        &self,
    ) -> Result<ModelReasoningEffortOutcome> {
        self.dispatch(UICommand::CycleModelReasoningEffort)
    }

    pub(super) fn set_model_reasoning_effort_result(
        &self,
        effort: &str,
    ) -> Result<ModelReasoningEffortOutcome> {
        self.dispatch(UICommand::SetModelReasoningEffort {
            effort: effort.to_string(),
        })
    }

    pub(super) fn schedule_live_task_attention(
        &self,
        outcome: &LiveTaskWaitOutcome,
        turn_running: bool,
    ) -> Result<LiveTaskAttentionOutcome> {
        self.dispatch(UICommand::ScheduleLiveTaskAttention {
            outcome: outcome.clone(),
            turn_running,
        })
    }

    pub(super) async fn refresh_stored_session_count(&self) -> Result<usize> {
        self.run_ui(UIAsyncCommand::RefreshStoredSessionCount).await
    }
}
