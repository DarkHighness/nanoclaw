use agent::DriverActivationOutcome;
use agent::mcp::{ConnectedMcpServer, McpPrompt, McpPromptArgument, McpResource};
use agent::types::{
    HookHostApiGrant, HookMutationPermission, HookNetworkPolicy, ToolOrigin, ToolSpec,
};
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupDiagnosticsSnapshot {
    pub(crate) local_tool_count: usize,
    pub(crate) mcp_tool_count: usize,
    pub(crate) enabled_plugin_count: usize,
    pub(crate) total_plugin_count: usize,
    pub(crate) mcp_servers: Vec<McpServerSummary>,
    pub(crate) plugin_details: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct McpServerSummary {
    pub(crate) server_name: String,
    pub(crate) tool_count: usize,
    pub(crate) prompt_count: usize,
    pub(crate) resource_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct McpPromptSummary {
    pub(crate) server_name: String,
    pub(crate) prompt_name: String,
    pub(crate) description: String,
    pub(crate) argument_names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct McpResourceSummary {
    pub(crate) server_name: String,
    pub(crate) uri: String,
    pub(crate) mime_type: Option<String>,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadedMcpPrompt {
    pub(crate) input_text: String,
    pub(crate) server_name: String,
    pub(crate) prompt_name: String,
    pub(crate) arguments_summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadedMcpResource {
    pub(crate) input_text: String,
    pub(crate) server_name: String,
    pub(crate) uri: String,
    pub(crate) mime_summary: String,
}

pub(crate) fn build_startup_diagnostics_snapshot(
    workspace_root: &Path,
    tool_specs: &[ToolSpec],
    connected_mcp_servers: &[ConnectedMcpServer],
    plugin_plan: &agent::plugins::PluginActivationPlan,
    startup_warnings: &[String],
    driver_outcome: &DriverActivationOutcome,
) -> StartupDiagnosticsSnapshot {
    let local_tool_count = tool_specs
        .iter()
        .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
        .count();
    let mcp_tool_count = tool_specs.len().saturating_sub(local_tool_count);

    let mut plugin_details = Vec::new();
    if let Some(memory_slot) = plugin_plan.slots.memory.as_ref() {
        plugin_details.push(format!("memory slot: {memory_slot}"));
    }
    for plugin in plugin_plan
        .plugin_states
        .iter()
        .filter(|state| state.enabled)
    {
        plugin_details.push(format!(
            "plugin {}: {}",
            plugin.plugin_id,
            describe_plugin_contributions(plugin)
        ));
        plugin_details.push(format!(
            "plugin {} perms: {}",
            plugin.plugin_id,
            describe_plugin_permissions(workspace_root, plugin)
        ));
    }
    for diagnostic in &plugin_plan.diagnostics {
        let level = match diagnostic.level {
            agent::plugins::PluginDiagnosticLevel::Warning => "plugin warning",
            agent::plugins::PluginDiagnosticLevel::Error => "plugin error",
        };
        plugin_details.push(format!("{level}: {}", diagnostic.message));
    }

    let mut warnings = startup_warnings.to_vec();
    warnings.extend(driver_outcome.warnings.clone());

    StartupDiagnosticsSnapshot {
        local_tool_count,
        mcp_tool_count,
        enabled_plugin_count: plugin_plan
            .plugin_states
            .iter()
            .filter(|state| state.enabled)
            .count(),
        total_plugin_count: plugin_plan.plugin_states.len(),
        mcp_servers: list_mcp_servers(connected_mcp_servers),
        plugin_details,
        warnings,
        diagnostics: driver_outcome.diagnostics.clone(),
    }
}

pub(crate) fn list_mcp_servers(servers: &[ConnectedMcpServer]) -> Vec<McpServerSummary> {
    servers
        .iter()
        .map(|server| McpServerSummary {
            server_name: server.server_name.to_string(),
            tool_count: server.catalog.tools.len(),
            prompt_count: server.catalog.prompts.len(),
            resource_count: server.catalog.resources.len(),
        })
        .collect()
}

pub(crate) fn list_mcp_prompts(servers: &[ConnectedMcpServer]) -> Vec<McpPromptSummary> {
    servers
        .iter()
        .flat_map(|server| {
            server
                .catalog
                .prompts
                .iter()
                .map(|prompt| McpPromptSummary {
                    server_name: server.server_name.to_string(),
                    prompt_name: prompt.name.clone(),
                    description: prompt.description.clone(),
                    argument_names: prompt_argument_names(&prompt.arguments),
                })
        })
        .collect()
}

pub(crate) fn list_mcp_resources(servers: &[ConnectedMcpServer]) -> Vec<McpResourceSummary> {
    servers
        .iter()
        .flat_map(|server| {
            server
                .catalog
                .resources
                .iter()
                .map(|resource| McpResourceSummary {
                    server_name: server.server_name.to_string(),
                    uri: resource.uri.clone(),
                    mime_type: resource.mime_type.clone(),
                    description: resource.description.clone(),
                })
        })
        .collect()
}

pub(crate) async fn load_mcp_prompt(
    servers: &[ConnectedMcpServer],
    server_name: &str,
    prompt_name: &str,
) -> Result<LoadedMcpPrompt> {
    let server = find_server(servers, server_name)?;
    let prompt = server.client.get_prompt(prompt_name, Value::Null).await?;
    Ok(LoadedMcpPrompt {
        input_text: prompt_to_text(&prompt),
        server_name: server_name.to_string(),
        prompt_name: prompt_name.to_string(),
        arguments_summary: render_prompt_argument_names(&prompt.arguments),
    })
}

pub(crate) async fn load_mcp_resource(
    servers: &[ConnectedMcpServer],
    server_name: &str,
    uri: &str,
) -> Result<LoadedMcpResource> {
    let server = find_server(servers, server_name)?;
    let resource = server.client.read_resource(uri).await?;
    Ok(LoadedMcpResource {
        input_text: resource_to_text(&resource),
        server_name: server_name.to_string(),
        uri: resource.uri,
        mime_summary: resource.mime_type.unwrap_or_else(|| "unknown".to_string()),
    })
}

fn find_server<'a>(
    servers: &'a [ConnectedMcpServer],
    server_name: &str,
) -> Result<&'a ConnectedMcpServer> {
    servers
        .iter()
        .find(|server| server.server_name.as_str() == server_name)
        .ok_or_else(|| anyhow!("unknown MCP server: {server_name}"))
}

fn prompt_argument_names(arguments: &[McpPromptArgument]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| {
            if argument.required {
                format!("{}*", argument.name)
            } else {
                argument.name.clone()
            }
        })
        .collect()
}

fn render_prompt_argument_names(arguments: &[McpPromptArgument]) -> String {
    if arguments.is_empty() {
        "none".to_string()
    } else {
        prompt_argument_names(arguments).join(", ")
    }
}

fn prompt_to_text(prompt: &McpPrompt) -> String {
    if prompt.messages.is_empty() {
        return prompt.description.clone();
    }
    prompt
        .messages
        .iter()
        .map(agent::types::message_operator_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn resource_to_text(resource: &McpResource) -> String {
    let parts = resource
        .parts
        .iter()
        .map(agent::types::message_part_operator_text)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        resource.description.clone()
    } else {
        parts.join("\n\n")
    }
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
    if contributions.custom_tool_root_count > 0 {
        parts.push(format!(
            "custom_tools={}",
            contributions.custom_tool_root_count
        ));
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
    if let Some(driver) = contributions.runtime_driver.as_ref() {
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

fn preview_list<T>(items: &[T], max_items: usize) -> String
where
    T: std::fmt::Display,
{
    if items.is_empty() {
        return "none".to_string();
    }
    let mut preview = items
        .iter()
        .take(max_items)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if items.len() > max_items {
        preview.push(format!("+{}", items.len() - max_items));
    }
    preview.join(", ")
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
        "none".to_string()
    } else {
        grants
            .iter()
            .map(describe_host_api_grant)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn describe_host_api_grant(grant: &HookHostApiGrant) -> String {
    match grant {
        HookHostApiGrant::GetHookContext => "get_hook_context",
        HookHostApiGrant::EmitHookEffect => "emit_hook_effect",
        HookHostApiGrant::Log => "log",
        HookHostApiGrant::ReadFile => "read_file",
        HookHostApiGrant::WriteFile => "write_file",
        HookHostApiGrant::ListDir => "list_dir",
        HookHostApiGrant::SpawnMcp => "spawn_mcp",
        HookHostApiGrant::ResolveSkill => "resolve_skill",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        McpServerSummary, StartupDiagnosticsSnapshot, build_startup_diagnostics_snapshot,
        prompt_to_text, resource_to_text,
    };
    use agent::DriverActivationOutcome;
    use agent::mcp::{McpPrompt, McpResource};
    use agent::types::{
        Message, MessagePart, MessageRole, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec,
    };
    use tempfile::tempdir;

    #[test]
    fn startup_diagnostics_separate_local_and_mcp_tool_counts() {
        let dir = tempdir().unwrap();
        let snapshot = build_startup_diagnostics_snapshot(
            dir.path(),
            &[
                ToolSpec {
                    name: "read".into(),
                    description: "read".to_string(),
                    kind: Default::default(),
                    input_schema: Some(serde_json::json!({})),
                    freeform_format: None,
                    output_mode: ToolOutputMode::Text,
                    output_schema: None,
                    defer_loading: false,
                    origin: ToolOrigin::Local,
                    source: ToolSource::Builtin,
                    aliases: Vec::new(),
                    supports_parallel_tool_calls: false,
                    availability: Default::default(),
                    approval: Default::default(),
                },
                ToolSpec {
                    name: "remote".into(),
                    description: "remote".to_string(),
                    kind: Default::default(),
                    input_schema: Some(serde_json::json!({})),
                    freeform_format: None,
                    output_mode: ToolOutputMode::Text,
                    output_schema: None,
                    defer_loading: false,
                    origin: ToolOrigin::Mcp {
                        server_name: "fs".into(),
                    },
                    source: ToolSource::McpTool {
                        server_name: "fs".into(),
                    },
                    aliases: Vec::new(),
                    supports_parallel_tool_calls: false,
                    availability: Default::default(),
                    approval: Default::default(),
                },
            ],
            &[],
            &agent::plugins::PluginActivationPlan::default(),
            &["sandbox unavailable".to_string()],
            &DriverActivationOutcome {
                warnings: vec!["slow startup".to_string()],
                diagnostics: vec!["validated wasm hook module".to_string()],
                ..Default::default()
            },
        );

        assert_eq!(
            snapshot,
            StartupDiagnosticsSnapshot {
                local_tool_count: 1,
                mcp_tool_count: 1,
                enabled_plugin_count: 0,
                total_plugin_count: 0,
                mcp_servers: Vec::<McpServerSummary>::new(),
                plugin_details: Vec::new(),
                warnings: vec![
                    "sandbox unavailable".to_string(),
                    "slow startup".to_string()
                ],
                diagnostics: vec!["validated wasm hook module".to_string()],
            }
        );
    }

    #[test]
    fn prompt_and_resource_text_reuse_operator_message_rendering() {
        let prompt = McpPrompt {
            name: "review".to_string(),
            title: None,
            description: "fallback".to_string(),
            arguments: Vec::new(),
            messages: vec![Message::new(
                MessageRole::User,
                vec![
                    MessagePart::ImageUrl {
                        url: "https://example.com/failure.png".to_string(),
                        mime_type: None,
                    },
                    MessagePart::ToolCall {
                        call: agent::types::ToolCall {
                            id: agent::types::ToolCallId::new(),
                            call_id: agent::types::CallId::new(),
                            tool_name: "read".into(),
                            arguments: serde_json::Value::Null,
                            origin: agent::types::ToolOrigin::Local,
                        },
                    },
                ],
            )],
        };
        let resource = McpResource {
            uri: "file://resource".to_string(),
            name: "resource".to_string(),
            title: None,
            description: "fallback resource".to_string(),
            mime_type: Some("application/json".to_string()),
            parts: vec![MessagePart::Reference {
                kind: "mention".to_string(),
                name: Some("connector".to_string()),
                uri: Some("app://connector".to_string()),
                text: Some("Follow this reference".to_string()),
            }],
        };

        assert_eq!(
            prompt_to_text(&prompt),
            "[image_url:https://example.com/failure.png]\n[tool_call:read]"
        );
        assert_eq!(
            resource_to_text(&resource),
            "[reference:mention connector app://connector Follow this reference]"
        );
    }
}
