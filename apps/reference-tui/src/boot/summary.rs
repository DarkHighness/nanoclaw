use super::StoreHandle;
use crate::{TuiStartupSummary, config::AgentCoreConfig};
use agent::mcp::ConnectedMcpServer;
use agent::plugins::{PluginActivationPlan, PluginDiagnosticLevel};
use tools::{SandboxBackendStatus, SandboxPolicy, describe_sandbox_policy};
use types::{RunId, ToolOrigin, ToolSpec};

pub(super) fn build_startup_summary(
    run_id: &RunId,
    workspace_root: &std::path::Path,
    provider_summary: &str,
    store_handle: &StoreHandle,
    stored_run_count: usize,
    tool_specs: &[ToolSpec],
    skill_names: &[String],
    mcp_servers: &[ConnectedMcpServer],
    config: &AgentCoreConfig,
    plugin_plan: &PluginActivationPlan,
    driver_warnings: &[String],
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
) -> TuiStartupSummary {
    let local_tools = tool_specs
        .iter()
        .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
        .count();
    let mcp_tools = tool_specs.len().saturating_sub(local_tools);
    let mut sidebar = vec![
        format!("run: {}", preview_id(run_id.as_str())),
        format!("workspace: {}", workspace_root.display()),
        format!("provider: {provider_summary}"),
        format!("store: {}", store_handle.label),
        format!("stored runs: {stored_run_count}"),
        format!(
            "tools: {} total ({local_tools} local, {mcp_tools} mcp)",
            tool_specs.len()
        ),
        format!("skills: {}", skill_names.len()),
        format!(
            "plugins: {} enabled / {} total",
            plugin_plan
                .plugin_states
                .iter()
                .filter(|state| state.enabled)
                .count(),
            plugin_plan.plugin_states.len()
        ),
        format!("mcp servers: {}", mcp_servers.len()),
        format!("command prefix: {}", config.tui.command_prefix),
        format!(
            "sandbox: {}",
            describe_sandbox_policy(sandbox_policy, sandbox_status)
        ),
        format!(
            "compaction: {}",
            if config.runtime.auto_compact {
                format!(
                    "auto at ~{} / {} tokens, keep {} recent messages",
                    config
                        .runtime
                        .compact_trigger_tokens
                        .unwrap_or(config.runtime.context_tokens.unwrap_or(128_000) * 3 / 4),
                    config.runtime.context_tokens.unwrap_or(128_000),
                    config.runtime.compact_preserve_recent_messages.unwrap_or(8),
                )
            } else {
                "disabled".to_string()
            }
        ),
    ];
    if let Some(warning) = &store_handle.warning {
        sidebar.push(format!("warning: {warning}"));
    }
    if let Some(memory_slot) = plugin_plan.slots.memory.as_deref() {
        sidebar.push(format!("memory slot: {memory_slot}"));
    }
    for diagnostic in &plugin_plan.diagnostics {
        let level = match diagnostic.level {
            PluginDiagnosticLevel::Warning => "plugin warning",
            PluginDiagnosticLevel::Error => "plugin error",
        };
        sidebar.push(format!("{level}: {}", diagnostic.message));
    }
    for warning in driver_warnings {
        sidebar.push(format!("driver warning: {warning}"));
    }
    if !skill_names.is_empty() {
        sidebar.push(format!("skill names: {}", preview_list(skill_names, 4)));
    }
    if !mcp_servers.is_empty() {
        sidebar.push(format!(
            "mcp names: {}",
            preview_list(
                &mcp_servers
                    .iter()
                    .map(|server| server.server_name.clone())
                    .collect::<Vec<_>>(),
                4,
            )
        ));
    }
    sidebar.push(
        "commands: /status /runs [query] /run <id> /export_run <id> <path> /compact [/notes] /skills /skill <name>"
            .to_string(),
    );

    TuiStartupSummary {
        sidebar_title: "Overview".to_string(),
        sidebar,
        status: "Ready. /status restores the startup overview.".to_string(),
    }
}

fn preview_list(items: &[String], max_items: usize) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    let mut preview = items.iter().take(max_items).cloned().collect::<Vec<_>>();
    if items.len() > max_items {
        preview.push(format!("+{}", items.len() - max_items));
    }
    preview.join(", ")
}

fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
}
