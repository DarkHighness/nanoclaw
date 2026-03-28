use super::StoreHandle;
use crate::{TuiStartupSummary, config::AgentCoreConfig};
use agent::mcp::ConnectedMcpServer;
use agent::plugins::{PluginActivationPlan, PluginDiagnosticLevel};
use std::path::{Path, PathBuf};
use tools::{SandboxBackendStatus, SandboxPolicy, describe_sandbox_policy};
use types::{HookHostApiGrant, HookMutationPermission, HookNetworkPolicy};
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
    driver_diagnostics: &[String],
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
            if config.primary_profile.auto_compact {
                format!(
                    "auto at ~{} / {} tokens, keep {} recent messages",
                    config.primary_profile.compact_trigger_tokens,
                    config.primary_profile.context_window_tokens,
                    config.primary_profile.compact_preserve_recent_messages,
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
    for plugin in plugin_plan
        .plugin_states
        .iter()
        .filter(|state| state.enabled)
    {
        sidebar.push(format!(
            "plugin {}: {}",
            plugin.plugin_id,
            describe_plugin_contributions(plugin)
        ));
        sidebar.push(format!(
            "plugin {} perms: {}",
            plugin.plugin_id,
            describe_plugin_permissions(workspace_root, plugin)
        ));
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
    for diagnostic in driver_diagnostics {
        sidebar.push(format!("driver diagnostic: {diagnostic}"));
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

fn describe_plugin_contributions(plugin: &agent::plugins::PluginState) -> String {
    let mut parts = Vec::new();
    let contributions = &plugin.contributions;
    if contributions.instruction_count > 0 {
        parts.push(format!("instructions={}", contributions.instruction_count));
    }
    if !contributions.skill_roots.is_empty() {
        parts.push(format!("skills={}", contributions.skill_roots.len()));
    }
    if !contributions.hook_names.is_empty() {
        parts.push(format!(
            "hooks={}",
            preview_list(&contributions.hook_names, 2)
        ));
    }
    if !contributions.mcp_servers.is_empty() {
        parts.push(format!(
            "mcp={}",
            preview_list(&contributions.mcp_servers, 2)
        ));
    }
    if let Some(driver) = contributions.runtime_driver.as_deref() {
        parts.push(format!("runtime={driver}"));
    }
    if parts.is_empty() {
        "no declarative or runtime contributions".to_string()
    } else {
        parts.join(", ")
    }
}

fn describe_plugin_permissions(
    workspace_root: &Path,
    plugin: &agent::plugins::PluginState,
) -> String {
    let permissions = &plugin.granted_permissions;
    let mut parts = Vec::new();
    parts.push(format!(
        "read={}",
        preview_paths(workspace_root, &permissions.read_roots)
    ));
    parts.push(format!(
        "write={}",
        preview_paths(workspace_root, &permissions.write_roots)
    ));
    parts.push(format!(
        "exec={}",
        preview_paths(workspace_root, &permissions.exec_roots)
    ));
    parts.push(format!(
        "network={}",
        describe_network_policy(&permissions.network)
    ));
    parts.push(format!(
        "mutation={}",
        describe_mutation_permission(permissions.message_mutation)
    ));
    parts.push(format!(
        "host_api={}",
        describe_host_api_grants(&permissions.host_api)
    ));
    parts.join(", ")
}

fn preview_paths(workspace_root: &Path, paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "none".to_string();
    }
    let preview = paths
        .iter()
        .take(2)
        .map(|path| {
            path.strip_prefix(workspace_root)
                .unwrap_or(path.as_path())
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();
    if paths.len() > 2 {
        format!("{}, +{}", preview.join(", "), paths.len() - 2)
    } else {
        preview.join(", ")
    }
}

fn describe_network_policy(policy: &HookNetworkPolicy) -> String {
    match policy {
        HookNetworkPolicy::Deny => "deny".to_string(),
        HookNetworkPolicy::Allow => "allow".to_string(),
        HookNetworkPolicy::AllowDomains { domains } => {
            if domains.is_empty() {
                "allow_domains".to_string()
            } else {
                format!("allow_domains({})", preview_list(domains, 2))
            }
        }
    }
}

fn describe_mutation_permission(permission: HookMutationPermission) -> &'static str {
    match permission {
        HookMutationPermission::Deny => "deny",
        HookMutationPermission::Allow => "allow",
        HookMutationPermission::ReviewRequired => "review_required",
    }
}

fn describe_host_api_grants(grants: &[HookHostApiGrant]) -> String {
    if grants.is_empty() {
        return "none".to_string();
    }
    let names = grants
        .iter()
        .map(|grant| match grant {
            HookHostApiGrant::GetHookContext => "get_hook_context".to_string(),
            HookHostApiGrant::EmitHookEffect => "emit_hook_effect".to_string(),
            HookHostApiGrant::Log => "log".to_string(),
            HookHostApiGrant::ReadFile => "read_file".to_string(),
            HookHostApiGrant::WriteFile => "write_file".to_string(),
            HookHostApiGrant::ListDir => "list_dir".to_string(),
            HookHostApiGrant::SpawnMcp => "spawn_mcp".to_string(),
            HookHostApiGrant::ResolveSkill => "resolve_skill".to_string(),
        })
        .collect::<Vec<_>>();
    preview_list(&names, 3)
}

#[cfg(test)]
mod tests {
    use super::StoreHandle;
    use super::build_startup_summary;
    use crate::config::AgentCoreConfig;
    use agent::plugins::{
        PluginActivationPlan, PluginContributionSummary, PluginResolvedPermissions, PluginState,
    };
    use std::sync::Arc;
    use store::InMemoryRunStore;
    use tempfile::tempdir;
    use tools::{
        HostEscapePolicy, NetworkPolicy, SandboxBackendStatus, SandboxMode, SandboxPolicy,
    };
    use types::{
        HookHostApiGrant, HookMutationPermission, HookNetworkPolicy, RunId, ToolOrigin,
        ToolOutputMode, ToolSpec,
    };

    #[test]
    fn startup_summary_lists_enabled_plugin_contributions_and_permissions() {
        let workspace = tempdir().unwrap();
        let summary = build_startup_summary(
            &RunId::from("run_test"),
            workspace.path(),
            "openai / gpt-test",
            &StoreHandle {
                store: Arc::new(InMemoryRunStore::new()),
                label: "memory fallback".to_string(),
                warning: None,
            },
            0,
            &[ToolSpec {
                name: "read".into(),
                description: "read".to_string(),
                input_schema: serde_json::json!({"type":"object"}),
                output_mode: ToolOutputMode::Text,
                output_schema: None,
                origin: ToolOrigin::Local,
                annotations: Default::default(),
            }],
            &[],
            &[],
            &AgentCoreConfig::default(),
            &PluginActivationPlan {
                plugin_states: vec![PluginState {
                    plugin_id: "team-policy".to_string(),
                    enabled: true,
                    reason: "enabled by plugins.entries".to_string(),
                    requested_permissions: Default::default(),
                    granted_permissions: PluginResolvedPermissions {
                        read_roots: vec![workspace.path().join("docs")],
                        write_roots: vec![
                            workspace.path().join(".nanoclaw/plugin-state/team-policy"),
                        ],
                        exec_roots: vec![
                            workspace.path().join(".nanoclaw/plugins-cache/team-policy"),
                        ],
                        network: HookNetworkPolicy::AllowDomains {
                            domains: vec!["api.example.com".to_string()],
                        },
                        message_mutation: HookMutationPermission::Allow,
                        host_api: vec![
                            HookHostApiGrant::ReadFile,
                            HookHostApiGrant::EmitHookEffect,
                        ],
                    },
                    contributions: PluginContributionSummary {
                        instruction_count: 1,
                        skill_roots: vec![workspace.path().join("plugins/team-policy/skills")],
                        hook_names: vec!["rewrite-user-message".to_string()],
                        mcp_servers: vec!["docs".to_string()],
                        runtime_driver: Some("builtin.wasm-hook-validator".to_string()),
                    },
                }],
                ..PluginActivationPlan::default()
            },
            &[],
            &[],
            &SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                filesystem: Default::default(),
                network: NetworkPolicy::Off,
                host_escape: HostEscapePolicy::Deny,
                fail_if_unavailable: false,
            },
            &SandboxBackendStatus::NotRequired,
        );

        assert!(summary
            .sidebar
            .iter()
            .any(|line| line.contains("plugin team-policy: instructions=1, skills=1, hooks=rewrite-user-message, mcp=docs, runtime=builtin.wasm-hook-validator")));
        assert!(summary
            .sidebar
            .iter()
            .any(|line| line.contains("plugin team-policy perms: read=docs, write=.nanoclaw/plugin-state/team-policy, exec=.nanoclaw/plugins-cache/team-policy, network=allow_domains(api.example.com), mutation=allow, host_api=read_file, emit_hook_effect")));
    }

    #[test]
    fn startup_summary_includes_driver_warnings_and_diagnostics() {
        let workspace = tempdir().unwrap();
        let summary = build_startup_summary(
            &RunId::from("run_test"),
            workspace.path(),
            "openai / gpt-test",
            &StoreHandle {
                store: Arc::new(InMemoryRunStore::new()),
                label: "memory fallback".to_string(),
                warning: None,
            },
            0,
            &[],
            &[],
            &[],
            &AgentCoreConfig::default(),
            &PluginActivationPlan::default(),
            &["slow startup".to_string()],
            &["validated wasm hook module".to_string()],
            &SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                filesystem: Default::default(),
                network: NetworkPolicy::Off,
                host_escape: HostEscapePolicy::Deny,
                fail_if_unavailable: false,
            },
            &SandboxBackendStatus::NotRequired,
        );

        assert!(
            summary
                .sidebar
                .iter()
                .any(|line| line == "driver warning: slow startup")
        );
        assert!(
            summary
                .sidebar
                .iter()
                .any(|line| line == "driver diagnostic: validated wasm hook module")
        );
    }
}
