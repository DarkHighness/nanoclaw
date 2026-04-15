mod approval;
mod boot;
mod boot_inputs;
mod boot_mcp;
mod boot_preamble;
mod boot_runtime;
mod boot_sandbox;
#[cfg(feature = "browser-tools")]
mod browser_manager;
mod checkpoint_manager;
#[cfg(feature = "automation-tools")]
mod cron_manager;
mod events;
mod memory_recall;
mod monitor_manager;
mod permission_request;
mod session;
mod session_catalog;
mod session_episodic_capture;
mod session_history;
mod session_memory_compaction;
mod session_memory_note;
mod session_resume;
mod session_store_history;
mod store;
mod task_history;
mod task_manager;
mod tool_approval_policy;
mod user_input;
mod worktree_manager;

pub use crate::ui::{
    HistoryRollbackOutcome, HistoryRollbackRound, LiveMonitorControlAction,
    LiveMonitorControlOutcome, LiveMonitorSummary, LiveTaskAttentionAction,
    LiveTaskAttentionOutcome, LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction,
    LiveTaskMessageOutcome, LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome,
    LoadedAgentSession, LoadedMcpPrompt, LoadedMcpResource, LoadedSession, LoadedSubagentSession,
    LoadedTask, LoadedTaskMessage, McpPromptSummary, McpResourceSummary, McpServerSummary,
    PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
    PersistedTaskSummary, ResumeSupport, SessionEvent, SessionExportArtifact, SessionExportKind,
    SessionOperation, SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot,
    SessionToolCall, SideQuestionOutcome, StartupDiagnosticsSnapshot,
};
pub use approval::{
    ApprovalCoordinator, NonInteractiveToolApprovalHandler, SessionToolApprovalHandler,
};
#[allow(unused_imports)]
pub use boot::CodeAgentSubagentProfileResolver;
pub use boot::{
    BootProgressItem, BootProgressItemKind, BootProgressStage, BootProgressStatus,
    BootProgressUpdate, SessionApprovalMode, build_session, build_session_with_approval_mode,
    build_session_with_approval_mode_and_progress,
};
pub use boot_inputs::driver_host_output_lines;
pub use boot_inputs::{dedup_mcp_servers, merge_driver_host_inputs, resolve_mcp_servers};
pub use boot_mcp::{
    connect_and_prepare_mcp_servers, list_mcp_prompts, list_mcp_resources, list_mcp_servers,
    load_mcp_prompt, load_mcp_resource, mcp_connection_sandbox_policy, resolve_mcp_tool_conflicts,
    summarize_mcp_servers,
};
pub use boot_preamble::{build_plugin_activation_plan, build_system_preamble, resolve_skill_roots};
pub use boot_sandbox::{
    SandboxFallbackNotice, build_sandbox_fallback_notice, build_sandbox_policy, build_tool_context,
    inject_process_env, inspect_sandbox_preflight, log_sandbox_status, tool_context_for_profile,
};
#[cfg(feature = "browser-tools")]
pub(crate) use browser_manager::SessionBrowserManager;
pub(crate) use checkpoint_manager::SessionCheckpointManager;
#[cfg(feature = "automation-tools")]
pub(crate) use cron_manager::SessionCronManager;
pub use events::{SessionEventObserver, SessionEventPublisher, SessionEventStream};
pub(crate) use monitor_manager::SessionMonitorManager;
pub use permission_request::{
    NonInteractivePermissionRequestHandler, PermissionRequestCoordinator,
    SessionPermissionRequestHandler,
};
pub use session::CodeAgentSession;
pub use session_history::{message_to_text, preview_id};
pub use session_store_history::{
    SessionArchiveArtifact, SessionHistoryClient, SessionImportArtifact,
};
pub(crate) use task_manager::SessionTaskManager;
pub use tool_approval_policy::build_code_agent_tool_approval_policy;
pub use user_input::{
    NonInteractiveUserInputHandler, SessionUserInputHandler, UserInputCoordinator,
};
pub(crate) use worktree_manager::SessionWorktreeManager;
