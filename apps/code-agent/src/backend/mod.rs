mod approval;
mod boot;
mod boot_inputs;
mod boot_mcp;
mod boot_preamble;
mod boot_runtime;
mod boot_sandbox;
mod events;
mod permission_request;
mod session;
mod session_catalog;
mod session_history;
mod session_resume;
mod store;
mod task_history;
mod user_input;

pub(crate) use approval::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, NonInteractiveToolApprovalHandler,
    SessionToolApprovalHandler,
};
#[allow(unused_imports)]
pub(crate) use boot::CodeAgentSubagentProfileResolver;
pub(crate) use boot::{SessionApprovalMode, build_session, build_session_with_approval_mode};
#[cfg(test)]
pub(crate) use boot_inputs::driver_host_output_lines;
pub(crate) use boot_inputs::{dedup_mcp_servers, merge_driver_host_inputs, resolve_mcp_servers};
pub(crate) use boot_mcp::{
    LoadedMcpPrompt, LoadedMcpResource, McpPromptSummary, McpResourceSummary, McpServerSummary,
    StartupDiagnosticsSnapshot, list_mcp_prompts, list_mcp_resources, list_mcp_servers,
    load_mcp_prompt, load_mcp_resource,
};
pub(crate) use boot_preamble::{
    build_plugin_activation_plan, build_system_preamble, resolve_skill_roots,
};
pub(crate) use boot_sandbox::{
    SandboxFallbackNotice, build_sandbox_fallback_notice, build_sandbox_policy, build_tool_context,
    inject_process_env, inspect_sandbox_preflight, log_sandbox_status, tool_context_for_profile,
};
pub(crate) use events::{SessionEvent, SessionEventObserver, SessionEventStream, SessionToolCall};
pub(crate) use permission_request::{
    NonInteractivePermissionRequestHandler, PermissionRequestCoordinator, PermissionRequestPrompt,
    SessionPermissionRequestHandler,
};
pub(crate) use session::{
    ArtifactProposalExecutionOutcome, BenchmarkExecutionOutcome, CodeAgentSession,
    ImproveExecutionOutcome, LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction,
    LiveTaskMessageOutcome, LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome,
    ModelReasoningEffortOutcome, PendingControlKind, PendingControlSummary, SessionOperation,
    SessionOperationAction, SessionOperationOutcome, SessionPermissionMode, SessionStartupSnapshot,
};
pub(crate) use session_catalog::{
    PersistedAgentSessionSummary, PersistedArtifactSummary, PersistedExperimentSummary,
    PersistedSessionSearchMatch, PersistedSessionSummary, ResumeSupport,
};
pub(crate) use session_history::{
    LoadedAgentSession, LoadedArtifact, LoadedExperiment, LoadedSession, LoadedSubagentSession,
    SessionExportArtifact, SessionExportKind, message_to_text, preview_id,
};
pub(crate) use task_history::{LoadedTask, LoadedTaskMessage, PersistedTaskSummary};
pub(crate) use user_input::{
    NonInteractiveUserInputHandler, SessionUserInputHandler, UserInputCoordinator, UserInputPrompt,
};
