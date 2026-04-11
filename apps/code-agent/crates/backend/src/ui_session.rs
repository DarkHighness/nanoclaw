use crate::backend::CodeAgentSession;
use crate::ui::{
    UIAsyncCommand, UIAsyncResult, UIAsyncValue, UICommand, UIQuery, UIQueryResult, UIQueryValue,
    UIResult, UIResultValue,
};
use anyhow::Result;

/// The TUI talks to the backend through a query/command protocol instead of a
/// wide list of ad hoc methods. That keeps the UI boundary explicit and lets
/// future frontends depend on one transport-shaped surface.
#[derive(Clone)]
pub struct CodeAgentUiSession {
    inner: CodeAgentSession,
}

impl From<CodeAgentSession> for CodeAgentUiSession {
    fn from(inner: CodeAgentSession) -> Self {
        Self { inner }
    }
}

impl CodeAgentUiSession {
    pub fn query<T: UIQueryValue>(&self, query: UIQuery) -> T {
        let result = match query {
            UIQuery::WorkspaceRoot => {
                UIQueryResult::PathBuf(self.inner.workspace_root().to_path_buf())
            }
            UIQuery::StartupSnapshot => {
                UIQueryResult::StartupSnapshot(self.inner.startup_snapshot())
            }
            UIQuery::HostProcessSurfacesAllowed => {
                UIQueryResult::Bool(self.inner.host_process_surfaces_allowed())
            }
            UIQuery::ApprovalPrompt => UIQueryResult::ApprovalPrompt(self.inner.approval_prompt()),
            UIQuery::PermissionRequestPrompt => {
                UIQueryResult::PermissionRequestPrompt(self.inner.permission_request_prompt())
            }
            UIQuery::UserInputPrompt => {
                UIQueryResult::UserInputPrompt(self.inner.user_input_prompt())
            }
            UIQuery::PendingControls => {
                UIQueryResult::PendingControls(self.inner.pending_controls())
            }
            UIQuery::QueuedCommandCount => UIQueryResult::Usize(self.inner.queued_command_count()),
            UIQuery::StartupDiagnostics => {
                UIQueryResult::StartupDiagnostics(self.inner.startup_diagnostics())
            }
            UIQuery::PermissionGrantProfiles => {
                UIQueryResult::PermissionGrantProfiles(self.inner.permission_grant_profiles())
            }
            UIQuery::Skills => UIQueryResult::Skills(self.inner.skill_summaries()),
        };
        T::from_query_result(result)
    }

    pub fn dispatch<T: UIResultValue>(&self, command: UICommand) -> Result<T> {
        let result = match command {
            UICommand::ResolveApproval(decision) => {
                UIResult::Bool(self.inner.resolve_approval(decision))
            }
            UICommand::ResolvePermissionRequest(decision) => {
                UIResult::Bool(self.inner.resolve_permission_request(decision))
            }
            UICommand::ResolveUserInput(submission) => {
                UIResult::Bool(self.inner.resolve_user_input(submission))
            }
            UICommand::CancelUserInput { reason } => {
                UIResult::Bool(self.inner.cancel_user_input(reason))
            }
            UICommand::RemovePendingControl { control_ref } => {
                UIResult::PendingControl(self.inner.remove_pending_control(&control_ref)?)
            }
            UICommand::UpdatePendingControl {
                control_ref,
                content,
            } => {
                UIResult::PendingControl(self.inner.update_pending_control(&control_ref, &content)?)
            }
            UICommand::ScheduleRuntimeSteer { message, reason } => {
                UIResult::String(self.inner.schedule_runtime_steer(message, reason)?)
            }
            UICommand::TakePendingSteers => {
                UIResult::PendingControls(self.inner.take_pending_steers()?)
            }
            UICommand::CycleModelReasoningEffort => {
                UIResult::ModelReasoningEffortOutcome(self.inner.cycle_model_reasoning_effort()?)
            }
            UICommand::SetModelReasoningEffort { effort } => UIResult::ModelReasoningEffortOutcome(
                self.inner.set_model_reasoning_effort(&effort)?,
            ),
            UICommand::DrainEvents => UIResult::UIEvents(self.inner.drain_events()),
            UICommand::ScheduleLiveTaskAttention {
                outcome,
                turn_running,
            } => UIResult::LiveTaskAttentionOutcome(
                self.inner
                    .schedule_live_task_attention(&outcome, turn_running)?,
            ),
        };
        Ok(T::from_ui_result(result))
    }

    pub async fn run<T: UIAsyncValue>(&self, command: UIAsyncCommand) -> Result<T> {
        let result = match command {
            UIAsyncCommand::EndSession { reason } => {
                self.inner.end_session(reason).await?;
                UIAsyncResult::Unit(())
            }
            UIAsyncCommand::ApplyControl { command } => {
                self.inner.apply_control(command).await?;
                UIAsyncResult::Unit(())
            }
            UIAsyncCommand::QueuePromptCommand {
                message,
                submitted_prompt,
            } => UIAsyncResult::String(
                self.inner
                    .queue_prompt_command(message, submitted_prompt)
                    .await?,
            ),
            UIAsyncCommand::ClearQueuedCommands => {
                UIAsyncResult::Usize(self.inner.clear_queued_commands().await)
            }
            UIAsyncCommand::DrainQueuedControls => {
                UIAsyncResult::Bool(self.inner.drain_queued_controls().await?)
            }
            UIAsyncCommand::RollbackVisibleHistoryToMessage { message_id } => {
                UIAsyncResult::HistoryRollbackOutcome(
                    self.inner
                        .rollback_visible_history_to_message(&message_id)
                        .await?,
                )
            }
            UIAsyncCommand::HistoryRollbackRounds => {
                UIAsyncResult::HistoryRollbackRounds(self.inner.history_rollback_rounds().await)
            }
            UIAsyncCommand::CompactNow { notes } => {
                UIAsyncResult::Bool(self.inner.compact_now(notes).await?)
            }
            UIAsyncCommand::ApplySessionOperation { operation } => {
                UIAsyncResult::SessionOperationOutcome(
                    self.inner.apply_session_operation(operation).await?,
                )
            }
            UIAsyncCommand::SetPermissionMode { mode } => {
                UIAsyncResult::SessionPermissionModeOutcome(
                    self.inner.set_permission_mode(mode).await?,
                )
            }
            UIAsyncCommand::RefreshStoredSessionCount => {
                UIAsyncResult::Usize(self.inner.refresh_stored_session_count().await?)
            }
            UIAsyncCommand::ListSessions => {
                UIAsyncResult::PersistedSessions(self.inner.list_sessions().await?)
            }
            UIAsyncCommand::SearchSessions { query } => {
                UIAsyncResult::PersistedSessionSearchMatches(
                    self.inner.search_sessions(&query).await?,
                )
            }
            UIAsyncCommand::ListAgentSessions { session_ref } => {
                UIAsyncResult::PersistedAgentSessions(
                    self.inner
                        .list_agent_sessions(session_ref.as_deref())
                        .await?,
                )
            }
            UIAsyncCommand::ListTasks { session_ref } => {
                UIAsyncResult::PersistedTasks(self.inner.list_tasks(session_ref.as_deref()).await?)
            }
            UIAsyncCommand::ListLiveTasks => {
                UIAsyncResult::LiveTasks(self.inner.list_live_tasks().await?)
            }
            UIAsyncCommand::SpawnLiveTask { role, prompt } => UIAsyncResult::LiveTaskSpawnOutcome(
                self.inner.spawn_live_task(&role, &prompt).await?,
            ),
            UIAsyncCommand::SendLiveTask {
                task_or_agent_ref,
                message,
            } => UIAsyncResult::LiveTaskMessageOutcome(
                self.inner
                    .send_live_task(&task_or_agent_ref, &message)
                    .await?,
            ),
            UIAsyncCommand::WaitLiveTask { task_or_agent_ref } => {
                UIAsyncResult::LiveTaskWaitOutcome(
                    self.inner.wait_live_task(&task_or_agent_ref).await?,
                )
            }
            UIAsyncCommand::CancelLiveTask {
                task_or_agent_ref,
                reason,
            } => UIAsyncResult::LiveTaskControlOutcome(
                self.inner
                    .cancel_live_task(&task_or_agent_ref, reason)
                    .await?,
            ),
            UIAsyncCommand::LoadSession { session_ref } => {
                UIAsyncResult::LoadedSession(self.inner.load_session(&session_ref).await?)
            }
            UIAsyncCommand::LoadAgentSession { agent_session_ref } => {
                UIAsyncResult::LoadedAgentSession(
                    self.inner.load_agent_session(&agent_session_ref).await?,
                )
            }
            UIAsyncCommand::LoadTask { task_ref } => {
                UIAsyncResult::LoadedTask(self.inner.load_task(&task_ref).await?)
            }
            UIAsyncCommand::ExportSession { session_ref, path } => {
                UIAsyncResult::SessionExportArtifact(
                    self.inner.export_session(&session_ref, &path).await?,
                )
            }
            UIAsyncCommand::ExportSessionTranscript { session_ref, path } => {
                UIAsyncResult::SessionExportArtifact(
                    self.inner
                        .export_session_transcript(&session_ref, &path)
                        .await?,
                )
            }
            UIAsyncCommand::AnswerSideQuestion { question } => UIAsyncResult::SideQuestionOutcome(
                self.inner.answer_side_question(&question).await?,
            ),
            UIAsyncCommand::ListMcpServers => {
                UIAsyncResult::McpServerSummaries(self.inner.list_mcp_servers().await)
            }
            UIAsyncCommand::ListMcpPrompts => {
                UIAsyncResult::McpPromptSummaries(self.inner.list_mcp_prompts().await)
            }
            UIAsyncCommand::ListMcpResources => {
                UIAsyncResult::McpResourceSummaries(self.inner.list_mcp_resources().await)
            }
            UIAsyncCommand::LoadMcpPrompt {
                server_name,
                prompt_name,
            } => UIAsyncResult::LoadedMcpPrompt(
                self.inner
                    .load_mcp_prompt(&server_name, &prompt_name)
                    .await?,
            ),
            UIAsyncCommand::LoadMcpResource { server_name, uri } => {
                UIAsyncResult::LoadedMcpResource(
                    self.inner.load_mcp_resource(&server_name, &uri).await?,
                )
            }
        };
        Ok(T::from_ui_async_result(result))
    }
}
