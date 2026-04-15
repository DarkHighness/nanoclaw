use crate::backend::boot_runtime::{
    COMMAND_HOOK_DISABLED_WARNING_PREFIX, MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX,
    SwitchableCodeIntelBackend, SwitchableCommandHookExecutor, SwitchableHostProcessExecutor,
};
use crate::backend::session_catalog;
use crate::backend::session_episodic_capture::{
    build_session_episodic_capture_prompt, parse_session_episodic_capture_entries,
};
use crate::backend::session_history::{self, preview_id};
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
use crate::backend::task_history::{self};
use crate::backend::{
    ApprovalCoordinator, PermissionRequestCoordinator, SessionEventObserver, SessionEventStream,
    UserInputCoordinator, connect_and_prepare_mcp_servers, list_mcp_prompts, list_mcp_resources,
    load_mcp_prompt, load_mcp_resource, resolve_mcp_tool_conflicts, summarize_mcp_servers,
};
use agent_env::EnvMap;
use nanoclaw_config::{PluginsConfig, ResolvedAgentProfile, ResolvedInternalProfile};
mod catalog;
mod controls;
mod dialogs;
mod history;
mod host_surfaces;
mod lifecycle;
mod live_tasks;
mod management;
mod memory;
mod monitors;
mod permissions;
mod review;
mod surface;

use crate::interaction::{ModelReasoningEffortOutcome, SkillSummary};
use crate::provider::{MutableAgentBackend, ReasoningEffortUpdate};
use crate::ui::{
    HistoryRollbackOutcome, HistoryRollbackRound, LiveMonitorControlAction,
    LiveMonitorControlOutcome, LiveMonitorSummary, LiveTaskAttentionAction,
    LiveTaskAttentionOutcome, LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction,
    LiveTaskMessageOutcome, LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome,
    LoadedAgentSession, LoadedMcpPrompt, LoadedMcpResource, LoadedSession, LoadedTask,
    ManagedMcpServerSummary, ManagedPluginSummary, ManagedSkillSummary, McpPromptSummary,
    McpResourceSummary, McpServerSummary, PersistedTaskSummary, ResumeSupport, SessionEvent,
    SessionExportArtifact, SessionOperation, SessionOperationAction, SessionOperationOutcome,
    SessionStartupSnapshot, SideQuestionOutcome, StartupDiagnosticsSnapshot,
};
use agent::mcp::{
    ConnectedMcpServer, McpConnectOptions, McpServerConfig, McpTransportConfig,
    catalog_resource_tools_as_registry_entries,
};
use agent::memory::{
    MemoryBackend, MemoryRecordMode, MemoryRecordRequest, MemoryScope, MemoryType,
};
use agent::runtime::{
    ModelBackend, PermissionGrantStore, Result as RuntimeResult, RollbackVisibleHistoryOutcome,
    RunTurnOutcome, RuntimeControlPlane, VisibleHistoryRollbackRound,
};
use agent::tools::{
    McpToolAdapter, MonitorManager, MonitorRuntimeContext, SandboxPolicy, SessionCompactionResult,
    SessionControlHandler, SessionReviewRequest, SessionReviewResult, SubagentExecutor,
    SubagentInputDelivery, SubagentLaunchSpec, SubagentParentContext,
};
use agent::types::{
    AgentSessionId, AgentTaskSpec, AgentWaitMode, AgentWaitRequest, HookHandler, HookRegistration,
    Message, MessageId, ModelEvent, ModelRequest, SessionId, ToolSpec, TurnId, new_opaque_id,
};
use agent::{AgentRuntime, RuntimeCommand, ToolExecutionContext};
use anyhow::Result;
use async_trait::async_trait;
use futures::{StreamExt, stream};
#[cfg(test)]
use memory::{CompactionWorkingSnapshot, SessionMemoryRefreshContext};
use memory::{SessionEpisodicCaptureState, SideQuestionContextSnapshot};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use store::{SessionStore, SessionSummary};
use tokio::fs;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{Duration, timeout};
use tracing::{info, warn};

// Keep the host-side session-note refresher aligned with Claude Code's
// default cadence so incremental updates happen often enough to preserve
// continuity without turning every small turn into note churn.
const SESSION_MEMORY_MIN_TOKENS_TO_INIT: usize = 10_000;
const SESSION_MEMORY_MIN_TOKENS_BETWEEN_UPDATES: usize = 5_000;
const SESSION_MEMORY_TOOL_CALLS_BETWEEN_UPDATES: usize = 3;
const SESSION_MEMORY_UPDATE_TIMEOUT_MS: u64 = 15_000;
const SESSION_NOTE_TITLE_LOAD_CONCURRENCY_LIMIT: usize = 8;
const WORKSPACE_MEMORY_RECALL_METADATA_KEY: &str = "workspace_memory_recall";
const STDIO_MCP_DISABLED_WARNING_PREFIX: &str =
    "sandbox backend unavailable; skipped stdio MCP servers to avoid host subprocess execution:";
const MCP_RESOURCE_TOOL_NAMES: [&str; 3] = [
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
];
const PERMISSION_MODE_SWITCH_BLOCKED_WHILE_TURN_RUNNING: &str =
    "cannot switch sandbox mode while a turn is running";
const MANAGED_SURFACE_REFRESH_BLOCKED_WHILE_TURN_RUNNING: &str =
    "cannot refresh managed surfaces while a turn is running";

#[derive(Clone)]
struct SessionPreambleConfig {
    skill_catalog: agent::SkillCatalog,
    plugin_instructions: Arc<RwLock<Vec<String>>>,
}

#[derive(Clone)]
pub(crate) struct ManagedSurfaceReloadConfig {
    pub(crate) env_map: EnvMap,
    pub(crate) primary_profile: ResolvedAgentProfile,
    pub(crate) memory_profile: ResolvedInternalProfile,
    pub(crate) skill_roots: Vec<PathBuf>,
    pub(crate) disabled_builtin_skills: Arc<RwLock<BTreeSet<String>>>,
    pub(crate) plugins: Arc<RwLock<PluginsConfig>>,
}

#[derive(Clone, Debug, Default)]
struct AppliedPluginSurfaceState {
    driver_tool_names: Vec<String>,
}

struct ActiveTurnGuard {
    active: Arc<AtomicBool>,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

/// The backend session owns runtime state so frontends can speak to a stable
/// host contract instead of sharing `AgentRuntime` directly.
#[derive(Clone)]
pub struct CodeAgentSession {
    runtime: Arc<AsyncMutex<AgentRuntime>>,
    control_plane: RuntimeControlPlane,
    checkpoint_manager: Arc<super::SessionCheckpointManager>,
    model_backend: Option<MutableAgentBackend>,
    subagent_executor: Arc<dyn SubagentExecutor>,
    monitor_manager: Arc<dyn MonitorManager>,
    worktree_manager: Arc<super::SessionWorktreeManager>,
    store: Arc<dyn SessionStore>,
    mcp_servers: Arc<RwLock<Vec<ConnectedMcpServer>>>,
    configured_mcp_servers: Arc<RwLock<Vec<McpServerConfig>>>,
    mcp_connection_details: Arc<RwLock<BTreeMap<String, String>>>,
    runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
    configured_runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
    mcp_process_executor: Arc<dyn agent::tools::ProcessExecutor>,
    host_process_executor: Arc<SwitchableHostProcessExecutor>,
    command_hook_executor: Arc<SwitchableCommandHookExecutor>,
    code_intel_backend: Arc<SwitchableCodeIntelBackend>,
    approvals: ApprovalCoordinator,
    user_inputs: UserInputCoordinator,
    permission_requests: PermissionRequestCoordinator,
    events: SessionEventStream,
    workspace_root: PathBuf,
    startup: Arc<RwLock<SessionStartupSnapshot>>,
    permission_grants: PermissionGrantStore,
    session_tool_context: Arc<RwLock<ToolExecutionContext>>,
    default_sandbox_policy: SandboxPolicy,
    preamble: SessionPreambleConfig,
    session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
    memory_backend: Arc<RwLock<Option<Arc<dyn MemoryBackend>>>>,
    session_memory_refresh: SharedSessionMemoryRefreshState,
    session_episodic_capture: Arc<Mutex<SessionEpisodicCaptureState>>,
    side_question_context: Arc<RwLock<Option<SideQuestionContextSnapshot>>>,
    runtime_turn_active: Arc<AtomicBool>,
    managed_surface_reload: ManagedSurfaceReloadConfig,
    applied_plugin_surfaces: Arc<RwLock<AppliedPluginSurfaceState>>,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        model_backend: Option<MutableAgentBackend>,
        session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
        subagent_executor: Arc<dyn SubagentExecutor>,
        monitor_manager: Arc<dyn MonitorManager>,
        worktree_manager: Arc<super::SessionWorktreeManager>,
        store: Arc<dyn SessionStore>,
        mcp_servers: Vec<ConnectedMcpServer>,
        configured_mcp_servers: Vec<McpServerConfig>,
        mcp_connection_details: BTreeMap<String, String>,
        runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
        configured_runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
        mcp_process_executor: Arc<dyn agent::tools::ProcessExecutor>,
        host_process_executor: Arc<SwitchableHostProcessExecutor>,
        command_hook_executor: Arc<SwitchableCommandHookExecutor>,
        code_intel_backend: Arc<SwitchableCodeIntelBackend>,
        approvals: ApprovalCoordinator,
        user_inputs: UserInputCoordinator,
        permission_requests: PermissionRequestCoordinator,
        events: SessionEventStream,
        permission_grants: PermissionGrantStore,
        session_tool_context: Arc<RwLock<ToolExecutionContext>>,
        default_sandbox_policy: SandboxPolicy,
        startup: SessionStartupSnapshot,
        skill_catalog: agent::SkillCatalog,
        plugin_instructions: Arc<RwLock<Vec<String>>>,
        memory_backend: Option<Arc<dyn MemoryBackend>>,
        session_memory_refresh: SharedSessionMemoryRefreshState,
        managed_surface_reload: ManagedSurfaceReloadConfig,
        driver_tool_names: Vec<String>,
    ) -> Self {
        let workspace_root = startup.workspace_root.clone();
        let checkpoint_manager = Arc::new(super::SessionCheckpointManager::new(store.clone()));
        session_tool_context.write().unwrap().checkpoint_handler = Some(checkpoint_manager.clone());
        // Session boot owns the runtime value here, so derive the initial
        // control-plane handles before wrapping it in an async mutex. Using
        // `blocking_lock()` during async startup can panic on current-thread
        // Tokio runtimes because that thread is already driving async tasks.
        let side_question_context = Some(Self::side_question_context_from_runtime(
            &runtime,
            None::<Message>,
        ));
        let control_plane = runtime.control_plane();
        let session_id = runtime.session_id();
        let runtime = Arc::new(AsyncMutex::new(runtime));
        session_memory_refresh.lock().unwrap().active_session_id = Some(session_id.clone());
        let initial_captured_message_id = side_question_context
            .as_ref()
            .and_then(|snapshot| snapshot.transcript.last())
            .map(|message| message.message_id.clone());
        let session_episodic_capture = Arc::new(Mutex::new(SessionEpisodicCaptureState {
            active_session_id: Some(session_id),
            last_captured_message_id: initial_captured_message_id,
            ..SessionEpisodicCaptureState::default()
        }));
        let session = Self {
            runtime,
            control_plane,
            checkpoint_manager,
            model_backend,
            subagent_executor,
            monitor_manager,
            worktree_manager,
            store,
            mcp_servers: Arc::new(RwLock::new(mcp_servers)),
            configured_mcp_servers: Arc::new(RwLock::new(configured_mcp_servers)),
            mcp_connection_details: Arc::new(RwLock::new(mcp_connection_details)),
            runtime_hooks,
            configured_runtime_hooks,
            mcp_process_executor,
            host_process_executor,
            command_hook_executor,
            code_intel_backend,
            approvals,
            user_inputs,
            permission_requests,
            events,
            workspace_root,
            startup: Arc::new(RwLock::new(startup)),
            permission_grants,
            session_tool_context,
            default_sandbox_policy,
            preamble: SessionPreambleConfig {
                skill_catalog,
                plugin_instructions,
            },
            session_memory_model_backend,
            memory_backend: Arc::new(RwLock::new(memory_backend)),
            session_memory_refresh,
            session_episodic_capture,
            side_question_context: Arc::new(RwLock::new(side_question_context)),
            runtime_turn_active: Arc::new(AtomicBool::new(false)),
            managed_surface_reload,
            applied_plugin_surfaces: Arc::new(RwLock::new(AppliedPluginSurfaceState {
                driver_tool_names,
            })),
        };
        session
            .session_tool_context
            .write()
            .unwrap()
            .session_control_handler = Some(Arc::new(session.clone()));
        session
    }
}

#[async_trait]
impl SessionControlHandler for CodeAgentSession {
    async fn compact_now(
        &self,
        _ctx: &ToolExecutionContext,
        notes: Option<String>,
    ) -> agent::tools::Result<SessionCompactionResult> {
        Ok(SessionCompactionResult {
            compacted: CodeAgentSession::compact_now(self, notes)
                .await
                .map_err(|error| agent::tools::ToolError::invalid_state(error.to_string()))?,
        })
    }

    async fn start_review(
        &self,
        ctx: &ToolExecutionContext,
        request: SessionReviewRequest,
    ) -> agent::tools::Result<SessionReviewResult> {
        self.session_review(ctx, request).await
    }
}

#[cfg(test)]
mod tests;
