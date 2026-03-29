mod approval;
mod boot;
mod boot_inputs;
mod boot_mcp;
mod boot_preamble;
mod boot_runtime;
mod boot_sandbox;
mod events;
mod run_history;
mod session;
mod session_catalog;
mod store;

pub(crate) use approval::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, SessionToolApprovalHandler,
};
pub(crate) use boot::CodeAgentSubagentProfileResolver;
pub(crate) use boot::build_session;
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
    build_sandbox_policy, build_tool_context, inject_process_env, log_sandbox_status,
    tool_context_for_profile,
};
pub(crate) use events::{SessionEvent, SessionEventObserver, SessionEventStream};
pub(crate) use run_history::{
    LoadedRun, RunExportArtifact, RunExportKind, message_to_text, preview_id,
};
pub(crate) use session::{CodeAgentSession, SessionStartupSnapshot};
pub(crate) use session_catalog::{
    PersistedSessionSearchMatch, PersistedSessionSummary, SessionResumeStatus, SessionResumeSupport,
};
