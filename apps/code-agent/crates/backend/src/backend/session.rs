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
    UserInputCoordinator, build_system_preamble, list_mcp_prompts, list_mcp_resources,
    list_mcp_servers, load_mcp_prompt, load_mcp_resource,
};
mod catalog;
mod controls;
mod diagnostics;
mod dialogs;
mod history;
mod host_surfaces;
mod lifecycle;
mod live_tasks;
mod memory;
mod monitors;
mod permissions;
mod surface;

use crate::frontend_contract::skill_summary_from_skill;
use crate::interaction::{ModelReasoningEffortOutcome, SkillSummary};
use crate::provider::{MutableAgentBackend, ReasoningEffortUpdate};
use crate::ui::{
    HistoryRollbackOutcome, HistoryRollbackRound, LiveMonitorControlAction,
    LiveMonitorControlOutcome, LiveMonitorSummary, LiveTaskAttentionAction,
    LiveTaskAttentionOutcome, LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction,
    LiveTaskMessageOutcome, LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome,
    LoadedAgentSession, LoadedMcpPrompt, LoadedMcpResource, LoadedSession, LoadedTask,
    McpPromptSummary, McpResourceSummary, McpServerSummary, PersistedTaskSummary, ResumeSupport,
    SessionEvent, SessionExportArtifact, SessionOperation, SessionOperationAction,
    SessionOperationOutcome, SessionStartupSnapshot, SideQuestionOutcome,
    StartupDiagnosticsSnapshot,
};
use agent::mcp::{
    ConnectedMcpServer, McpConnectOptions, McpServerConfig, McpTransportConfig,
    catalog_resource_tools_as_registry_entries, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_options,
};
use agent::memory::{
    MemoryBackend, MemoryRecordMode, MemoryRecordRequest, MemoryScope, MemoryType,
};
use agent::runtime::{
    ModelBackend, PermissionGrantStore, Result as RuntimeResult, RollbackVisibleHistoryOutcome,
    RunTurnOutcome, RuntimeControlPlane, VisibleHistoryRollbackRound,
};
use agent::tools::{
    McpToolAdapter, MonitorManager, MonitorRuntimeContext, SandboxPolicy, SubagentExecutor,
    SubagentInputDelivery, SubagentLaunchSpec, SubagentParentContext,
};
use agent::types::{
    AgentSessionId, AgentTaskSpec, AgentWaitMode, AgentWaitRequest, HookHandler, HookRegistration,
    Message, MessageId, ModelEvent, ModelRequest, SessionId, ToolSpec, TurnId, new_opaque_id,
};
use agent::{AgentRuntime, RuntimeCommand, Skill, ToolExecutionContext};
use anyhow::Result;
use futures::{StreamExt, stream};
#[cfg(test)]
use memory::{CompactionWorkingSnapshot, SessionMemoryRefreshContext};
use memory::{SessionEpisodicCaptureState, SideQuestionContextSnapshot};
use nanoclaw_config::ResolvedAgentProfile;
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

#[derive(Clone)]
struct SessionPreambleConfig {
    profile: ResolvedAgentProfile,
    skill_catalog: agent::SkillCatalog,
    plugin_instructions: Vec<String>,
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
    model_backend: Option<MutableAgentBackend>,
    subagent_executor: Arc<dyn SubagentExecutor>,
    monitor_manager: Arc<dyn MonitorManager>,
    store: Arc<dyn SessionStore>,
    mcp_servers: Arc<RwLock<Vec<ConnectedMcpServer>>>,
    configured_mcp_servers: Arc<Vec<McpServerConfig>>,
    runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
    configured_runtime_hooks: Arc<Vec<HookRegistration>>,
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
    runtime_turn_active: Arc<AtomicBool>,
}

impl CodeAgentSession {
    pub fn new(
        runtime: AgentRuntime,
        model_backend: Option<MutableAgentBackend>,
        session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
        subagent_executor: Arc<dyn SubagentExecutor>,
        monitor_manager: Arc<dyn MonitorManager>,
        store: Arc<dyn SessionStore>,
        mcp_servers: Vec<ConnectedMcpServer>,
        configured_mcp_servers: Vec<McpServerConfig>,
        runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
        configured_runtime_hooks: Vec<HookRegistration>,
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
            monitor_manager,
            store,
            mcp_servers: Arc::new(RwLock::new(mcp_servers)),
            configured_mcp_servers: Arc::new(configured_mcp_servers),
            runtime_hooks,
            configured_runtime_hooks: Arc::new(configured_runtime_hooks),
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
            runtime_turn_active: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[cfg(test)]
mod tests;
