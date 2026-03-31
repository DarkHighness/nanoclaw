use crate::backend::session_catalog;
use crate::backend::session_history::{
    self, LoadedAgentSession, LoadedSession, SessionExportArtifact, preview_id,
};
use crate::backend::session_resume;
use crate::backend::task_history::{self, LoadedTask, PersistedTaskSummary};
use crate::backend::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, LoadedMcpPrompt, LoadedMcpResource,
    McpPromptSummary, McpResourceSummary, McpServerSummary, PermissionRequestCoordinator,
    PermissionRequestPrompt, ResumeSupport, SessionEvent, SessionEventObserver, SessionEventStream,
    StartupDiagnosticsSnapshot, UserInputCoordinator, UserInputPrompt, list_mcp_prompts,
    list_mcp_resources, list_mcp_servers, load_mcp_prompt, load_mcp_resource,
};
use crate::provider::{MutableAgentBackend, ReasoningEffortUpdate};
use crate::statusline::StatusLineConfig;
use agent::mcp::ConnectedMcpServer;
use agent::runtime::{
    PermissionGrantSnapshot, PermissionGrantStore, Result as RuntimeResult,
    RollbackVisibleHistoryOutcome, RunTurnOutcome, RuntimeCommandId, RuntimeControlPlane,
};
use agent::tools::{
    GrantedPermissionResponse, RequestPermissionProfile, SandboxPolicy, SubagentExecutor,
    SubagentLaunchSpec, SubagentParentContext, UserInputResponse, describe_sandbox_policy,
    request_permission_profile_from_granted, sandbox_backend_status,
};
use agent::types::{
    AgentSessionId, AgentTaskSpec, AgentWaitMode, AgentWaitRequest, Message, SessionId,
    new_opaque_id,
};
use agent::{AgentRuntime, RuntimeCommand, Skill, ToolExecutionContext};
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use store::SessionStore;
use tokio::sync::Mutex as AsyncMutex;

/// This snapshot is the frontend-facing startup contract. It keeps stable host
/// facts separate from the mutable runtime handle so new frontends can render
/// the same session metadata without reconstructing boot logic locally.
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionStartupSnapshot {
    pub(crate) workspace_name: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) active_session_ref: String,
    pub(crate) root_agent_session_id: String,
    pub(crate) provider_label: String,
    pub(crate) model: String,
    pub(crate) model_reasoning_effort: Option<String>,
    pub(crate) supported_model_reasoning_efforts: Vec<String>,
    pub(crate) tool_names: Vec<String>,
    pub(crate) store_label: String,
    pub(crate) store_warning: Option<String>,
    pub(crate) stored_session_count: usize,
    pub(crate) default_sandbox_summary: String,
    pub(crate) sandbox_summary: String,
    pub(crate) permission_mode: SessionPermissionMode,
    pub(crate) host_process_surfaces_allowed: bool,
    pub(crate) startup_diagnostics: StartupDiagnosticsSnapshot,
    pub(crate) statusline: StatusLineConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SessionPermissionMode {
    #[default]
    Default,
    DangerFullAccess,
}

impl SessionPermissionMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionPermissionModeOutcome {
    pub(crate) previous: SessionPermissionMode,
    pub(crate) current: SessionPermissionMode,
    pub(crate) sandbox_summary: String,
    pub(crate) host_process_surfaces_allowed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionOperation {
    StartFresh,
    ResumeAgentSession { agent_session_ref: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionOperationAction {
    StartedFresh,
    AlreadyAttached,
    Reattached,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionOperationOutcome {
    pub(crate) action: SessionOperationAction,
    pub(crate) session_ref: String,
    pub(crate) active_agent_session_ref: String,
    pub(crate) requested_agent_session_ref: Option<String>,
    pub(crate) startup: SessionStartupSnapshot,
    pub(crate) transcript: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskSummary {
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) role: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) session_ref: String,
    pub(crate) agent_session_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskSpawnOutcome {
    pub(crate) task: LiveTaskSummary,
    pub(crate) prompt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveTaskControlAction {
    Cancelled,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskControlOutcome {
    pub(crate) requested_ref: String,
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) action: LiveTaskControlAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveTaskMessageAction {
    Sent,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskMessageOutcome {
    pub(crate) requested_ref: String,
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) action: LiveTaskMessageAction,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskWaitOutcome {
    pub(crate) requested_ref: String,
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) summary: String,
    pub(crate) claimed_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelReasoningEffortOutcome {
    pub(crate) previous: Option<String>,
    pub(crate) current: Option<String>,
    pub(crate) supported: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingControlKind {
    Prompt,
    Steer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingControlSummary {
    pub(crate) id: String,
    pub(crate) kind: PendingControlKind,
    pub(crate) preview: String,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryRollbackOutcome {
    pub(crate) transcript: Vec<Message>,
    pub(crate) removed_message_count: usize,
}

/// The backend session owns runtime state so frontends can speak to a stable
/// host contract instead of sharing `AgentRuntime` directly.
#[derive(Clone)]
pub(crate) struct CodeAgentSession {
    runtime: Arc<AsyncMutex<AgentRuntime>>,
    control_plane: RuntimeControlPlane,
    model_backend: Option<MutableAgentBackend>,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    mcp_servers: Arc<Vec<ConnectedMcpServer>>,
    approvals: ApprovalCoordinator,
    user_inputs: UserInputCoordinator,
    permission_requests: PermissionRequestCoordinator,
    events: SessionEventStream,
    workspace_root: PathBuf,
    startup: Arc<RwLock<SessionStartupSnapshot>>,
    skills: Arc<Vec<Skill>>,
    permission_grants: PermissionGrantStore,
    session_tool_context: Arc<RwLock<ToolExecutionContext>>,
    default_sandbox_policy: SandboxPolicy,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        model_backend: Option<MutableAgentBackend>,
        subagent_executor: Arc<dyn SubagentExecutor>,
        store: Arc<dyn SessionStore>,
        mcp_servers: Vec<ConnectedMcpServer>,
        approvals: ApprovalCoordinator,
        user_inputs: UserInputCoordinator,
        permission_requests: PermissionRequestCoordinator,
        events: SessionEventStream,
        permission_grants: PermissionGrantStore,
        session_tool_context: Arc<RwLock<ToolExecutionContext>>,
        default_sandbox_policy: SandboxPolicy,
        startup: SessionStartupSnapshot,
        skills: Vec<Skill>,
    ) -> Self {
        let workspace_root = startup.workspace_root.clone();
        let control_plane = runtime.control_plane();
        Self {
            runtime: Arc::new(AsyncMutex::new(runtime)),
            control_plane,
            model_backend,
            subagent_executor,
            store,
            mcp_servers: Arc::new(mcp_servers),
            approvals,
            user_inputs,
            permission_requests,
            events,
            workspace_root,
            startup: Arc::new(RwLock::new(startup)),
            skills: Arc::new(skills),
            permission_grants,
            session_tool_context,
            default_sandbox_policy,
        }
    }

    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub(crate) fn startup_snapshot(&self) -> SessionStartupSnapshot {
        self.startup.read().unwrap().clone()
    }

    pub(crate) fn host_process_surfaces_allowed(&self) -> bool {
        self.startup.read().unwrap().host_process_surfaces_allowed
    }

    pub(crate) fn permission_mode(&self) -> SessionPermissionMode {
        self.startup.read().unwrap().permission_mode
    }

    fn sandbox_policy_for_mode(&self, mode: SessionPermissionMode) -> SandboxPolicy {
        match mode {
            SessionPermissionMode::Default => self.default_sandbox_policy.clone(),
            SessionPermissionMode::DangerFullAccess => SandboxPolicy::permissive()
                .with_fail_if_unavailable(self.default_sandbox_policy.fail_if_unavailable),
        }
    }

    pub(crate) fn skills(&self) -> &[Skill] {
        self.skills.as_slice()
    }

    pub(crate) fn startup_diagnostics(&self) -> StartupDiagnosticsSnapshot {
        self.startup.read().unwrap().startup_diagnostics.clone()
    }

    pub(crate) fn cycle_model_reasoning_effort(&self) -> Result<ModelReasoningEffortOutcome> {
        let backend = self
            .model_backend
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("thinking effort controls are unavailable"))?;
        let update = backend.cycle_reasoning_effort()?;
        Ok(self.apply_model_reasoning_effort_update(update))
    }

    pub(crate) fn set_model_reasoning_effort(
        &self,
        effort: &str,
    ) -> Result<ModelReasoningEffortOutcome> {
        let backend = self
            .model_backend
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("thinking effort controls are unavailable"))?;
        let update = backend.set_reasoning_effort(effort)?;
        Ok(self.apply_model_reasoning_effort_update(update))
    }

    pub(crate) async fn end_session(&self, reason: Option<String>) -> RuntimeResult<()> {
        self.user_inputs.cancel("session ended");
        self.permission_requests.cancel("session ended");
        let mut runtime = self.runtime.lock().await;
        runtime.end_session(reason).await
    }

    pub(crate) async fn apply_control(&self, command: RuntimeCommand) -> Result<()> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        runtime
            .apply_control_with_observer(command, &mut observer)
            .await
            .map_err(anyhow::Error::from)?;
        self.sync_runtime_session_refs(&runtime);
        Ok(())
    }

    pub(crate) async fn queue_prompt_command(&self, prompt: impl Into<String>) -> Result<String> {
        let queued = self.control_plane.push_prompt(prompt);
        Ok(queued.id.to_string())
    }

    pub(crate) fn queued_command_count(&self) -> usize {
        self.control_plane.len()
    }

    pub(crate) fn pending_controls(&self) -> Vec<PendingControlSummary> {
        self.control_plane
            .snapshot()
            .into_iter()
            .map(|queued| match queued.command {
                RuntimeCommand::Prompt { prompt } => PendingControlSummary {
                    id: queued.id.to_string(),
                    kind: PendingControlKind::Prompt,
                    preview: prompt,
                    reason: None,
                },
                RuntimeCommand::Steer { message, reason } => PendingControlSummary {
                    id: queued.id.to_string(),
                    kind: PendingControlKind::Steer,
                    preview: message,
                    reason,
                },
            })
            .collect()
    }

    pub(crate) fn update_pending_control(
        &self,
        control_ref: &str,
        content: &str,
    ) -> Result<PendingControlSummary> {
        let controls = self.pending_controls();
        let current = resolve_pending_control_reference(&controls, control_ref)?;
        let updated = self
            .control_plane
            .update(
                &RuntimeCommandId::from(current.id.clone()),
                match current.kind {
                    PendingControlKind::Prompt => RuntimeCommand::Prompt {
                        prompt: content.to_string(),
                    },
                    PendingControlKind::Steer => RuntimeCommand::Steer {
                        message: content.to_string(),
                        reason: current.reason.clone(),
                    },
                },
            )
            .ok_or_else(|| anyhow::anyhow!("pending control update failed for {control_ref}"))?;
        Ok(match updated.command {
            RuntimeCommand::Prompt { prompt } => PendingControlSummary {
                id: updated.id.to_string(),
                kind: PendingControlKind::Prompt,
                preview: prompt,
                reason: None,
            },
            RuntimeCommand::Steer { message, reason } => PendingControlSummary {
                id: updated.id.to_string(),
                kind: PendingControlKind::Steer,
                preview: message,
                reason,
            },
        })
    }

    pub(crate) fn remove_pending_control(
        &self,
        control_ref: &str,
    ) -> Result<PendingControlSummary> {
        let controls = self.pending_controls();
        let current = resolve_pending_control_reference(&controls, control_ref)?;
        let removed = self
            .control_plane
            .remove(&RuntimeCommandId::from(current.id.clone()))
            .ok_or_else(|| anyhow::anyhow!("pending control removal failed for {control_ref}"))?;
        Ok(match removed.command {
            RuntimeCommand::Prompt { prompt } => PendingControlSummary {
                id: removed.id.to_string(),
                kind: PendingControlKind::Prompt,
                preview: prompt,
                reason: None,
            },
            RuntimeCommand::Steer { message, reason } => PendingControlSummary {
                id: removed.id.to_string(),
                kind: PendingControlKind::Steer,
                preview: message,
                reason,
            },
        })
    }

    pub(crate) async fn clear_queued_commands(&self) -> usize {
        let mut runtime = self.runtime.lock().await;
        let cleared = runtime.clear_pending_runtime_commands_for_host();
        self.sync_runtime_session_refs(&runtime);
        cleared
    }

    pub(crate) async fn drain_queued_controls(&self) -> Result<bool> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        // Frontends never pop queued prompts themselves. They only wake the
        // runtime at an idle edge so the runtime can drain its own queue and
        // emit one consistent event stream for every dequeued control.
        let drained = runtime
            .drain_queued_controls_with_observer(&mut observer)
            .await
            .map_err(anyhow::Error::from)?;
        self.sync_runtime_session_refs(&runtime);
        Ok(drained)
    }

    pub(crate) fn schedule_runtime_steer(
        &self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> Result<String> {
        // Active-turn steer must bypass the host prompt queue so the runtime can
        // merge it only at its own safe points between model/tool phases.
        let queued = self.control_plane.push_steer(message, reason);
        Ok(queued.id.to_string())
    }

    pub(crate) fn take_pending_steers(&self) -> Result<Vec<PendingControlSummary>> {
        let steers = self
            .pending_controls()
            .into_iter()
            .filter(|control| control.kind == PendingControlKind::Steer)
            .collect::<Vec<_>>();
        for steer in &steers {
            let _ = self.remove_pending_control(&steer.id)?;
        }
        Ok(steers)
    }

    pub(crate) async fn run_one_shot_prompt(&self, prompt: &str) -> Result<RunTurnOutcome> {
        let mut runtime = self.runtime.lock().await;
        let outcome = runtime
            .run_user_prompt(prompt)
            .await
            .map_err(anyhow::Error::from)?;
        self.sync_runtime_session_refs(&runtime);
        Ok(outcome)
    }

    pub(crate) async fn rollback_visible_history_to_message(
        &self,
        message_id: &str,
    ) -> Result<HistoryRollbackOutcome> {
        let mut runtime = self.runtime.lock().await;
        let RollbackVisibleHistoryOutcome {
            removed_message_ids,
        } = runtime
            .rollback_visible_history_to_message(message_id.into())
            .await
            .map_err(anyhow::Error::from)?;
        let transcript = runtime.visible_transcript_snapshot();
        self.sync_runtime_session_refs(&runtime);
        Ok(HistoryRollbackOutcome {
            transcript,
            removed_message_count: removed_message_ids.len(),
        })
    }

    pub(crate) async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        let compacted = runtime
            .compact_now_with_observer(notes, &mut observer)
            .await?;
        self.sync_runtime_session_refs(&runtime);
        Ok(compacted)
    }

    pub(crate) async fn apply_session_operation(
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
        let (session_ref, agent_session_ref) = {
            let mut runtime = self.runtime.lock().await;
            runtime
                .start_new_session()
                .await
                .map_err(anyhow::Error::from)?;
            (
                runtime.session_id().to_string(),
                runtime.agent_session_id().to_string(),
            )
        };
        self.set_runtime_session_refs(session_ref, agent_session_ref);
        self.refresh_stored_session_count().await?;
        Ok(self
            .build_session_operation_outcome(SessionOperationAction::StartedFresh, None)
            .await)
    }

    pub(crate) fn approval_prompt(&self) -> Option<ApprovalPrompt> {
        self.approvals.snapshot()
    }

    pub(crate) fn resolve_approval(&self, decision: ApprovalDecision) -> bool {
        self.approvals.resolve(decision)
    }

    pub(crate) fn user_input_prompt(&self) -> Option<UserInputPrompt> {
        self.user_inputs.snapshot()
    }

    pub(crate) fn resolve_user_input(&self, response: UserInputResponse) -> bool {
        self.user_inputs.resolve(response)
    }

    pub(crate) fn cancel_user_input(&self, reason: impl Into<String>) -> bool {
        self.user_inputs.cancel(reason)
    }

    pub(crate) fn permission_request_prompt(&self) -> Option<PermissionRequestPrompt> {
        self.permission_requests.snapshot()
    }

    pub(crate) fn resolve_permission_request(&self, response: GrantedPermissionResponse) -> bool {
        self.permission_requests.resolve(response)
    }

    pub(crate) async fn set_permission_mode(
        &self,
        mode: SessionPermissionMode,
    ) -> Result<SessionPermissionModeOutcome> {
        let previous = self.permission_mode();
        let policy = self.sandbox_policy_for_mode(mode);
        let backend_status = sandbox_backend_status(&policy);
        let sandbox_summary = describe_sandbox_policy(&policy, &backend_status);
        let host_process_surfaces_allowed =
            !policy.requires_enforcement() || backend_status.is_available();

        {
            let mut runtime = self.runtime.lock().await;
            // Sticky `request_permissions` grants stay in the runtime-owned
            // grant store. This setter only swaps the session's base sandbox
            // mode so later tool calls and newly spawned subagents inherit the
            // same host-selected baseline.
            runtime.set_base_sandbox_policy(policy.clone());
            self.sync_runtime_session_refs(&runtime);
        }
        {
            let mut tool_context = self.session_tool_context.write().unwrap();
            tool_context.effective_sandbox_policy = Some(policy);
        }
        {
            let mut startup = self.startup.write().unwrap();
            startup.permission_mode = mode;
            startup.sandbox_summary = sandbox_summary.clone();
            startup.host_process_surfaces_allowed = host_process_surfaces_allowed;
        }

        Ok(SessionPermissionModeOutcome {
            previous,
            current: mode,
            sandbox_summary,
            host_process_surfaces_allowed,
        })
    }

    pub(crate) fn permission_grant_snapshot(&self) -> PermissionGrantSnapshot {
        self.permission_grants.snapshot()
    }

    pub(crate) fn permission_grant_profiles(
        &self,
    ) -> (RequestPermissionProfile, RequestPermissionProfile) {
        let snapshot = self.permission_grant_snapshot();
        (
            request_permission_profile_from_granted(&snapshot.turn),
            request_permission_profile_from_granted(&snapshot.session),
        )
    }

    pub(crate) fn drain_events(&self) -> Vec<SessionEvent> {
        self.events.drain()
    }

    pub(crate) async fn list_sessions(
        &self,
    ) -> Result<Vec<crate::backend::PersistedSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        self.set_stored_session_count(sessions.len());
        let active_session_ref = self.startup_snapshot().active_session_ref;
        Ok(sessions
            .iter()
            .map(|summary| session_catalog::persisted_session_summary(summary, &active_session_ref))
            .collect())
    }

    pub(crate) async fn search_sessions(
        &self,
        query: &str,
    ) -> Result<Vec<crate::backend::PersistedSessionSearchMatch>> {
        let matches = session_history::search_sessions(&self.store, query).await?;
        let active_session_ref = self.startup_snapshot().active_session_ref;
        Ok(matches
            .iter()
            .map(|result| {
                session_catalog::persisted_session_search_match(result, &active_session_ref)
            })
            .collect())
    }

    pub(crate) async fn list_agent_sessions(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<crate::backend::PersistedAgentSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let filtered_session_id = session_ref
            .map(|session_ref| session_history::resolve_session_reference(&sessions, session_ref))
            .transpose()?;
        let active_agent_session_ref = self.startup_snapshot().root_agent_session_id;
        let mut agent_sessions = Vec::new();
        for summary in sessions.into_iter().filter(|summary| {
            filtered_session_id
                .as_ref()
                .is_none_or(|session_id| summary.session_id == *session_id)
        }) {
            let events = self.store.events(&summary.session_id).await?;
            agent_sessions.extend(session_catalog::persisted_agent_session_summaries(
                summary.session_id.as_str(),
                &events,
                &active_agent_session_ref,
            ));
        }
        agent_sessions.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.agent_session_ref.cmp(&right.agent_session_ref))
        });
        Ok(agent_sessions)
    }

    pub(crate) async fn list_tasks(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<PersistedTaskSummary>> {
        task_history::list_tasks(&self.store, session_ref).await
    }

    pub(crate) async fn list_live_tasks(&self) -> Result<Vec<LiveTaskSummary>> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(live_task_summaries(&handles))
    }

    pub(crate) async fn spawn_live_task(
        &self,
        role: &str,
        prompt: &str,
    ) -> Result<LiveTaskSpawnOutcome> {
        let role = role.trim();
        if role.is_empty() {
            return Err(anyhow::anyhow!("live task role cannot be empty"));
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(anyhow::anyhow!("live task prompt cannot be empty"));
        }

        let parent = self.live_task_parent_context();
        let task = AgentTaskSpec {
            task_id: new_live_task_id(),
            role: role.to_string(),
            prompt: prompt.to_string(),
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let mut handles = self
            .subagent_executor
            .spawn(parent, vec![SubagentLaunchSpec::from_task(task)])
            .await
            .map_err(anyhow::Error::from)?;
        let handle = handles
            .pop()
            .ok_or_else(|| anyhow::anyhow!("live task spawn returned no child handle"))?;
        Ok(LiveTaskSpawnOutcome {
            task: live_task_summary(&handle),
            prompt: prompt.to_string(),
        })
    }

    pub(crate) async fn send_live_task(
        &self,
        task_or_agent_ref: &str,
        message: &str,
    ) -> Result<LiveTaskMessageOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(anyhow::Error::from)?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let updated = self
            .subagent_executor
            .send(
                parent,
                handle.agent_id.clone(),
                "steer".to_string(),
                json!({ "text": message }),
            )
            .await
            .map_err(anyhow::Error::from)?;
        Ok(LiveTaskMessageOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: updated.agent_id.to_string(),
            task_id: updated.task_id,
            status: updated.status.clone(),
            action: if handle.status.is_terminal() {
                LiveTaskMessageAction::AlreadyTerminal
            } else {
                LiveTaskMessageAction::Sent
            },
            message: message.to_string(),
        })
    }

    pub(crate) async fn wait_live_task(
        &self,
        task_or_agent_ref: &str,
    ) -> Result<LiveTaskWaitOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(anyhow::Error::from)?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let response = self
            .subagent_executor
            .wait(
                parent,
                AgentWaitRequest {
                    agent_ids: vec![handle.agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .map_err(anyhow::Error::from)?;
        let completed = response
            .completed
            .into_iter()
            .find(|candidate| candidate.agent_id == handle.agent_id)
            .unwrap_or(handle);
        let result = response
            .results
            .into_iter()
            .find(|candidate| candidate.agent_id.as_str() == completed.agent_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing live task result for {}", completed.task_id))?;
        Ok(LiveTaskWaitOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: completed.agent_id.to_string(),
            task_id: completed.task_id,
            status: completed.status,
            summary: result.summary,
            claimed_files: result.claimed_files,
        })
    }

    pub(crate) async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        session_history::load_session(&self.store, session_ref).await
    }

    pub(crate) async fn load_agent_session(
        &self,
        agent_session_ref: &str,
    ) -> Result<LoadedAgentSession> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        let summary =
            session_catalog::resolve_agent_session_reference(&agent_sessions, agent_session_ref)?
                .clone();
        session_history::load_agent_session(&self.store, summary).await
    }

    pub(crate) async fn load_task(&self, task_ref: &str) -> Result<LoadedTask> {
        let tasks = self.list_tasks(None).await?;
        let summary = task_history::resolve_task_reference(&tasks, task_ref)?.clone();
        task_history::load_task(&self.store, summary).await
    }

    pub(crate) async fn cancel_live_task(
        &self,
        task_or_agent_ref: &str,
        reason: Option<String>,
    ) -> Result<LiveTaskControlOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(anyhow::Error::from)?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let updated = self
            .subagent_executor
            .cancel(parent, handle.agent_id.clone(), reason)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(LiveTaskControlOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: updated.agent_id.to_string(),
            task_id: updated.task_id,
            status: updated.status.clone(),
            action: if handle.status.is_terminal() {
                LiveTaskControlAction::AlreadyTerminal
            } else {
                LiveTaskControlAction::Cancelled
            },
        })
    }

    pub(crate) async fn export_session(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        session_history::export_session_events(
            &self.store,
            self.workspace_root(),
            session_ref,
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn export_session_transcript(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        session_history::export_session_transcript(
            &self.store,
            self.workspace_root(),
            session_ref,
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn refresh_stored_session_count(&self) -> Result<usize> {
        let count = session_history::list_sessions(&self.store).await?.len();
        self.set_stored_session_count(count);
        Ok(count)
    }

    fn apply_model_reasoning_effort_update(
        &self,
        update: ReasoningEffortUpdate,
    ) -> ModelReasoningEffortOutcome {
        self.startup.write().unwrap().model_reasoning_effort = update.current.clone();
        ModelReasoningEffortOutcome {
            previous: update.previous,
            current: update.current,
            supported: update.supported,
        }
    }

    async fn resume_existing_agent_session(
        &self,
        agent_session_ref: &str,
    ) -> Result<SessionOperationOutcome> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        let summary =
            session_catalog::resolve_agent_session_reference(&agent_sessions, agent_session_ref)?;
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
        let runtime_session =
            session_resume::reconstruct_runtime_session(&events, &target_agent_session_id)?;
        let (active_session_ref, active_agent_session_ref) = {
            let mut runtime = self.runtime.lock().await;
            runtime
                .resume_session(runtime_session)
                .await
                .map_err(anyhow::Error::from)?;
            (
                runtime.session_id().to_string(),
                runtime.agent_session_id().to_string(),
            )
        };
        self.set_runtime_session_refs(active_session_ref.clone(), active_agent_session_ref.clone());
        self.refresh_stored_session_count().await?;
        Ok(self
            .build_session_operation_outcome(
                SessionOperationAction::Reattached,
                Some(summary.agent_session_ref.clone()),
            )
            .await)
    }

    pub(crate) async fn active_visible_transcript(&self) -> Vec<Message> {
        self.runtime.lock().await.visible_transcript_snapshot()
    }

    pub(crate) async fn list_mcp_servers(&self) -> Vec<McpServerSummary> {
        list_mcp_servers(self.mcp_servers.as_slice())
    }

    pub(crate) async fn list_mcp_prompts(&self) -> Vec<McpPromptSummary> {
        list_mcp_prompts(self.mcp_servers.as_slice())
    }

    pub(crate) async fn list_mcp_resources(&self) -> Vec<McpResourceSummary> {
        list_mcp_resources(self.mcp_servers.as_slice())
    }

    pub(crate) async fn load_mcp_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
    ) -> Result<LoadedMcpPrompt> {
        load_mcp_prompt(self.mcp_servers.as_slice(), server_name, prompt_name).await
    }

    pub(crate) async fn load_mcp_resource(
        &self,
        server_name: &str,
        uri: &str,
    ) -> Result<LoadedMcpResource> {
        load_mcp_resource(self.mcp_servers.as_slice(), server_name, uri).await
    }

    fn set_stored_session_count(&self, count: usize) {
        self.startup.write().unwrap().stored_session_count = count;
    }

    fn sync_runtime_session_refs(&self, runtime: &AgentRuntime) {
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

    // Host-initiated live task operations should still append their lifecycle
    // into the active top-level session, otherwise operator-side spawn/send/
    // cancel actions disappear from durable task history.
    fn live_task_parent_context(&self) -> SubagentParentContext {
        let startup = self.startup_snapshot();
        SubagentParentContext {
            session_id: Some(SessionId::from(startup.active_session_ref)),
            agent_session_id: Some(AgentSessionId::from(startup.root_agent_session_id)),
            turn_id: None,
            parent_agent_id: None,
        }
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
}

fn new_live_task_id() -> String {
    format!("task_{}", new_opaque_id())
}

fn live_task_summary(handle: &agent::types::AgentHandle) -> LiveTaskSummary {
    LiveTaskSummary {
        agent_id: handle.agent_id.to_string(),
        task_id: handle.task_id.clone(),
        role: handle.role.clone(),
        status: handle.status.clone(),
        session_ref: handle.session_id.to_string(),
        agent_session_ref: handle.agent_session_id.to_string(),
    }
}

fn live_task_summaries(handles: &[agent::types::AgentHandle]) -> Vec<LiveTaskSummary> {
    let mut summaries = handles.iter().map(live_task_summary).collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.task_id
            .cmp(&right.task_id)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    summaries
}

fn resolve_pending_control_reference<'a>(
    controls: &'a [PendingControlSummary],
    control_ref: &str,
) -> Result<&'a PendingControlSummary> {
    if let Some(control) = controls.iter().find(|control| control.id == control_ref) {
        return Ok(control);
    }

    let matches = controls
        .iter()
        .filter(|control| control.id.starts_with(control_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!("unknown pending control: {control_ref}")),
        [control] => Ok(control),
        _ => Err(anyhow::anyhow!(
            "ambiguous pending control prefix {control_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|control| preview_id(&control.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn resolve_live_task_reference<'a>(
    handles: &'a [agent::types::AgentHandle],
    task_or_agent_ref: &str,
) -> Result<&'a agent::types::AgentHandle> {
    if let Some(handle) = handles
        .iter()
        .find(|handle| handle.task_id == task_or_agent_ref)
    {
        return Ok(handle);
    }
    if let Some(handle) = handles
        .iter()
        .find(|handle| handle.agent_id.as_str() == task_or_agent_ref)
    {
        return Ok(handle);
    }

    let task_matches = handles
        .iter()
        .filter(|handle| handle.task_id.starts_with(task_or_agent_ref))
        .collect::<Vec<_>>();
    match task_matches.as_slice() {
        [handle] => return Ok(handle),
        [] => {}
        _ => {
            return Err(anyhow::anyhow!(
                "ambiguous live task prefix {task_or_agent_ref}: {}",
                task_matches
                    .iter()
                    .take(6)
                    .map(|handle| handle.task_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let agent_matches = handles
        .iter()
        .filter(|handle| handle.agent_id.as_str().starts_with(task_or_agent_ref))
        .collect::<Vec<_>>();
    match agent_matches.as_slice() {
        [] => Err(anyhow::anyhow!(
            "unknown live task or agent id: {task_or_agent_ref}"
        )),
        [handle] => Ok(handle),
        _ => Err(anyhow::anyhow!(
            "ambiguous live agent prefix {task_or_agent_ref}: {}",
            agent_matches
                .iter()
                .take(6)
                .map(|handle| preview_id(handle.agent_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodeAgentSession, PendingControlKind, SessionOperation, SessionOperationAction,
        SessionPermissionMode, SessionStartupSnapshot,
    };
    use crate::backend::{
        ApprovalCoordinator, PermissionRequestCoordinator, SessionEventStream,
        StartupDiagnosticsSnapshot, UserInputCoordinator,
    };
    use crate::statusline::StatusLineConfig;
    use agent::runtime::{HookRunner, ModelBackend, PermissionGrantStore, Result as RuntimeResult};
    use agent::tools::{
        Result as ToolResult, SubagentExecutor, SubagentLaunchSpec, SubagentParentContext,
        ToolError, ToolExecutionContext,
    };
    use agent::types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentStatus, AgentTaskSpec, AgentWaitRequest,
        AgentWaitResponse, ModelEvent, ModelRequest, SessionEventKind, SessionId,
    };
    use agent::{AgentRuntimeBuilder, RuntimeCommand, Skill};
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use serde_json::Value;
    use std::sync::{Arc, Mutex, RwLock};
    use store::{InMemorySessionStore, SessionStore};

    struct NeverBackend;

    #[async_trait]
    impl ModelBackend for NeverBackend {
        async fn stream_turn(
            &self,
            _request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            unreachable!("session-start tests never execute model turns")
        }
    }

    struct StreamingTextBackend;

    #[async_trait]
    impl ModelBackend for StreamingTextBackend {
        async fn stream_turn(
            &self,
            _request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            Ok(stream::iter(vec![Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                usage: None,
                reasoning: Vec::new(),
            })])
            .boxed())
        }
    }

    #[derive(Clone, Default)]
    struct RecordingPromptBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl RecordingPromptBackend {
        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ModelBackend for RecordingPromptBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            self.requests.lock().unwrap().push(request);
            Ok(stream::iter(vec![Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                usage: None,
                reasoning: Vec::new(),
            })])
            .boxed())
        }
    }

    struct NoopSubagentExecutor;

    #[async_trait]
    impl SubagentExecutor for NoopSubagentExecutor {
        async fn spawn(
            &self,
            _parent: SubagentParentContext,
            _tasks: Vec<SubagentLaunchSpec>,
        ) -> ToolResult<Vec<AgentHandle>> {
            Err(ToolError::invalid_state(
                "test executor does not support spawn",
            ))
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _channel: String,
            _payload: Value,
        ) -> ToolResult<AgentHandle> {
            Err(ToolError::invalid_state(
                "test executor does not support send",
            ))
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            _request: AgentWaitRequest,
        ) -> ToolResult<AgentWaitResponse> {
            Ok(AgentWaitResponse {
                completed: Vec::new(),
                pending: Vec::new(),
                results: Vec::<AgentResultEnvelope>::new(),
            })
        }

        async fn resume(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
        ) -> ToolResult<AgentHandle> {
            Err(ToolError::invalid_state(
                "test executor does not support resume",
            ))
        }

        async fn list(&self, _parent: SubagentParentContext) -> ToolResult<Vec<AgentHandle>> {
            Ok(Vec::new())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _reason: Option<String>,
        ) -> ToolResult<AgentHandle> {
            Err(ToolError::invalid_state(
                "test executor does not support cancel",
            ))
        }
    }

    struct RecordingSubagentExecutor {
        handles: Mutex<Vec<AgentHandle>>,
        spawned_tasks: Mutex<Vec<AgentTaskSpec>>,
        spawn_parents: Mutex<Vec<SubagentParentContext>>,
        sent_messages: Mutex<Vec<(AgentId, String, Value)>>,
        wait_response: Mutex<Option<AgentWaitResponse>>,
    }

    impl RecordingSubagentExecutor {
        fn new(handles: Vec<AgentHandle>) -> Self {
            Self {
                handles: Mutex::new(handles),
                spawned_tasks: Mutex::new(Vec::new()),
                spawn_parents: Mutex::new(Vec::new()),
                sent_messages: Mutex::new(Vec::new()),
                wait_response: Mutex::new(None),
            }
        }

        fn with_wait_response(handles: Vec<AgentHandle>, wait_response: AgentWaitResponse) -> Self {
            Self {
                handles: Mutex::new(handles),
                spawned_tasks: Mutex::new(Vec::new()),
                spawn_parents: Mutex::new(Vec::new()),
                sent_messages: Mutex::new(Vec::new()),
                wait_response: Mutex::new(Some(wait_response)),
            }
        }
    }

    #[async_trait]
    impl SubagentExecutor for RecordingSubagentExecutor {
        async fn spawn(
            &self,
            parent: SubagentParentContext,
            tasks: Vec<SubagentLaunchSpec>,
        ) -> ToolResult<Vec<AgentHandle>> {
            self.spawn_parents.lock().unwrap().push(parent);
            self.spawned_tasks
                .lock()
                .unwrap()
                .extend(tasks.iter().map(|launch| launch.task.clone()));
            let mut handles = self.handles.lock().unwrap();
            let mut spawned = Vec::with_capacity(tasks.len());
            for launch in tasks {
                let task = launch.task;
                let handle = AgentHandle {
                    agent_id: AgentId::from(format!("agent-{}", task.task_id)),
                    parent_agent_id: None,
                    session_id: SessionId::from(format!("session-{}", task.task_id)),
                    agent_session_id: agent::types::AgentSessionId::from(format!(
                        "agent-session-{}",
                        task.task_id
                    )),
                    task_id: task.task_id.clone(),
                    role: task.role.clone(),
                    status: AgentStatus::Queued,
                };
                handles.push(handle.clone());
                spawned.push(handle);
            }
            Ok(spawned)
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
            channel: String,
            payload: Value,
        ) -> ToolResult<AgentHandle> {
            let handle = self
                .handles
                .lock()
                .unwrap()
                .iter()
                .find(|handle| handle.agent_id == agent_id)
                .cloned()
                .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
            self.sent_messages
                .lock()
                .unwrap()
                .push((agent_id, channel, payload));
            Ok(handle)
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            _request: AgentWaitRequest,
        ) -> ToolResult<AgentWaitResponse> {
            Ok(self
                .wait_response
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(AgentWaitResponse {
                    completed: Vec::new(),
                    pending: Vec::new(),
                    results: Vec::new(),
                }))
        }

        async fn resume(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
        ) -> ToolResult<AgentHandle> {
            let mut handles = self.handles.lock().unwrap();
            let handle = handles
                .iter_mut()
                .find(|handle| handle.agent_id == agent_id)
                .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
            handle.status = AgentStatus::Queued;
            Ok(handle.clone())
        }

        async fn list(&self, _parent: SubagentParentContext) -> ToolResult<Vec<AgentHandle>> {
            Ok(self.handles.lock().unwrap().clone())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
            _reason: Option<String>,
        ) -> ToolResult<AgentHandle> {
            let mut handles = self.handles.lock().unwrap();
            let handle = handles
                .iter_mut()
                .find(|handle| handle.agent_id == agent_id)
                .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
            if !handle.status.is_terminal() {
                handle.status = agent::types::AgentStatus::Cancelled;
            }
            Ok(handle.clone())
        }
    }

    fn startup_snapshot(workspace_root: &std::path::Path) -> SessionStartupSnapshot {
        SessionStartupSnapshot {
            workspace_name: "workspace".to_string(),
            workspace_root: workspace_root.to_path_buf(),
            active_session_ref: "session-active".to_string(),
            root_agent_session_id: "agent-session-active".to_string(),
            provider_label: "provider".to_string(),
            model: "model".to_string(),
            model_reasoning_effort: Some("medium".to_string()),
            supported_model_reasoning_efforts: vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
            ],
            tool_names: Vec::new(),
            store_label: "memory".to_string(),
            store_warning: None,
            stored_session_count: 0,
            default_sandbox_summary: "workspace-write".to_string(),
            sandbox_summary: "workspace-write".to_string(),
            permission_mode: SessionPermissionMode::Default,
            host_process_surfaces_allowed: true,
            startup_diagnostics: StartupDiagnosticsSnapshot::default(),
            statusline: StatusLineConfig::default(),
        }
    }

    fn build_session(
        runtime: agent::AgentRuntime,
        subagent_executor: Arc<dyn SubagentExecutor>,
        store: Arc<dyn SessionStore>,
        startup: SessionStartupSnapshot,
    ) -> CodeAgentSession {
        let default_sandbox_policy = runtime.base_sandbox_policy();
        let session_tool_context = Arc::new(RwLock::new(ToolExecutionContext {
            workspace_root: startup.workspace_root.clone(),
            worktree_root: Some(startup.workspace_root.clone()),
            effective_sandbox_policy: Some(default_sandbox_policy.clone()),
            workspace_only: true,
            ..Default::default()
        }));
        CodeAgentSession::new(
            runtime,
            None,
            subagent_executor,
            store,
            Vec::new(),
            ApprovalCoordinator::default(),
            UserInputCoordinator::default(),
            PermissionRequestCoordinator::default(),
            SessionEventStream::default(),
            PermissionGrantStore::default(),
            session_tool_context,
            default_sandbox_policy,
            startup,
            Vec::<Skill>::new(),
        )
    }

    fn sample_handle(task_id: &str, agent_id: &str, status: AgentStatus) -> AgentHandle {
        AgentHandle {
            agent_id: AgentId::from(agent_id),
            parent_agent_id: None,
            session_id: SessionId::from("session-1"),
            agent_session_id: agent::types::AgentSessionId::from(format!(
                "agent-session-{task_id}"
            )),
            task_id: task_id.to_string(),
            role: "worker".to_string(),
            status,
        }
    }

    #[tokio::test]
    async fn start_new_session_refreshes_backend_snapshot_refs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let initial_session_ref = runtime.session_id().to_string();
        let initial_agent_session_ref = runtime.agent_session_id().to_string();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = initial_session_ref.clone();
        startup.root_agent_session_id = initial_agent_session_ref.clone();
        let session = build_session(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store.clone(),
            startup,
        );

        let outcome = session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();

        assert_eq!(outcome.action, SessionOperationAction::StartedFresh);
        assert_ne!(outcome.startup.active_session_ref, initial_session_ref);
        assert_ne!(
            outcome.startup.root_agent_session_id,
            initial_agent_session_ref
        );
        assert_eq!(outcome.startup.stored_session_count, 1);
        assert!(outcome.transcript.is_empty());

        let new_events = store
            .events(&SessionId::from(outcome.startup.active_session_ref.clone()))
            .await
            .unwrap();
        assert!(new_events.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::SessionStart { reason }
                if reason.as_deref() == Some("operator_new_session")
        )));
    }

    #[tokio::test]
    async fn queued_prompts_are_drained_by_runtime_owned_queue() {
        let dir = tempfile::tempdir().unwrap();
        let backend = RecordingPromptBackend::default();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = runtime.session_id().to_string();
        startup.root_agent_session_id = runtime.agent_session_id().to_string();
        let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

        let queued_id = session.queue_prompt_command("second").await.unwrap();
        assert!(!queued_id.is_empty());
        assert_eq!(session.queued_command_count(), 1);

        session
            .apply_control(RuntimeCommand::Prompt {
                prompt: "first".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(session.queued_command_count(), 0);
        let requests = backend.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].messages.last().unwrap().text_content(), "first");
        assert_eq!(
            requests[1].messages.last().unwrap().text_content(),
            "second"
        );
    }

    #[tokio::test]
    async fn permission_mode_switch_updates_frontend_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let session = build_session(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup_snapshot(dir.path()),
        );

        let outcome = session
            .set_permission_mode(SessionPermissionMode::DangerFullAccess)
            .await
            .unwrap();
        let snapshot = session.startup_snapshot();

        assert_eq!(outcome.current, SessionPermissionMode::DangerFullAccess);
        assert_eq!(
            snapshot.permission_mode,
            SessionPermissionMode::DangerFullAccess
        );
        assert!(snapshot.host_process_surfaces_allowed);
        assert!(snapshot.sandbox_summary.contains("danger-full-access"));
    }

    #[tokio::test]
    async fn pending_controls_can_be_updated_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = runtime.session_id().to_string();
        startup.root_agent_session_id = runtime.agent_session_id().to_string();
        let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

        let prompt_id = session.queue_prompt_command("draft").await.unwrap();
        let steer_id = session
            .schedule_runtime_steer("focus on tests", Some("manual".to_string()))
            .unwrap();

        let updated_prompt = session
            .update_pending_control(&prompt_id, "edited draft")
            .unwrap();
        assert_eq!(updated_prompt.kind, PendingControlKind::Prompt);
        assert_eq!(updated_prompt.preview, "edited draft");

        let removed_steer = session.remove_pending_control(&steer_id).unwrap();
        assert_eq!(removed_steer.kind, PendingControlKind::Steer);
        assert_eq!(removed_steer.preview, "focus on tests");
        assert_eq!(session.pending_controls().len(), 1);
    }

    #[tokio::test]
    async fn take_pending_steers_drains_all_steers_in_fifo_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = runtime.session_id().to_string();
        startup.root_agent_session_id = runtime.agent_session_id().to_string();
        let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

        let prompt_id = session
            .queue_prompt_command("follow-up prompt")
            .await
            .unwrap();
        let steer_one = session
            .schedule_runtime_steer("first steer", Some("manual_command".to_string()))
            .unwrap();
        let steer_two = session
            .schedule_runtime_steer("latest steer", Some("inline_enter".to_string()))
            .unwrap();

        let promoted = session.take_pending_steers().unwrap();

        assert_eq!(promoted.len(), 2);
        assert_eq!(promoted[0].id, steer_one);
        assert_eq!(promoted[0].kind, PendingControlKind::Steer);
        assert_eq!(promoted[0].preview, "first steer");
        assert_eq!(promoted[0].reason.as_deref(), Some("manual_command"));
        assert_eq!(promoted[1].id, steer_two);
        assert_eq!(promoted[1].kind, PendingControlKind::Steer);
        assert_eq!(promoted[1].preview, "latest steer");
        assert_eq!(promoted[1].reason.as_deref(), Some("inline_enter"));

        let remaining = session.pending_controls();
        assert_eq!(remaining.len(), 1);
        assert!(remaining.iter().any(|control| control.id == prompt_id));
        assert!(!remaining.iter().any(|control| control.id == steer_one));
        assert!(!remaining.iter().any(|control| control.id == steer_two));
    }

    #[tokio::test]
    async fn resume_agent_session_reattaches_archived_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let original_session_ref = runtime.session_id().to_string();
        let original_agent_session_ref = runtime.agent_session_id().to_string();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = original_session_ref.clone();
        startup.root_agent_session_id = original_agent_session_ref.clone();
        let session = build_session(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store.clone(),
            startup,
        );

        session
            .apply_control(RuntimeCommand::Prompt {
                prompt: "resume me".to_string(),
            })
            .await
            .unwrap();
        session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();

        let outcome = session
            .apply_session_operation(SessionOperation::ResumeAgentSession {
                agent_session_ref: original_agent_session_ref.clone(),
            })
            .await
            .unwrap();

        assert_eq!(outcome.action, SessionOperationAction::Reattached);
        assert_eq!(
            outcome.requested_agent_session_ref.as_deref(),
            Some(original_agent_session_ref.as_str())
        );
        assert_eq!(outcome.session_ref, original_session_ref);
        assert_ne!(outcome.active_agent_session_ref, original_agent_session_ref);
        assert_eq!(outcome.startup.active_session_ref, outcome.session_ref);
        assert_eq!(
            outcome.startup.root_agent_session_id,
            outcome.active_agent_session_ref
        );
        assert_eq!(outcome.transcript.len(), 1);
        assert_eq!(outcome.transcript[0].text_content(), "resume me");
    }

    #[tokio::test]
    async fn live_task_listing_projects_sorted_child_handles() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let executor = Arc::new(RecordingSubagentExecutor::new(vec![
            AgentHandle {
                role: "reviewer".to_string(),
                ..sample_handle("task-b", "agent-b", AgentStatus::Running)
            },
            AgentHandle {
                role: "researcher".to_string(),
                ..sample_handle("task-a", "agent-a", AgentStatus::Queued)
            },
        ]));
        let session = build_session(runtime, executor, store, startup_snapshot(dir.path()));

        let live_tasks = session.list_live_tasks().await.unwrap();

        assert_eq!(live_tasks.len(), 2);
        assert_eq!(live_tasks[0].task_id, "task-a");
        assert_eq!(live_tasks[1].task_id, "task-b");
        assert_eq!(live_tasks[0].role, "researcher");
    }

    #[tokio::test]
    async fn spawn_live_task_returns_handle_and_tracks_active_parent_context() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let startup = startup_snapshot(dir.path());
        let active_session_ref = startup.active_session_ref.clone();
        let active_agent_session_ref = startup.root_agent_session_id.clone();
        let executor = Arc::new(RecordingSubagentExecutor::new(Vec::new()));
        let session = build_session(runtime, executor.clone(), store, startup);

        let outcome = session
            .spawn_live_task("reviewer", "inspect the failing tests")
            .await
            .unwrap();

        assert_eq!(outcome.task.role, "reviewer");
        assert_eq!(outcome.task.status, AgentStatus::Queued);
        assert_eq!(outcome.prompt, "inspect the failing tests");
        assert!(outcome.task.task_id.starts_with("task_"));
        let spawned_tasks = executor.spawned_tasks.lock().unwrap();
        assert_eq!(spawned_tasks.len(), 1);
        assert_eq!(spawned_tasks[0].role, "reviewer");
        assert_eq!(spawned_tasks[0].prompt, "inspect the failing tests");
        let spawn_parents = executor.spawn_parents.lock().unwrap();
        assert_eq!(spawn_parents.len(), 1);
        assert_eq!(
            spawn_parents[0]
                .session_id
                .as_ref()
                .map(|value| value.as_str()),
            Some(active_session_ref.as_str())
        );
        assert_eq!(
            spawn_parents[0]
                .agent_session_id
                .as_ref()
                .map(|value| value.as_str()),
            Some(active_agent_session_ref.as_str())
        );
        assert!(spawn_parents[0].parent_agent_id.is_none());
    }

    #[tokio::test]
    async fn cancel_live_task_updates_backend_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let session = build_session(
            runtime,
            Arc::new(RecordingSubagentExecutor::new(vec![AgentHandle {
                role: "editor".to_string(),
                ..sample_handle("task-cancel", "agent-cancel", AgentStatus::Running)
            }])),
            store,
            startup_snapshot(dir.path()),
        );

        let outcome = session
            .cancel_live_task("task-cancel", Some("operator_cancel".to_string()))
            .await
            .unwrap();

        assert_eq!(outcome.action, super::LiveTaskControlAction::Cancelled);
        assert_eq!(outcome.task_id, "task-cancel");
        assert_eq!(outcome.status, AgentStatus::Cancelled);
    }

    #[tokio::test]
    async fn send_live_task_routes_steer_message_to_child_agent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let executor = Arc::new(RecordingSubagentExecutor::new(vec![sample_handle(
            "task-send",
            "agent-send",
            AgentStatus::Running,
        )]));
        let session = build_session(
            runtime,
            executor.clone(),
            store,
            startup_snapshot(dir.path()),
        );

        let outcome = session
            .send_live_task("task-send", "focus on tests")
            .await
            .unwrap();

        assert_eq!(outcome.action, super::LiveTaskMessageAction::Sent);
        assert_eq!(outcome.task_id, "task-send");
        let sent_messages = executor.sent_messages.lock().unwrap();
        assert_eq!(sent_messages.len(), 1);
        assert_eq!(sent_messages[0].0, AgentId::from("agent-send"));
        assert_eq!(sent_messages[0].1, "steer");
        assert_eq!(sent_messages[0].2["text"], "focus on tests");
    }

    #[tokio::test]
    async fn wait_live_task_returns_terminal_result_summary() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let completed_handle = sample_handle("task-wait", "agent-wait", AgentStatus::Completed);
        let wait_response = AgentWaitResponse {
            completed: vec![completed_handle.clone()],
            pending: Vec::new(),
            results: vec![AgentResultEnvelope {
                agent_id: AgentId::from("agent-wait"),
                task_id: "task-wait".to_string(),
                status: AgentStatus::Completed,
                summary: "finished child task".to_string(),
                text: "done".to_string(),
                artifacts: Vec::new(),
                claimed_files: vec!["src/lib.rs".to_string()],
                structured_payload: None,
            }],
        };
        let session = build_session(
            runtime,
            Arc::new(RecordingSubagentExecutor::with_wait_response(
                vec![sample_handle(
                    "task-wait",
                    "agent-wait",
                    AgentStatus::Running,
                )],
                wait_response,
            )),
            store,
            startup_snapshot(dir.path()),
        );

        let outcome = session.wait_live_task("task-wait").await.unwrap();

        assert_eq!(outcome.task_id, "task-wait");
        assert_eq!(outcome.status, AgentStatus::Completed);
        assert_eq!(outcome.summary, "finished child task");
        assert_eq!(outcome.claimed_files, vec!["src/lib.rs".to_string()]);
    }
}
