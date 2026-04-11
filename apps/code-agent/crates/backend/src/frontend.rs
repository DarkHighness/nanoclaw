use crate::backend::{
    ApprovalDecision, ApprovalPrompt, CodeAgentSession, HistoryRollbackOutcome,
    HistoryRollbackRound, LiveTaskAttentionOutcome, LiveTaskControlOutcome, LiveTaskMessageOutcome,
    LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome, LoadedAgentSession,
    LoadedMcpPrompt, LoadedMcpResource, LoadedSession, McpPromptSummary, McpResourceSummary,
    McpServerSummary, ModelReasoningEffortOutcome, PendingControlSummary, PermissionRequestPrompt,
    PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
    SessionEvent, SessionOperation, SessionOperationOutcome, SessionPermissionMode,
    SessionPermissionModeOutcome, SessionStartupSnapshot, SideQuestionOutcome,
    StartupDiagnosticsSnapshot, UserInputPrompt,
};
use agent::runtime::{PermissionGrantSnapshot, Result as RuntimeResult, RunTurnOutcome};
use agent::tools::{GrantedPermissionResponse, RequestPermissionProfile, UserInputResponse};
use agent::types::{Message, SubmittedPromptSnapshot};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// The TUI only talks to this facade. Keeping a host-facing adapter here makes
/// the frontend boundary explicit and prevents the terminal shell from growing
/// implicit dependencies on backend internals by importing `CodeAgentSession`
/// directly.
#[derive(Clone)]
pub struct CodeAgentFrontendSession {
    inner: CodeAgentSession,
}

impl From<CodeAgentSession> for CodeAgentFrontendSession {
    fn from(inner: CodeAgentSession) -> Self {
        Self { inner }
    }
}

impl CodeAgentFrontendSession {
    pub fn workspace_root(&self) -> &Path {
        self.inner.workspace_root()
    }

    pub fn workspace_root_buf(&self) -> PathBuf {
        self.inner.workspace_root().to_path_buf()
    }

    pub fn startup_snapshot(&self) -> SessionStartupSnapshot {
        self.inner.startup_snapshot()
    }

    pub fn host_process_surfaces_allowed(&self) -> bool {
        self.inner.host_process_surfaces_allowed()
    }

    pub fn approval_prompt(&self) -> Option<ApprovalPrompt> {
        self.inner.approval_prompt()
    }

    pub fn resolve_approval(&self, decision: ApprovalDecision) -> bool {
        self.inner.resolve_approval(decision)
    }

    pub fn permission_request_prompt(&self) -> Option<PermissionRequestPrompt> {
        self.inner.permission_request_prompt()
    }

    pub fn resolve_permission_request(&self, response: GrantedPermissionResponse) -> bool {
        self.inner.resolve_permission_request(response)
    }

    pub fn user_input_prompt(&self) -> Option<UserInputPrompt> {
        self.inner.user_input_prompt()
    }

    pub fn resolve_user_input(&self, response: UserInputResponse) -> bool {
        self.inner.resolve_user_input(response)
    }

    pub fn cancel_user_input(&self, reason: impl Into<String>) -> bool {
        self.inner.cancel_user_input(reason)
    }

    pub fn pending_controls(&self) -> Vec<PendingControlSummary> {
        self.inner.pending_controls()
    }

    pub fn queued_command_count(&self) -> usize {
        self.inner.queued_command_count()
    }

    pub fn update_pending_control(
        &self,
        control_ref: &str,
        content: &str,
    ) -> Result<PendingControlSummary> {
        self.inner.update_pending_control(control_ref, content)
    }

    pub fn remove_pending_control(&self, control_ref: &str) -> Result<PendingControlSummary> {
        self.inner.remove_pending_control(control_ref)
    }

    pub fn schedule_runtime_steer(
        &self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> Result<String> {
        self.inner.schedule_runtime_steer(message, reason)
    }

    pub fn take_pending_steers(&self) -> Result<Vec<PendingControlSummary>> {
        self.inner.take_pending_steers()
    }

    pub fn cycle_model_reasoning_effort(&self) -> Result<ModelReasoningEffortOutcome> {
        self.inner.cycle_model_reasoning_effort()
    }

    pub fn set_model_reasoning_effort(&self, effort: &str) -> Result<ModelReasoningEffortOutcome> {
        self.inner.set_model_reasoning_effort(effort)
    }

    pub fn skills(&self) -> &[agent::Skill] {
        self.inner.skills()
    }

    pub fn startup_diagnostics(&self) -> StartupDiagnosticsSnapshot {
        self.inner.startup_diagnostics()
    }

    pub fn permission_grant_snapshot(&self) -> PermissionGrantSnapshot {
        self.inner.permission_grant_snapshot()
    }

    pub fn permission_grant_profiles(
        &self,
    ) -> (RequestPermissionProfile, RequestPermissionProfile) {
        self.inner.permission_grant_profiles()
    }

    pub fn drain_events(&self) -> Vec<SessionEvent> {
        self.inner.drain_events()
    }

    pub async fn end_session(&self, reason: Option<String>) -> RuntimeResult<()> {
        self.inner.end_session(reason).await
    }

    pub async fn apply_control(&self, command: agent::RuntimeCommand) -> Result<()> {
        self.inner.apply_control(command).await
    }

    pub async fn queue_prompt_command(
        &self,
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    ) -> Result<String> {
        self.inner
            .queue_prompt_command(message, submitted_prompt)
            .await
    }

    pub async fn clear_queued_commands(&self) -> usize {
        self.inner.clear_queued_commands().await
    }

    pub async fn drain_queued_controls(&self) -> Result<bool> {
        self.inner.drain_queued_controls().await
    }

    pub async fn run_one_shot_prompt(&self, prompt: &str) -> Result<RunTurnOutcome> {
        self.inner.run_one_shot_prompt(prompt).await
    }

    pub async fn rollback_visible_history_to_message(
        &self,
        message_id: &str,
    ) -> Result<HistoryRollbackOutcome> {
        self.inner
            .rollback_visible_history_to_message(message_id)
            .await
    }

    pub async fn history_rollback_rounds(&self) -> Vec<HistoryRollbackRound> {
        self.inner.history_rollback_rounds().await
    }

    pub async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        self.inner.compact_now(notes).await
    }

    pub async fn apply_session_operation(
        &self,
        operation: SessionOperation,
    ) -> Result<SessionOperationOutcome> {
        self.inner.apply_session_operation(operation).await
    }

    pub async fn set_permission_mode(
        &self,
        mode: SessionPermissionMode,
    ) -> Result<SessionPermissionModeOutcome> {
        self.inner.set_permission_mode(mode).await
    }

    pub async fn list_sessions(&self) -> Result<Vec<PersistedSessionSummary>> {
        self.inner.list_sessions().await
    }

    pub async fn search_sessions(&self, query: &str) -> Result<Vec<PersistedSessionSearchMatch>> {
        self.inner.search_sessions(query).await
    }

    pub async fn list_agent_sessions(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<PersistedAgentSessionSummary>> {
        self.inner.list_agent_sessions(session_ref).await
    }

    pub async fn list_tasks(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<crate::backend::PersistedTaskSummary>> {
        self.inner.list_tasks(session_ref).await
    }

    pub async fn list_live_tasks(&self) -> Result<Vec<LiveTaskSummary>> {
        self.inner.list_live_tasks().await
    }

    pub async fn spawn_live_task(&self, role: &str, prompt: &str) -> Result<LiveTaskSpawnOutcome> {
        self.inner.spawn_live_task(role, prompt).await
    }

    pub async fn send_live_task(
        &self,
        task_or_agent_ref: &str,
        message: &str,
    ) -> Result<LiveTaskMessageOutcome> {
        self.inner.send_live_task(task_or_agent_ref, message).await
    }

    pub async fn wait_live_task(&self, task_or_agent_ref: &str) -> Result<LiveTaskWaitOutcome> {
        self.inner.wait_live_task(task_or_agent_ref).await
    }

    pub fn schedule_live_task_attention(
        &self,
        outcome: &LiveTaskWaitOutcome,
        turn_running: bool,
    ) -> Result<LiveTaskAttentionOutcome> {
        self.inner
            .schedule_live_task_attention(outcome, turn_running)
    }

    pub async fn cancel_live_task(
        &self,
        task_or_agent_ref: &str,
        reason: Option<String>,
    ) -> Result<LiveTaskControlOutcome> {
        self.inner.cancel_live_task(task_or_agent_ref, reason).await
    }

    pub async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        self.inner.load_session(session_ref).await
    }

    pub async fn load_agent_session(&self, agent_session_ref: &str) -> Result<LoadedAgentSession> {
        self.inner.load_agent_session(agent_session_ref).await
    }

    pub async fn load_task(&self, task_ref: &str) -> Result<crate::backend::LoadedTask> {
        self.inner.load_task(task_ref).await
    }

    pub async fn export_session(
        &self,
        session_ref: &str,
        path: impl AsRef<Path>,
    ) -> Result<crate::backend::SessionExportArtifact> {
        let path = path.as_ref().to_string_lossy().into_owned();
        self.inner.export_session(session_ref, &path).await
    }

    pub async fn export_session_transcript(
        &self,
        session_ref: &str,
        path: impl AsRef<Path>,
    ) -> Result<crate::backend::SessionExportArtifact> {
        let path = path.as_ref().to_string_lossy().into_owned();
        self.inner
            .export_session_transcript(session_ref, &path)
            .await
    }

    pub async fn refresh_stored_session_count(&self) -> Result<usize> {
        self.inner.refresh_stored_session_count().await
    }

    pub async fn active_visible_transcript(&self) -> Vec<Message> {
        self.inner.active_visible_transcript().await
    }

    pub async fn answer_side_question(&self, question: &str) -> Result<SideQuestionOutcome> {
        self.inner.answer_side_question(question).await
    }

    pub async fn list_mcp_servers(&self) -> Vec<McpServerSummary> {
        self.inner.list_mcp_servers().await
    }

    pub async fn list_mcp_prompts(&self) -> Vec<McpPromptSummary> {
        self.inner.list_mcp_prompts().await
    }

    pub async fn list_mcp_resources(&self) -> Vec<McpResourceSummary> {
        self.inner.list_mcp_resources().await
    }

    pub async fn load_mcp_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
    ) -> Result<LoadedMcpPrompt> {
        self.inner.load_mcp_prompt(server_name, prompt_name).await
    }

    pub async fn load_mcp_resource(
        &self,
        server_name: &str,
        uri: &str,
    ) -> Result<LoadedMcpResource> {
        self.inner.load_mcp_resource(server_name, uri).await
    }
}
