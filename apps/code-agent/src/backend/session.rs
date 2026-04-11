use crate::backend::session_catalog;
use crate::backend::session_episodic_capture::{
    build_session_episodic_capture_prompt, parse_session_episodic_capture_entries,
};
use crate::backend::session_history::{
    self, LoadedAgentSession, LoadedSession, SessionExportArtifact, preview_id,
};
use crate::backend::session_memory_compaction::{
    SESSION_MEMORY_STALE_THRESHOLD_MS, SharedSessionMemoryRefreshState,
    session_memory_note_absolute_path,
};
use crate::backend::session_memory_note::{
    build_session_memory_update_prompt, default_session_memory_note,
    parse_session_memory_note_snapshot, render_session_memory_note, session_memory_note_title,
    upsert_session_memory_note_frontmatter,
};
use crate::backend::session_resume;
use crate::backend::task_history::{self, LoadedTask, PersistedTaskSummary};
use crate::backend::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, LoadedMcpPrompt, LoadedMcpResource,
    McpPromptSummary, McpResourceSummary, McpServerSummary, PermissionRequestCoordinator,
    PermissionRequestPrompt, ResumeSupport, SessionEvent, SessionEventObserver, SessionEventStream,
    StartupDiagnosticsSnapshot, UserInputCoordinator, UserInputPrompt, build_system_preamble,
    list_mcp_prompts, list_mcp_resources, list_mcp_servers, load_mcp_prompt, load_mcp_resource,
};
use crate::provider::{MutableAgentBackend, ReasoningEffortUpdate};
use crate::statusline::StatusLineConfig;
use agent::mcp::ConnectedMcpServer;
use agent::memory::{
    MemoryBackend, MemoryRecordMode, MemoryRecordRequest, MemoryScope, MemoryType,
};
use agent::runtime::{
    ModelBackend, PermissionGrantSnapshot, PermissionGrantStore, Result as RuntimeResult,
    RollbackVisibleHistoryOutcome, RunTurnOutcome, RuntimeCommandId, RuntimeControlPlane,
    VisibleHistoryRollbackRound,
};
use agent::tools::{
    GrantedPermissionResponse, HOST_FEATURE_HOST_PROCESS_SURFACES, RequestPermissionProfile,
    SandboxPolicy, SubagentExecutor, SubagentInputDelivery, SubagentLaunchSpec,
    SubagentParentContext, UserInputResponse, describe_sandbox_policy,
    request_permission_profile_from_granted, sandbox_backend_status,
};
use agent::types::{
    AgentSessionId, AgentStatus, AgentTaskSpec, AgentWaitMode, AgentWaitRequest, Message,
    MessageId, ModelEvent, ModelRequest, SessionId, ToolSpec, TurnId, message_operator_text,
    new_opaque_id,
};
use agent::{AgentRuntime, RuntimeCommand, Skill, ToolExecutionContext};
use anyhow::Result;
use futures::{StreamExt, stream};
use nanoclaw_config::ResolvedAgentProfile;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use store::{SessionStore, SessionSummary};
use tokio::fs;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{Duration, timeout};
use tracing::warn;

// Keep the host-side session-note refresher aligned with Claude Code's
// default cadence so incremental updates happen often enough to preserve
// continuity without turning every small turn into note churn.
const SESSION_MEMORY_MIN_TOKENS_TO_INIT: usize = 10_000;
const SESSION_MEMORY_MIN_TOKENS_BETWEEN_UPDATES: usize = 5_000;
const SESSION_MEMORY_TOOL_CALLS_BETWEEN_UPDATES: usize = 3;
const SESSION_MEMORY_UPDATE_TIMEOUT_MS: u64 = 15_000;
const SESSION_NOTE_TITLE_LOAD_CONCURRENCY_LIMIT: usize = 8;
const WORKSPACE_MEMORY_RECALL_METADATA_KEY: &str = "workspace_memory_recall";

#[derive(Clone)]
struct SessionPreambleConfig {
    profile: ResolvedAgentProfile,
    skill_catalog: agent::SkillCatalog,
    plugin_instructions: Vec<String>,
}

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
    pub(crate) supports_image_input: bool,
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
    pub(crate) remaining_live_tasks: Vec<LiveTaskSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveTaskAttentionAction {
    QueuedPrompt,
    ScheduledSteer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskAttentionOutcome {
    pub(crate) action: LiveTaskAttentionAction,
    pub(crate) control_id: String,
    pub(crate) preview: String,
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryRollbackRound {
    pub(crate) rollback_message_id: MessageId,
    pub(crate) prompt_message: Message,
    pub(crate) round_messages: Vec<Message>,
    pub(crate) removed_turn_count: usize,
    pub(crate) removed_message_count: usize,
}

fn history_rollback_round_from_snapshot(
    snapshot: VisibleHistoryRollbackRound,
) -> Option<HistoryRollbackRound> {
    let prompt_message = snapshot
        .messages
        .iter()
        .find(|message| message.message_id == snapshot.prompt_message_id)
        .cloned()?;
    Some(HistoryRollbackRound {
        rollback_message_id: snapshot.rollback_message_id,
        prompt_message,
        round_messages: snapshot.messages,
        removed_turn_count: snapshot.removed_turn_count,
        removed_message_count: snapshot.removed_message_count,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactionWorkingSnapshot {
    session_id: SessionId,
    agent_session_id: AgentSessionId,
    summary: String,
    summary_message_id: MessageId,
}

#[derive(Clone, Debug)]
struct SessionMemoryRefreshContext {
    session_id: SessionId,
    agent_session_id: AgentSessionId,
    visible_transcript: Vec<Message>,
    context_tokens: usize,
    completed_turn_count: usize,
    tool_call_count: usize,
    compaction_summary_message_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
struct SessionMemoryRefreshJob {
    context: SessionMemoryRefreshContext,
    transcript_delta_text: String,
    epoch: u64,
}

#[derive(Clone, Debug, Default)]
struct SessionEpisodicCaptureState {
    active_session_id: Option<SessionId>,
    capture_in_flight: bool,
    capture_started_at: Option<Instant>,
    capture_epoch: u64,
    last_captured_message_id: Option<MessageId>,
}

#[derive(Clone, Debug)]
struct SessionEpisodicCaptureJob {
    context: SessionMemoryRefreshContext,
    transcript_delta_text: String,
    epoch: u64,
}

#[derive(Clone, Debug)]
struct SideQuestionContextSnapshot {
    session_id: SessionId,
    agent_session_id: AgentSessionId,
    instructions: Vec<String>,
    transcript: Vec<Message>,
    tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SideQuestionOutcome {
    pub(crate) question: String,
    pub(crate) response: String,
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
    preamble: SessionPreambleConfig,
    session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
    memory_backend: Option<Arc<dyn MemoryBackend>>,
    session_memory_refresh: SharedSessionMemoryRefreshState,
    session_episodic_capture: Arc<Mutex<SessionEpisodicCaptureState>>,
    side_question_context: Arc<RwLock<Option<SideQuestionContextSnapshot>>>,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        model_backend: Option<MutableAgentBackend>,
        session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
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
        profile: ResolvedAgentProfile,
        skill_catalog: agent::SkillCatalog,
        plugin_instructions: Vec<String>,
        skills: Vec<Skill>,
        memory_backend: Option<Arc<dyn MemoryBackend>>,
        session_memory_refresh: SharedSessionMemoryRefreshState,
    ) -> Self {
        let workspace_root = startup.workspace_root.clone();
        let side_question_context = Some(Self::side_question_context_from_runtime(
            &runtime,
            None::<Message>,
        ));
        let control_plane = runtime.control_plane();
        session_memory_refresh.lock().unwrap().active_session_id = Some(runtime.session_id());
        let initial_captured_message_id = side_question_context
            .as_ref()
            .and_then(|snapshot| snapshot.transcript.last())
            .map(|message| message.message_id.clone());
        let session_episodic_capture = Arc::new(Mutex::new(SessionEpisodicCaptureState {
            active_session_id: Some(runtime.session_id()),
            last_captured_message_id: initial_captured_message_id,
            ..SessionEpisodicCaptureState::default()
        }));
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
            preamble: SessionPreambleConfig {
                profile,
                skill_catalog,
                plugin_instructions,
            },
            session_memory_model_backend,
            memory_backend,
            session_memory_refresh,
            session_episodic_capture,
            side_question_context: Arc::new(RwLock::new(side_question_context)),
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
        let refresh_context = self.session_memory_refresh_context(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(refresh_context, snapshot)
            .await;
        Ok(())
    }

    pub(crate) async fn queue_prompt_command(
        &self,
        message: Message,
        submitted_prompt: Option<agent::types::SubmittedPromptSnapshot>,
    ) -> Result<String> {
        let queued = self
            .control_plane
            .push_prompt_with_snapshot(message, submitted_prompt);
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
                RuntimeCommand::Prompt { message, .. } => PendingControlSummary {
                    id: queued.id.to_string(),
                    kind: PendingControlKind::Prompt,
                    preview: message_operator_text(&message),
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
                        message: Message::user(content.to_string()),
                        submitted_prompt: Some(agent::types::SubmittedPromptSnapshot::from_text(
                            content.to_string(),
                        )),
                    },
                    PendingControlKind::Steer => RuntimeCommand::Steer {
                        message: content.to_string(),
                        reason: current.reason.clone(),
                    },
                },
            )
            .ok_or_else(|| anyhow::anyhow!("pending control update failed for {control_ref}"))?;
        Ok(match updated.command {
            RuntimeCommand::Prompt { message, .. } => PendingControlSummary {
                id: updated.id.to_string(),
                kind: PendingControlKind::Prompt,
                preview: message_operator_text(&message),
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
            RuntimeCommand::Prompt { message, .. } => PendingControlSummary {
                id: removed.id.to_string(),
                kind: PendingControlKind::Prompt,
                preview: message_operator_text(&message),
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
        let snapshot = self.latest_compaction_working_snapshot(&runtime, &observer);
        let refresh_context = self.session_memory_refresh_context(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(refresh_context, snapshot)
            .await;
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
        let refresh_context = self.session_memory_refresh_context(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(refresh_context, snapshot)
            .await;
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

    pub(crate) async fn history_rollback_rounds(&self) -> Vec<HistoryRollbackRound> {
        self.runtime
            .lock()
            .await
            .visible_history_rollback_rounds_snapshot()
            .into_iter()
            .filter_map(history_rollback_round_from_snapshot)
            .collect()
    }

    pub(crate) async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        let compacted = runtime
            .compact_now_with_observer(notes, &mut observer)
            .await?;
        let snapshot = self.latest_compaction_working_snapshot(&runtime, &observer);
        let refresh_context = self.session_memory_refresh_context(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(refresh_context, snapshot)
            .await;
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
        let instructions = self.rebuild_system_preamble();
        let (session_ref, agent_session_ref, side_question_context) = {
            let mut runtime = self.runtime.lock().await;
            runtime.replace_base_instructions(instructions);
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
        let (tool_names, side_question_context) = {
            let mut runtime = self.runtime.lock().await;
            let mut visibility = runtime.tool_visibility_context_snapshot();
            visibility.set_feature_enabled(
                HOST_FEATURE_HOST_PROCESS_SURFACES,
                host_process_surfaces_allowed,
            );

            // Sticky `request_permissions` grants stay in the runtime-owned
            // grant store. This setter only swaps the session's base sandbox
            // mode so later tool calls and newly spawned subagents inherit the
            // same host-selected baseline.
            runtime.replace_tool_visibility_context(visibility);
            runtime.set_base_sandbox_policy(policy.clone());
            self.sync_runtime_session_refs(&runtime);
            (
                runtime.tool_registry_names(),
                Self::side_question_context_from_runtime(&runtime, None),
            )
        };
        self.store_side_question_context(side_question_context);
        {
            let mut tool_context = self.session_tool_context.write().unwrap();
            tool_context.effective_sandbox_policy = Some(policy);
            tool_context.model_visibility.set_feature_enabled(
                HOST_FEATURE_HOST_PROCESS_SURFACES,
                host_process_surfaces_allowed,
            );
        }
        {
            let mut startup = self.startup.write().unwrap();
            startup.permission_mode = mode;
            startup.sandbox_summary = sandbox_summary.clone();
            startup.host_process_surfaces_allowed = host_process_surfaces_allowed;
            startup.tool_names = tool_names;
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

    fn latest_compaction_working_snapshot(
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

    fn side_question_context_from_runtime(
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

    fn store_side_question_context(&self, context: SideQuestionContextSnapshot) {
        *self.side_question_context.write().unwrap() = Some(context);
    }

    fn session_memory_refresh_context(
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

    async fn sync_session_memory_after_runtime_activity(
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

    fn mark_session_memory_refreshed(
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

    fn maybe_capture_session_episodic_memory(&self, context: SessionMemoryRefreshContext) {
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

    fn maybe_refresh_session_memory_note(
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

    async fn reset_session_memory_refresh_state(&self, context: &SideQuestionContextSnapshot) {
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

    async fn persist_compaction_working_snapshot(
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

    pub(crate) async fn list_sessions(
        &self,
    ) -> Result<Vec<crate::backend::PersistedSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        self.set_stored_session_count(sessions.len());
        let active_session_ref = self.startup_snapshot().active_session_ref;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        Ok(sessions
            .iter()
            .map(|summary| {
                session_catalog::persisted_session_summary(
                    summary,
                    &active_session_ref,
                    session_titles.get(&summary.session_id).cloned(),
                )
            })
            .collect())
    }

    pub(crate) async fn search_sessions(
        &self,
        query: &str,
    ) -> Result<Vec<crate::backend::PersistedSessionSearchMatch>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let matches = session_history::search_sessions(&self.store, query).await?;
        let sessions = session_history::list_sessions(&self.store).await?;
        let active_session_ref = self.startup_snapshot().active_session_ref;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let mut seen_session_refs = BTreeSet::new();
        let mut title_matches = Vec::new();
        let mut other_matches = Vec::new();

        for result in matches {
            let session_title = session_titles.get(&result.summary.session_id).cloned();
            let mut persisted = session_catalog::persisted_session_search_match(
                &result,
                &active_session_ref,
                session_title.clone(),
            );
            let matched_title =
                prepend_session_title_preview(&mut persisted, session_title.as_deref(), query);
            seen_session_refs.insert(persisted.summary.session_ref.clone());
            if matched_title {
                title_matches.push(persisted);
            } else {
                other_matches.push(persisted);
            }
        }

        let title_only_matches = sessions
            .iter()
            .filter_map(|summary| {
                let session_title = session_titles.get(&summary.session_id)?.clone();
                if !session_title_matches_query(Some(&session_title), query)
                    || seen_session_refs.contains(summary.session_id.as_str())
                {
                    return None;
                }
                Some(crate::backend::PersistedSessionSearchMatch {
                    summary: session_catalog::persisted_session_summary(
                        summary,
                        &active_session_ref,
                        Some(session_title.clone()),
                    ),
                    matched_event_count: 0,
                    preview_matches: vec![session_title_preview(&session_title)],
                })
            })
            .collect::<Vec<_>>();

        title_matches.extend(title_only_matches);
        title_matches.extend(other_matches);
        Ok(title_matches)
    }

    pub(crate) async fn list_agent_sessions(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<crate::backend::PersistedAgentSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let filtered_session_id = session_ref
            .map(|session_ref| {
                self.resolve_session_reference_from_catalog(&sessions, &session_titles, session_ref)
            })
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
                session_titles.get(&summary.session_id).map(String::as_str),
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

    // Session-note titles are host-owned derived memory, not store-owned
    // transcript metadata, so the catalog layer reads them here instead of
    // widening the session-store schema for one frontend-specific cue.
    async fn load_session_note_titles<I>(&self, session_ids: I) -> BTreeMap<SessionId, String>
    where
        I: IntoIterator<Item = SessionId>,
    {
        let workspace_root = self.workspace_root().to_path_buf();
        stream::iter(session_ids.into_iter().map(|session_id| {
            let workspace_root = workspace_root.clone();
            async move {
                let path = session_memory_note_absolute_path(&workspace_root, &session_id);
                let text = match fs::read_to_string(path).await {
                    Ok(text) => text,
                    Err(error) if error.kind() == ErrorKind::NotFound => return None,
                    Err(error) => {
                        warn!(
                            session_id = %session_id,
                            error = %error,
                            "failed to load session note title"
                        );
                        return None;
                    }
                };
                session_memory_note_title(&text).map(|title| (session_id, title))
            }
        }))
        .buffer_unordered(SESSION_NOTE_TITLE_LOAD_CONCURRENCY_LIMIT)
        .filter_map(async move |entry| entry)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
    }

    pub(crate) async fn list_tasks(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<PersistedTaskSummary>> {
        let resolved_session_ref = if let Some(session_ref) = session_ref {
            Some(
                self.resolve_session_reference_from_operator_input(session_ref)
                    .await?
                    .to_string(),
            )
        } else {
            None
        };
        task_history::list_tasks(&self.store, resolved_session_ref.as_deref()).await
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
                Message::user(message),
                SubagentInputDelivery::Queue,
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
                parent.clone(),
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
        let refreshed_handles = self
            .subagent_executor
            .list(parent)
            .await
            .map_err(anyhow::Error::from)?;
        let remaining_live_tasks = live_task_summaries(&refreshed_handles)
            .into_iter()
            .filter(|task| task.agent_id != completed.agent_id.as_str())
            .filter(|task| !task.status.is_terminal())
            .collect();
        Ok(LiveTaskWaitOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: completed.agent_id.to_string(),
            task_id: completed.task_id,
            status: completed.status,
            summary: result.summary,
            claimed_files: result.claimed_files,
            remaining_live_tasks,
        })
    }

    pub(crate) fn schedule_live_task_attention(
        &self,
        outcome: &LiveTaskWaitOutcome,
        turn_running: bool,
    ) -> Result<LiveTaskAttentionOutcome> {
        let preview = render_live_task_attention_message(outcome);
        if turn_running {
            let control_id = self.schedule_runtime_steer(
                preview.clone(),
                Some(format!("live_task_wait_complete:{}", outcome.task_id)),
            )?;
            return Ok(LiveTaskAttentionOutcome {
                action: LiveTaskAttentionAction::ScheduledSteer,
                control_id,
                preview,
            });
        }

        let queued = self
            .control_plane
            .push_prompt(Message::user(preview.clone()));
        Ok(LiveTaskAttentionOutcome {
            action: LiveTaskAttentionAction::QueuedPrompt,
            control_id: queued.id.to_string(),
            preview,
        })
    }

    pub(crate) async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        session_history::load_session(&self.store, session_id.as_str()).await
    }

    pub(crate) async fn load_agent_session(
        &self,
        agent_session_ref: &str,
    ) -> Result<LoadedAgentSession> {
        let summary = self
            .resolve_agent_session_reference_from_operator_input(agent_session_ref)
            .await?;
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
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        session_history::export_session_events(
            &self.store,
            self.workspace_root(),
            session_id.as_str(),
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn export_session_transcript(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        session_history::export_session_transcript(
            &self.store,
            self.workspace_root(),
            session_id.as_str(),
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
        let runtime_session =
            session_resume::reconstruct_runtime_session(&events, &target_agent_session_id)?;
        let instructions = self.rebuild_system_preamble();
        let (active_session_ref, active_agent_session_ref, side_question_context) = {
            let mut runtime = self.runtime.lock().await;
            runtime.replace_base_instructions(instructions);
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
        self.set_runtime_session_refs(active_session_ref.clone(), active_agent_session_ref.clone());
        self.refresh_stored_session_count().await?;
        Ok(self
            .build_session_operation_outcome(
                SessionOperationAction::Reattached,
                Some(summary.agent_session_ref.clone()),
            )
            .await)
    }

    async fn resolve_session_reference_from_operator_input(
        &self,
        session_ref: &str,
    ) -> Result<SessionId> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        self.resolve_session_reference_from_catalog(&sessions, &session_titles, session_ref)
    }

    async fn resolve_agent_session_reference_from_operator_input(
        &self,
        agent_session_ref: &str,
    ) -> Result<crate::backend::PersistedAgentSessionSummary> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        self.resolve_agent_session_reference_from_catalog(&agent_sessions, agent_session_ref)
    }

    fn resolve_session_reference_from_catalog(
        &self,
        sessions: &[SessionSummary],
        session_titles: &BTreeMap<SessionId, String>,
        session_ref: &str,
    ) -> Result<SessionId> {
        if let Some(session) = sessions
            .iter()
            .find(|summary| summary.session_id.as_str() == session_ref)
        {
            return Ok(session.session_id.clone());
        }

        let prefix_matches = sessions
            .iter()
            .filter(|summary| summary.session_id.as_str().starts_with(session_ref))
            .collect::<Vec<_>>();
        match prefix_matches.as_slice() {
            [session] => return Ok(session.session_id.clone()),
            [] => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "ambiguous session prefix {session_ref}: {}",
                    prefix_matches
                        .iter()
                        .take(6)
                        .map(|session| preview_id(session.session_id.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        let title_matches = sessions
            .iter()
            .filter_map(|summary| {
                let session_title = session_titles.get(&summary.session_id)?;
                session_title_matches_query(Some(session_title.as_str()), session_ref)
                    .then_some((summary, session_title.as_str()))
            })
            .collect::<Vec<_>>();
        match title_matches.as_slice() {
            [] => Err(anyhow::anyhow!(
                "unknown session id, prefix, or session title: {session_ref}"
            )),
            [(summary, _)] => Ok(summary.session_id.clone()),
            _ => Err(anyhow::anyhow!(
                "ambiguous session title {session_ref}: {}",
                title_matches
                    .iter()
                    .take(6)
                    .map(|(summary, title)| session_title_reference_preview(
                        summary.session_id.as_str(),
                        title
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    // Session-title fallback is intentionally host-owned. Claude-style session
    // selectors use human-readable memory cues, but the underlying store should
    // keep stable transcript ids as its only hard reference surface.
    fn resolve_agent_session_reference_from_catalog(
        &self,
        agent_sessions: &[crate::backend::PersistedAgentSessionSummary],
        agent_session_ref: &str,
    ) -> Result<crate::backend::PersistedAgentSessionSummary> {
        if let Some(summary) = agent_sessions
            .iter()
            .find(|summary| summary.agent_session_ref == agent_session_ref)
        {
            return Ok(summary.clone());
        }

        let prefix_matches = agent_sessions
            .iter()
            .filter(|summary| summary.agent_session_ref.starts_with(agent_session_ref))
            .collect::<Vec<_>>();
        match prefix_matches.as_slice() {
            [summary] => return Ok((*summary).clone()),
            [] => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "ambiguous agent session prefix {agent_session_ref}: {}",
                    prefix_matches
                        .iter()
                        .take(6)
                        .map(|summary| preview_id(summary.agent_session_ref.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        let mut session_matches = BTreeMap::new();
        for summary in agent_sessions.iter().filter(|summary| {
            session_title_matches_query(summary.session_title.as_deref(), agent_session_ref)
        }) {
            session_matches
                .entry(summary.session_ref.clone())
                .or_insert_with(Vec::new)
                .push(summary);
        }
        match session_matches.len() {
            0 => Err(anyhow::anyhow!(
                "unknown agent session id, prefix, or session title: {agent_session_ref}"
            )),
            1 => {
                let summaries = session_matches.into_values().next().unwrap();
                summaries
                    .iter()
                    .find(|summary| summary.label == "root")
                    .or_else(|| (summaries.len() == 1).then_some(&summaries[0]))
                    .map(|summary| (*summary).clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "ambiguous agent session title {agent_session_ref}: {}",
                            summaries
                                .iter()
                                .take(6)
                                .map(|summary| preview_id(summary.agent_session_ref.as_str()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
            }
            _ => Err(anyhow::anyhow!(
                "ambiguous session title {agent_session_ref}: {}",
                session_matches
                    .iter()
                    .take(6)
                    .map(|(session_ref, summaries)| {
                        let title = summaries
                            .first()
                            .and_then(|summary| summary.session_title.as_deref())
                            .unwrap_or("");
                        session_title_reference_preview(session_ref, title)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub(crate) async fn active_visible_transcript(&self) -> Vec<Message> {
        self.runtime.lock().await.visible_transcript_snapshot()
    }

    pub(crate) async fn answer_side_question(&self, question: &str) -> Result<SideQuestionOutcome> {
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

    fn rebuild_system_preamble(&self) -> Vec<String> {
        let tool_visibility = self
            .session_tool_context
            .read()
            .unwrap()
            .model_visibility
            .clone();
        build_system_preamble(
            self.workspace_root(),
            &self.preamble.profile,
            &self.preamble.skill_catalog,
            &self.preamble.plugin_instructions,
            &tool_visibility,
        )
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

fn render_live_task_attention_message(outcome: &LiveTaskWaitOutcome) -> String {
    let mut lines = vec![format!(
        "Background task {} finished with status {}.",
        outcome.task_id, outcome.status
    )];
    if !outcome.summary.trim().is_empty() {
        lines.push(format!("Task summary: {}", outcome.summary.trim()));
    }
    if !outcome.claimed_files.is_empty() {
        lines.push(format!(
            "Claimed files: {}.",
            outcome.claimed_files.join(", ")
        ));
    }
    if !outcome.remaining_live_tasks.is_empty() {
        lines.push(format!(
            "Still running background tasks: {}.",
            outcome
                .remaining_live_tasks
                .iter()
                .map(render_live_task_attention_task)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines.push(live_task_attention_instruction(outcome.status.clone()).to_string());
    lines.join("\n")
}

fn render_live_task_attention_task(task: &LiveTaskSummary) -> String {
    format!("{} ({}, {})", task.task_id, task.role, task.status)
}

fn live_task_attention_instruction(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Completed => {
            "Review the completed background task and integrate any useful findings."
        }
        AgentStatus::Failed => "Inspect the failed background task and decide whether to retry it.",
        AgentStatus::Cancelled => {
            "Inspect the cancelled background task and decide whether it should be restarted."
        }
        AgentStatus::Queued
        | AgentStatus::Running
        | AgentStatus::WaitingApproval
        | AgentStatus::WaitingMessage => {
            "Inspect the background task state before deciding on the next step."
        }
    }
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

fn session_title_matches_query(session_title: Option<&str>, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return false;
    }
    session_title
        .map(str::to_lowercase)
        .is_some_and(|title| title.contains(&query))
}

fn session_title_preview(session_title: &str) -> String {
    format!("session title: {}", session_title.trim())
}

fn session_title_reference_preview(session_ref: &str, session_title: &str) -> String {
    format!(
        "{} ({})",
        preview_id(session_ref),
        session_title.trim().chars().take(32).collect::<String>()
    )
}

fn prepend_session_title_preview(
    result: &mut crate::backend::PersistedSessionSearchMatch,
    session_title: Option<&str>,
    query: &str,
) -> bool {
    if !session_title_matches_query(session_title, query) {
        return false;
    }
    let Some(session_title) = session_title else {
        return false;
    };
    let preview = session_title_preview(session_title);
    if !result.preview_matches.iter().any(|entry| entry == &preview) {
        result.preview_matches.insert(0, preview);
        result.preview_matches.truncate(3);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        CodeAgentSession, CompactionWorkingSnapshot, LiveTaskAttentionAction, LiveTaskSummary,
        LiveTaskWaitOutcome, PendingControlKind, SessionMemoryRefreshContext, SessionOperation,
        SessionOperationAction, SessionPermissionMode, SessionStartupSnapshot,
        SideQuestionContextSnapshot,
    };
    use crate::backend::{
        ApprovalCoordinator, PermissionRequestCoordinator, SessionEventStream,
        StartupDiagnosticsSnapshot, UserInputCoordinator,
    };
    use crate::statusline::StatusLineConfig;
    use agent::memory::{MemoryBackend, MemoryCoreBackend};
    use agent::runtime::{
        CompactionConfig, CompactionRequest, CompactionResult, ConversationCompactor, HookRunner,
        ModelBackend, PermissionGrantStore, Result as RuntimeResult,
    };
    use agent::tools::{
        ExecCommandTool, Result as ToolResult, SubagentExecutor, SubagentInputDelivery,
        SubagentLaunchSpec, SubagentParentContext, ToolError, ToolExecutionContext, ToolRegistry,
        WriteStdinTool,
    };
    use agent::types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentSessionId, AgentStatus, AgentTaskSpec,
        AgentWaitRequest, AgentWaitResponse, Message, MessageId, ModelEvent, ModelRequest,
        SessionEventEnvelope, SessionEventKind, SessionId, SubmittedPromptSnapshot,
    };
    use agent::{AgentRuntimeBuilder, RuntimeCommand, Skill, SkillCatalog};
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use nanoclaw_config::CoreConfig;
    use std::sync::{Arc, Mutex, RwLock};
    use store::{EventSink, InMemorySessionStore, SessionStore};
    use tokio::sync::Semaphore;
    use tokio::time::{Duration, timeout};

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

    struct StaticCompactor;

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

    #[async_trait]
    impl ConversationCompactor for StaticCompactor {
        async fn compact(&self, request: CompactionRequest) -> RuntimeResult<CompactionResult> {
            Ok(CompactionResult {
                summary: format!("summary for {} messages", request.messages.len()),
            })
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

    #[derive(Clone)]
    struct ScriptedTextBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        responses: Arc<Mutex<Vec<String>>>,
    }

    impl ScriptedTextBackend {
        fn new(responses: Vec<String>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(responses)),
            }
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ModelBackend for ScriptedTextBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            self.requests.lock().unwrap().push(request);
            let response = self
                .responses
                .lock()
                .unwrap()
                .drain(..1)
                .next()
                .expect("scripted text backend response");
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta { delta: response }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[derive(Clone)]
    struct GatedTextBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        gate: Arc<Semaphore>,
        response: Arc<Mutex<Option<String>>>,
    }

    impl GatedTextBackend {
        fn new(response: &str) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                gate: Arc::new(Semaphore::new(0)),
                response: Arc::new(Mutex::new(Some(response.to_string()))),
            }
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn release(&self) {
            self.gate.add_permits(1);
        }
    }

    #[async_trait]
    impl ModelBackend for GatedTextBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            self.requests.lock().unwrap().push(request);
            let _permit = self.gate.acquire().await.unwrap();
            let response = self
                .response
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| "# Current State\n\nreleased".to_string());
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta { delta: response }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
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
            _message: agent::types::Message,
            _delivery: SubagentInputDelivery,
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
        sent_messages: Mutex<Vec<(AgentId, SubagentInputDelivery, agent::types::Message)>>,
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
            message: agent::types::Message,
            delivery: SubagentInputDelivery,
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
                .push((agent_id, delivery, message));
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
            supports_image_input: false,
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
        build_session_with_backends(runtime, subagent_executor, store, startup, None, None)
    }

    fn build_session_with_memory(
        runtime: agent::AgentRuntime,
        subagent_executor: Arc<dyn SubagentExecutor>,
        store: Arc<dyn SessionStore>,
        startup: SessionStartupSnapshot,
        memory_backend: Option<Arc<dyn MemoryBackend>>,
    ) -> CodeAgentSession {
        build_session_with_backends(
            runtime,
            subagent_executor,
            store,
            startup,
            memory_backend,
            None,
        )
    }

    fn build_session_with_backends(
        runtime: agent::AgentRuntime,
        subagent_executor: Arc<dyn SubagentExecutor>,
        store: Arc<dyn SessionStore>,
        startup: SessionStartupSnapshot,
        memory_backend: Option<Arc<dyn MemoryBackend>>,
        session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
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
            session_memory_model_backend,
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
            CoreConfig::default().resolve_primary_agent().unwrap(),
            SkillCatalog::default(),
            Vec::new(),
            Vec::<Skill>::new(),
            memory_backend,
            Arc::new(std::sync::Mutex::new(
                crate::backend::session_memory_compaction::SessionMemoryRefreshState::default(),
            )),
        )
    }

    fn write_session_note_title(
        workspace_root: &std::path::Path,
        session_id: &SessionId,
        title: &str,
    ) {
        let path =
            workspace_root.join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let note = super::render_session_memory_note(&format!(
            "# Session Title\n\n{title}\n\n# Current State\n\nContinue from the saved plan."
        ));
        std::fs::write(path, note).unwrap();
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
    async fn start_new_session_refreshes_workspace_memory_primer() {
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

        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("before refresh"),
                submitted_prompt: None,
            })
            .await
            .unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "# Rules\nrefresh on new session",
        )
        .unwrap();

        session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();
        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("after refresh"),
                submitted_prompt: None,
            })
            .await
            .unwrap();

        let requests = backend.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            !requests[0]
                .instructions
                .join("\n\n")
                .contains("# Workspace Memory Primer")
        );
        let refreshed = requests[1].instructions.join("\n\n");
        assert!(refreshed.contains("# Workspace Memory Primer"));
        assert!(refreshed.contains("refresh on new session"));
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

        let queued_id = session
            .queue_prompt_command(Message::user("second"), None)
            .await
            .unwrap();
        assert!(!queued_id.is_empty());
        assert_eq!(session.queued_command_count(), 1);

        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("first"),
                submitted_prompt: None,
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
        let mut registry = ToolRegistry::new();
        registry.register(ExecCommandTool::new());
        registry.register(WriteStdinTool::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_registry(registry)
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
        assert_eq!(
            snapshot.tool_names,
            vec!["exec_command".to_string(), "write_stdin".to_string()]
        );
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

        let prompt_id = session
            .queue_prompt_command(Message::user("draft"), None)
            .await
            .unwrap();
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
            .queue_prompt_command(Message::user("follow-up prompt"), None)
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
    async fn search_sessions_includes_title_only_session_note_matches() {
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
            store.clone(),
            startup_snapshot(dir.path()),
        );
        let archived_session_id = SessionId::from("session-archived");

        store
            .append(SessionEventEnvelope::new(
                archived_session_id.clone(),
                AgentSessionId::from("agent-archived"),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("status update"),
                },
            ))
            .await
            .unwrap();
        write_session_note_title(
            dir.path(),
            &archived_session_id,
            "Deploy rollback follow-up",
        );

        let matches = session.search_sessions("rollback").await.unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].summary.session_ref,
            archived_session_id.to_string()
        );
        assert_eq!(
            matches[0].summary.session_title.as_deref(),
            Some("Deploy rollback follow-up")
        );
        assert_eq!(matches[0].matched_event_count, 0);
        assert_eq!(
            matches[0].preview_matches,
            vec!["session title: Deploy rollback follow-up".to_string()]
        );
    }

    #[tokio::test]
    async fn load_session_resolves_unique_session_note_title_reference() {
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
            store.clone(),
            startup_snapshot(dir.path()),
        );
        let archived_session_id = SessionId::from("session-archived");

        store
            .append(SessionEventEnvelope::new(
                archived_session_id.clone(),
                AgentSessionId::from("agent-archived"),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("status update"),
                },
            ))
            .await
            .unwrap();
        write_session_note_title(
            dir.path(),
            &archived_session_id,
            "Deploy rollback follow-up",
        );

        let loaded = session.load_session("rollback").await.unwrap();

        assert_eq!(loaded.summary.session_id, archived_session_id);
        assert_eq!(loaded.events.len(), 1);
        assert!(matches!(
            &loaded.events[0].event,
            SessionEventKind::UserPromptSubmit { prompt } if prompt.text == "status update"
        ));
    }

    #[tokio::test]
    async fn load_session_rejects_ambiguous_session_note_title_reference() {
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
            store.clone(),
            startup_snapshot(dir.path()),
        );

        for (session_id, agent_session_id, prompt, title) in [
            (
                SessionId::from("session-archived-a"),
                AgentSessionId::from("agent-archived-a"),
                "status update",
                "Deploy rollback follow-up",
            ),
            (
                SessionId::from("session-archived-b"),
                AgentSessionId::from("agent-archived-b"),
                "rollback checklist",
                "Rollback verification",
            ),
        ] {
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id,
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: SubmittedPromptSnapshot::from_text(prompt),
                    },
                ))
                .await
                .unwrap();
            write_session_note_title(dir.path(), &session_id, title);
        }

        let error = session
            .load_session("rollback")
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("ambiguous session title rollback"));
        assert!(error.contains("session-"));
    }

    #[tokio::test]
    async fn list_agent_sessions_carries_parent_session_note_title() {
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
            store.clone(),
            startup_snapshot(dir.path()),
        );
        let archived_session_id = SessionId::from("session-archived");

        store
            .append_batch(vec![
                SessionEventEnvelope::new(
                    archived_session_id.clone(),
                    AgentSessionId::from("agent-root"),
                    None,
                    None,
                    SessionEventKind::SessionStart {
                        reason: Some("resume".to_string()),
                    },
                ),
                SessionEventEnvelope::new(
                    archived_session_id.clone(),
                    AgentSessionId::from("agent-root"),
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: SubmittedPromptSnapshot::from_text("inspect"),
                    },
                ),
            ])
            .await
            .unwrap();
        write_session_note_title(
            dir.path(),
            &archived_session_id,
            "Deploy rollback follow-up",
        );

        let agent_sessions = session.list_agent_sessions(None).await.unwrap();

        assert_eq!(agent_sessions.len(), 1);
        assert_eq!(
            agent_sessions[0].session_title.as_deref(),
            Some("Deploy rollback follow-up")
        );
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
                message: Message::user("resume me"),
                submitted_prompt: None,
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
    async fn resume_agent_session_resolves_session_note_title_to_root_agent() {
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
                message: Message::user("resume me"),
                submitted_prompt: None,
            })
            .await
            .unwrap();
        write_session_note_title(
            dir.path(),
            &SessionId::from(original_session_ref.clone()),
            "Deploy rollback follow-up",
        );
        session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();

        let outcome = session
            .apply_session_operation(SessionOperation::ResumeAgentSession {
                agent_session_ref: "rollback".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(outcome.action, SessionOperationAction::Reattached);
        assert_eq!(
            outcome.requested_agent_session_ref.as_deref(),
            Some(original_agent_session_ref.as_str())
        );
        assert_eq!(outcome.session_ref, original_session_ref);
        assert_eq!(outcome.transcript.len(), 1);
        assert_eq!(outcome.transcript[0].text_content(), "resume me");
    }

    #[tokio::test]
    async fn resume_agent_session_refreshes_workspace_memory_primer() {
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
        let original_session_ref = runtime.session_id().to_string();
        let original_agent_session_ref = runtime.agent_session_id().to_string();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = original_session_ref;
        startup.root_agent_session_id = original_agent_session_ref.clone();
        let session = build_session(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store.clone(),
            startup,
        );

        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("before resume"),
                submitted_prompt: None,
            })
            .await
            .unwrap();
        session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Rules\nrefresh on resume").unwrap();

        session
            .apply_session_operation(SessionOperation::ResumeAgentSession {
                agent_session_ref: original_agent_session_ref,
            })
            .await
            .unwrap();
        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("after resume"),
                submitted_prompt: None,
            })
            .await
            .unwrap();

        let requests = backend.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            !requests[0]
                .instructions
                .join("\n\n")
                .contains("# Workspace Memory Primer")
        );
        let refreshed = requests[1].instructions.join("\n\n");
        assert!(refreshed.contains("# Workspace Memory Primer"));
        assert!(refreshed.contains("refresh on resume"));
    }

    #[tokio::test]
    async fn manual_compaction_persists_working_memory_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .conversation_compactor(Arc::new(StaticCompactor))
            .compaction_config(CompactionConfig {
                enabled: true,
                context_window_tokens: 64,
                trigger_tokens: 1,
                preserve_recent_messages: 0,
            })
            .build();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = runtime.session_id().to_string();
        startup.root_agent_session_id = runtime.agent_session_id().to_string();
        let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let session = build_session_with_memory(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            Some(memory_backend),
        );

        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("first turn"),
                submitted_prompt: None,
            })
            .await
            .unwrap();
        session
            .apply_control(RuntimeCommand::Steer {
                message: "retain latest steer".to_string(),
                reason: Some("test".to_string()),
            })
            .await
            .unwrap();

        assert!(session.compact_now(None).await.unwrap());

        let working_path = dir.path().join(format!(
            ".nanoclaw/memory/working/sessions/{}.md",
            session.startup_snapshot().active_session_ref
        ));
        let snapshot = std::fs::read_to_string(working_path).unwrap();
        assert!(snapshot.contains("Session continuation snapshot"));
        assert!(snapshot.contains("# Session Title"));
        assert!(snapshot.contains("# Current State"));
        assert!(snapshot.contains("summary for 2 messages"));
        assert!(snapshot.contains("session_id:"));
        assert!(snapshot.contains("last_summarized_message_id:"));
    }

    #[tokio::test]
    async fn compaction_working_snapshot_replaces_previous_body() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .build();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = runtime.session_id().to_string();
        startup.root_agent_session_id = runtime.agent_session_id().to_string();
        let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let session = build_session_with_memory(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            Some(memory_backend),
        );

        session
            .persist_compaction_working_snapshot(Some(CompactionWorkingSnapshot {
                session_id: SessionId::from("session-1"),
                agent_session_id: AgentSessionId::from("agent-session-1"),
                summary: "first snapshot".to_string(),
                summary_message_id: MessageId::from("summary-first"),
            }))
            .await;
        session
            .persist_compaction_working_snapshot(Some(CompactionWorkingSnapshot {
                session_id: SessionId::from("session-1"),
                agent_session_id: AgentSessionId::from("agent-session-1"),
                summary: "second snapshot".to_string(),
                summary_message_id: MessageId::from("summary-second"),
            }))
            .await;

        let snapshot = std::fs::read_to_string(
            dir.path()
                .join(".nanoclaw/memory/working/sessions/session-1.md"),
        )
        .unwrap();
        assert!(snapshot.contains("# Session Title"));
        assert!(snapshot.contains("# Current State"));
        assert!(snapshot.contains("second snapshot"));
        assert!(snapshot.contains("last_summarized_message_id: summary-second"));
        assert!(!snapshot.contains("first snapshot"));
    }

    #[tokio::test]
    async fn forced_session_note_refresh_uses_summary_message_boundary() {
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
        let session_id = runtime.session_id();
        let agent_session_id = runtime.agent_session_id();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = session_id.to_string();
        startup.root_agent_session_id = agent_session_id.to_string();
        let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let note_backend = ScriptedTextBackend::new(vec![
            concat!(
                "# Current State\n",
                "Tracked tail update after compaction.\n\n",
                "# Worklog\n",
                "- Refreshed from transcript delta only."
            )
            .to_string(),
        ]);
        let session = build_session_with_backends(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            Some(memory_backend),
            Some(Arc::new(note_backend.clone())),
        );

        let summary_message = Message::system("summary before new work");
        let tail_message = Message::assistant("tail update after compaction");
        let context = SessionMemoryRefreshContext {
            session_id: session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            visible_transcript: vec![summary_message.clone(), tail_message.clone()],
            context_tokens: 0,
            completed_turn_count: 1,
            tool_call_count: 0,
            compaction_summary_message_id: Some(summary_message.message_id.clone()),
        };

        session.mark_session_memory_refreshed(&context, Some(summary_message.message_id.clone()));
        session.maybe_refresh_session_memory_note(context, true);
        timeout(Duration::from_secs(1), async {
            loop {
                if note_backend.requests().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let requests = note_backend.requests();
        assert_eq!(requests.len(), 1);
        let update_prompt = requests[0].messages[0].text_content();
        assert!(update_prompt.contains("tail update after compaction"));
        assert!(!update_prompt.contains("summary before new work"));

        timeout(Duration::from_secs(1), async {
            loop {
                let state = session.session_memory_refresh.lock().unwrap().clone();
                if !state.refresh_in_flight
                    && dir
                        .path()
                        .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
                        .exists()
                {
                    break;
                }
                drop(state);
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let note = std::fs::read_to_string(
            dir.path()
                .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md")),
        )
        .unwrap();
        assert!(note.contains("# Session Title"));
        assert!(note.contains("Tracked tail update after compaction."));
        assert!(!note.contains("summary before new work"));

        let state = session.session_memory_refresh.lock().unwrap().clone();
        assert!(state.initialized);
        assert_eq!(
            state.last_summarized_message_id,
            Some(tail_message.message_id.clone())
        );
    }

    #[tokio::test]
    async fn session_note_refresh_runs_in_background_without_blocking_caller() {
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
        let session_id = runtime.session_id();
        let agent_session_id = runtime.agent_session_id();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = session_id.to_string();
        startup.root_agent_session_id = agent_session_id.to_string();
        let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let note_backend =
            GatedTextBackend::new("# Current State\n\nAsync refresh completed successfully.");
        let session = build_session_with_backends(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            Some(memory_backend),
            Some(Arc::new(note_backend.clone())),
        );

        let context = SessionMemoryRefreshContext {
            session_id: session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            visible_transcript: vec![Message::assistant("fresh transcript delta")],
            context_tokens: 12_000,
            completed_turn_count: 1,
            tool_call_count: 0,
            compaction_summary_message_id: None,
        };

        session.maybe_refresh_session_memory_note(context, true);
        timeout(Duration::from_secs(1), async {
            loop {
                if note_backend.requests().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let state = session.session_memory_refresh.lock().unwrap().clone();
        assert!(state.refresh_in_flight);
        assert!(
            !dir.path()
                .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
                .exists()
        );

        note_backend.release();
        timeout(Duration::from_secs(1), async {
            loop {
                let state = session.session_memory_refresh.lock().unwrap().clone();
                if !state.refresh_in_flight {
                    break;
                }
                drop(state);
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let note = std::fs::read_to_string(
            dir.path()
                .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md")),
        )
        .unwrap();
        assert!(note.contains("Async refresh completed successfully."));
        let state = session.session_memory_refresh.lock().unwrap().clone();
        assert!(!state.refresh_in_flight);
        assert_eq!(state.active_session_id, Some(session_id));
    }

    #[tokio::test]
    async fn session_switch_invalidates_in_flight_refresh_state_updates() {
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
        let session_id = runtime.session_id();
        let agent_session_id = runtime.agent_session_id();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = session_id.to_string();
        startup.root_agent_session_id = agent_session_id.to_string();
        let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let note_backend = GatedTextBackend::new("# Current State\n\nOld session background note.");
        let session = build_session_with_backends(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            Some(memory_backend),
            Some(Arc::new(note_backend.clone())),
        );

        let old_context = SessionMemoryRefreshContext {
            session_id: session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            visible_transcript: vec![Message::assistant("old session delta")],
            context_tokens: 12_000,
            completed_turn_count: 1,
            tool_call_count: 0,
            compaction_summary_message_id: None,
        };
        session.maybe_refresh_session_memory_note(old_context, true);
        timeout(Duration::from_secs(1), async {
            loop {
                if note_backend.requests().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        session
            .reset_session_memory_refresh_state(&SideQuestionContextSnapshot {
                session_id: SessionId::from("session-new"),
                agent_session_id: AgentSessionId::from("agent-session-new"),
                instructions: Vec::new(),
                transcript: Vec::new(),
                tools: Vec::new(),
            })
            .await;

        note_backend.release();
        timeout(Duration::from_secs(1), async {
            loop {
                let state = session.session_memory_refresh.lock().unwrap().clone();
                if !state.refresh_in_flight
                    && dir
                        .path()
                        .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
                        .exists()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let state = session.session_memory_refresh.lock().unwrap().clone();
        assert_eq!(
            state.active_session_id,
            Some(SessionId::from("session-new"))
        );
        assert_eq!(state.last_summarized_message_id, None);
        assert!(!state.initialized);
        assert!(
            dir.path()
                .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
                .exists()
        );
    }

    #[tokio::test]
    async fn episodic_capture_appends_daily_log_entries_in_background() {
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
        let session_id = runtime.session_id();
        let agent_session_id = runtime.agent_session_id();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = session_id.to_string();
        startup.root_agent_session_id = agent_session_id.to_string();
        let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let capture_backend = ScriptedTextBackend::new(vec![
            "- User prefers canary deploys\n- Incident coordination moved to pager".to_string(),
        ]);
        let session = build_session_with_backends(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            Some(memory_backend),
            Some(Arc::new(capture_backend.clone())),
        );
        let context = SessionMemoryRefreshContext {
            session_id: session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            visible_transcript: vec![
                Message::user("remember that canary deploys are preferred"),
                Message::assistant("I'll keep that in mind and note the incident channel."),
            ],
            context_tokens: 1_000,
            completed_turn_count: 1,
            tool_call_count: 0,
            compaction_summary_message_id: None,
        };
        session.maybe_capture_session_episodic_memory(context);
        let logs_root = dir.path().join(".nanoclaw/memory/episodic/logs");
        timeout(Duration::from_secs(1), async {
            loop {
                let has_log_file = std::fs::read_dir(&logs_root)
                    .ok()
                    .into_iter()
                    .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .flat_map(|year_dir| {
                        std::fs::read_dir(year_dir)
                            .ok()
                            .into_iter()
                            .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
                            .map(|entry| entry.path())
                            .filter(|path| path.is_dir())
                            .collect::<Vec<_>>()
                    })
                    .any(|month_dir| {
                        std::fs::read_dir(month_dir)
                            .ok()
                            .into_iter()
                            .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
                            .any(|entry| entry.path().is_file())
                    });
                if capture_backend.requests().len() == 1 && has_log_file {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let requests = capture_backend.requests();
        assert_eq!(requests.len(), 1);
        let prompt = requests[0].messages[0].text_content();
        assert!(prompt.contains("append-only episodic daily log"));
        assert!(prompt.contains("canary deploys are preferred"));

        timeout(Duration::from_secs(1), async {
            loop {
                let state = session.session_episodic_capture.lock().unwrap().clone();
                if !state.capture_in_flight {
                    break;
                }
                drop(state);
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let year_dir = std::fs::read_dir(&logs_root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let month_dir = std::fs::read_dir(&year_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let log_path = std::fs::read_dir(&month_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let recorded = std::fs::read_to_string(log_path).unwrap();
        assert!(recorded.contains("scope: episodic"));
        assert!(recorded.contains("layer: daily-log"));
        assert!(recorded.contains("User prefers canary deploys"));
        assert!(recorded.contains("Incident coordination moved to pager"));
        let state = session.session_episodic_capture.lock().unwrap().clone();
        assert!(!state.capture_in_flight);
        assert_eq!(state.active_session_id, Some(session_id));
    }

    #[tokio::test]
    async fn reset_session_memory_refresh_state_rebases_episodic_capture_cursor() {
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
        let resumed_tail = Message::assistant("resume tail");

        session
            .reset_session_memory_refresh_state(&SideQuestionContextSnapshot {
                session_id: SessionId::from("session-new"),
                agent_session_id: AgentSessionId::from("agent-session-new"),
                instructions: Vec::new(),
                transcript: vec![resumed_tail.clone()],
                tools: Vec::new(),
            })
            .await;

        let state = session.session_episodic_capture.lock().unwrap().clone();
        assert_eq!(
            state.active_session_id,
            Some(SessionId::from("session-new"))
        );
        assert_eq!(
            state.last_captured_message_id,
            Some(resumed_tail.message_id.clone())
        );
        assert!(!state.capture_in_flight);
    }

    #[tokio::test]
    async fn answer_side_question_uses_snapshot_context_and_wrapper_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime_backend = RecordingPromptBackend::default();
        let runtime = AgentRuntimeBuilder::new(Arc::new(runtime_backend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .instructions(vec!["stable base instruction".to_string()])
            .build();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = runtime.session_id().to_string();
        startup.root_agent_session_id = runtime.agent_session_id().to_string();
        let side_backend = ScriptedTextBackend::new(vec!["Short answer.".to_string()]);
        let session = build_session_with_backends(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store,
            startup,
            None,
            Some(Arc::new(side_backend.clone())),
        );

        session
            .apply_control(RuntimeCommand::Prompt {
                message: Message::user("main thread question"),
                submitted_prompt: None,
            })
            .await
            .unwrap();

        let outcome = session
            .answer_side_question("  what changed?  ")
            .await
            .unwrap();

        assert_eq!(outcome.question, "what changed?");
        assert_eq!(outcome.response, "Short answer.");

        let requests = side_backend.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.instructions, vec!["stable base instruction"]);
        assert!(request.tools.is_empty());
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].text_content(), "main thread question");
        let side_prompt = request.messages[1].text_content();
        assert!(side_prompt.contains("This is a side question from the user"));
        assert!(side_prompt.contains("Do not call tools."));
        assert!(side_prompt.ends_with("what changed?"));
        assert_eq!(request.metadata["code_agent"]["purpose"], "side_question");
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
        assert_eq!(sent_messages[0].1, SubagentInputDelivery::Queue);
        assert_eq!(sent_messages[0].2.text_content(), "focus on tests");
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
                vec![
                    sample_handle("task-wait", "agent-wait", AgentStatus::Running),
                    sample_handle("task-followup", "agent-followup", AgentStatus::Running),
                    sample_handle("task-done", "agent-done", AgentStatus::Completed),
                ],
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
        assert_eq!(
            outcome.remaining_live_tasks,
            vec![LiveTaskSummary {
                agent_id: "agent-followup".to_string(),
                task_id: "task-followup".to_string(),
                role: "worker".to_string(),
                status: AgentStatus::Running,
                session_ref: "session-1".to_string(),
                agent_session_ref: "agent-session-task-followup".to_string(),
            }]
        );
    }

    #[test]
    fn schedule_live_task_attention_queues_prompt_when_idle() {
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
        let outcome = LiveTaskWaitOutcome {
            requested_ref: "task-wait".to_string(),
            agent_id: "agent-wait".to_string(),
            task_id: "task-wait".to_string(),
            status: AgentStatus::Completed,
            summary: "finished child task".to_string(),
            claimed_files: vec!["src/lib.rs".to_string()],
            remaining_live_tasks: vec![LiveTaskSummary {
                agent_id: "agent-followup".to_string(),
                task_id: "task-followup".to_string(),
                role: "reviewer".to_string(),
                status: AgentStatus::Running,
                session_ref: "session-1".to_string(),
                agent_session_ref: "agent-session-task-followup".to_string(),
            }],
        };

        let scheduled = session
            .schedule_live_task_attention(&outcome, false)
            .unwrap();

        assert_eq!(scheduled.action, LiveTaskAttentionAction::QueuedPrompt);
        assert!(!scheduled.control_id.is_empty());
        assert!(
            scheduled
                .preview
                .contains("Background task task-wait finished with status completed.")
        );
        assert!(
            scheduled
                .preview
                .contains("Task summary: finished child task")
        );
        assert!(scheduled.preview.contains("Claimed files: src/lib.rs."));
        assert!(
            scheduled
                .preview
                .contains("Still running background tasks: task-followup (reviewer, running).")
        );
        assert!(
            scheduled.preview.contains(
                "Review the completed background task and integrate any useful findings."
            )
        );

        let pending = session.pending_controls();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, PendingControlKind::Prompt);
        assert_eq!(pending[0].preview, scheduled.preview);
    }

    #[test]
    fn schedule_live_task_attention_schedules_steer_when_turn_running() {
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
        let outcome = LiveTaskWaitOutcome {
            requested_ref: "task-wait".to_string(),
            agent_id: "agent-wait".to_string(),
            task_id: "task-wait".to_string(),
            status: AgentStatus::Failed,
            summary: "child task failed".to_string(),
            claimed_files: Vec::new(),
            remaining_live_tasks: Vec::new(),
        };

        let scheduled = session
            .schedule_live_task_attention(&outcome, true)
            .unwrap();

        assert_eq!(scheduled.action, LiveTaskAttentionAction::ScheduledSteer);
        assert!(!scheduled.control_id.is_empty());
        assert!(
            scheduled
                .preview
                .contains("Background task task-wait finished with status failed.")
        );
        assert!(
            scheduled
                .preview
                .contains("Inspect the failed background task and decide whether to retry it.")
        );

        let pending = session.pending_controls();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, PendingControlKind::Steer);
        assert_eq!(
            pending[0].reason.as_deref(),
            Some("live_task_wait_complete:task-wait")
        );
        assert_eq!(pending[0].preview, scheduled.preview);
    }
}
