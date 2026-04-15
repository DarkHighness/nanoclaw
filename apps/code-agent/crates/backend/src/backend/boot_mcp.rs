use crate::ui::{
    LoadedMcpPrompt, LoadedMcpResource, McpPromptSummary, McpResourceSummary, McpServerSummary,
    StartupDiagnosticsSnapshot,
};
use agent::DriverActivationOutcome;
use agent::mcp::{
    ConnectedMcpServer, McpConnectOptions, McpNetworkPolicyConfig, McpPrompt, McpPromptArgument,
    McpResource, McpServerConfig, McpTransportConfig, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_configured_options,
};
use agent::tools::{
    McpToolAdapter, NetworkAllowlist, NetworkPolicy, SandboxPolicy, Tool, ToolRegistry,
};
use agent::types::{
    HookHostApiGrant, HookMutationPermission, HookNetworkPolicy, ToolOrigin, ToolSpec,
};
use anyhow::{Result, anyhow};
use futures::{StreamExt, stream};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const MCP_CONNECT_CONCURRENCY_LIMIT: usize = 8;

#[derive(Default)]
pub struct PreparedMcpConnectionOutcome {
    pub connected: Vec<(ConnectedMcpServer, Vec<McpToolAdapter>)>,
    pub failures: BTreeMap<String, String>,
}

pub fn build_startup_diagnostics_snapshot(
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

pub fn summarize_mcp_servers(
    configured_servers: &[McpServerConfig],
    connected_servers: &[ConnectedMcpServer],
    failures: &BTreeMap<String, String>,
) -> Vec<McpServerSummary> {
    let mut summaries = Vec::new();
    let mut seen = BTreeSet::new();
    let connected_by_name = connected_servers
        .iter()
        .map(|server| (server.server_name.to_string(), server))
        .collect::<BTreeMap<_, _>>();

    for server in configured_servers {
        let name = server.name.to_string();
        let connected = connected_by_name.get(&name).copied();
        seen.insert(name.clone());
        summaries.push(McpServerSummary {
            server_name: name.clone(),
            transport: transport_label(&server.transport).to_string(),
            enabled: server.enabled,
            connected: connected.is_some(),
            tool_count: connected.map_or(0, |value| value.catalog.tools.len()),
            prompt_count: connected.map_or(0, |value| value.catalog.prompts.len()),
            resource_count: connected.map_or(0, |value| value.catalog.resources.len()),
            last_error: failures.get(&name).cloned(),
        });
    }

    for server in connected_servers {
        let name = server.server_name.to_string();
        if seen.contains(&name) {
            continue;
        }
        summaries.push(McpServerSummary {
            server_name: name,
            transport: transport_label_from_connected(server).to_string(),
            enabled: true,
            connected: true,
            tool_count: server.catalog.tools.len(),
            prompt_count: server.catalog.prompts.len(),
            resource_count: server.catalog.resources.len(),
            last_error: None,
        });
    }

    summaries
}

pub async fn connect_and_prepare_mcp_servers(
    configs: Vec<(McpServerConfig, McpConnectOptions)>,
) -> PreparedMcpConnectionOutcome {
    let mut indexed = stream::iter(configs.into_iter().enumerate().map(
        |(index, (server, options))| async move {
            let server_name = server.name.to_string();
            let connected =
                connect_and_catalog_mcp_servers_with_configured_options(vec![(server, options)])
                    .await;
            let outcome = match connected {
                Ok(mut connected) => {
                    let server = connected
                        .pop()
                        .expect("single-server MCP connection should return one catalog");
                    match catalog_tools_as_registry_entries(server.client.clone()).await {
                        Ok(adapters) => Ok((server, adapters)),
                        Err(error) => Err((server_name, error.to_string())),
                    }
                }
                Err(error) => Err((server_name, error.to_string())),
            };
            (index, outcome)
        },
    ))
    .buffer_unordered(MCP_CONNECT_CONCURRENCY_LIMIT)
    .collect::<Vec<_>>()
    .await;
    indexed.sort_by_key(|(index, _)| *index);

    let mut outcome = PreparedMcpConnectionOutcome::default();
    for (_, result) in indexed {
        match result {
            Ok(connected) => outcome.connected.push(connected),
            Err((name, error)) => {
                outcome.failures.insert(name, error);
            }
        }
    }
    outcome
}

pub fn filter_mcp_tool_conflicts(
    registry: &ToolRegistry,
    connected_servers: Vec<(ConnectedMcpServer, Vec<McpToolAdapter>)>,
) -> PreparedMcpConnectionOutcome {
    let mut outcome = PreparedMcpConnectionOutcome::default();
    for (server, adapters) in connected_servers {
        let conflicts = adapters
            .iter()
            .map(|adapter| adapter.spec().name.to_string())
            .filter(|name| registry.get(name.as_str()).is_some())
            .collect::<BTreeSet<_>>();
        if conflicts.is_empty() {
            outcome.connected.push((server, adapters));
            continue;
        }
        outcome.failures.insert(
            server.server_name.to_string(),
            format!(
                "MCP tool names conflict with existing registry entries: {}",
                conflicts.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }
    outcome
}

pub fn list_mcp_servers(servers: &[ConnectedMcpServer]) -> Vec<McpServerSummary> {
    summarize_mcp_servers(&[], servers, &BTreeMap::new())
}

pub fn mcp_connection_sandbox_policy(
    base: &SandboxPolicy,
    server: &McpServerConfig,
) -> SandboxPolicy {
    if !matches!(server.transport, McpTransportConfig::Stdio { .. }) {
        return base.clone();
    }

    if server.bootstrap_network.is_none() && server.runtime_network.is_none() {
        // Default stdio MCP servers behave more like host-managed integrations
        // than one-shot model tool invocations. Keeping them inside the normal
        // workspace sandbox breaks common launch paths (`pnpm dlx`, user npm
        // mirrors, caches under HOME, browser helper downloads, etc.). The
        // trust-first default is therefore "host managed unless the operator
        // explicitly tightens this server".
        return SandboxPolicy::permissive().with_fail_if_unavailable(base.fail_if_unavailable);
    }

    let mut policy = base.clone();

    // Direct stdio launchers such as `pnpm dlx` perform bootstrap and runtime
    // work inside one long-lived subprocess. Until built-in MCP installs move
    // to a managed cache, the effective sandbox network policy must cover both
    // phases, so we merge bootstrap/runtime intents instead of trying to apply
    // them separately during connect.
    if let Some(bootstrap) = configured_mcp_network(server.bootstrap_network.as_ref())
        .or_else(|| default_mcp_bootstrap_network(server))
    {
        policy.network = merge_network_policy(&policy.network, &bootstrap);
    }
    if let Some(runtime) = configured_mcp_network(server.runtime_network.as_ref())
        .or_else(|| default_mcp_runtime_network(server))
    {
        policy.network = merge_network_policy(&policy.network, &runtime);
    }
    policy
}

fn configured_mcp_network(policy: Option<&McpNetworkPolicyConfig>) -> Option<NetworkPolicy> {
    match policy? {
        McpNetworkPolicyConfig::Off => Some(NetworkPolicy::Off),
        McpNetworkPolicyConfig::Full => Some(NetworkPolicy::Full),
        McpNetworkPolicyConfig::Allowlist { domains, cidrs } => {
            let allowlist = NetworkAllowlist {
                domains: domains.clone(),
                cidrs: cidrs.clone(),
            };
            (!allowlist.is_empty()).then_some(NetworkPolicy::Allowlist(allowlist))
        }
    }
}

fn merge_network_policy(base: &NetworkPolicy, overlay: &NetworkPolicy) -> NetworkPolicy {
    match (base, overlay) {
        (NetworkPolicy::Full, _) | (_, NetworkPolicy::Full) => NetworkPolicy::Full,
        (NetworkPolicy::Off, policy) | (policy, NetworkPolicy::Off) => policy.clone(),
        (NetworkPolicy::Allowlist(left), NetworkPolicy::Allowlist(right)) => {
            NetworkPolicy::Allowlist(NetworkAllowlist {
                domains: left
                    .domains
                    .iter()
                    .chain(right.domains.iter())
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                cidrs: left
                    .cidrs
                    .iter()
                    .chain(right.cidrs.iter())
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
        }
    }
}

fn default_mcp_bootstrap_network(server: &McpServerConfig) -> Option<NetworkPolicy> {
    let McpTransportConfig::Stdio { command, .. } = &server.transport else {
        return None;
    };
    match command.as_str() {
        "pnpm" | "npx" | "bunx" | "npm" => Some(NetworkPolicy::Allowlist(
            NetworkAllowlist::with_domains(vec!["registry.npmjs.org".to_string()]),
        )),
        _ => None,
    }
}

fn default_mcp_runtime_network(server: &McpServerConfig) -> Option<NetworkPolicy> {
    match server.transport {
        // User-managed local MCP integrations are effectively trusted host
        // services unless the operator tightens them explicitly. Defaulting to
        // `off` would make most third-party stdio MCP entries unusable because
        // operators rarely know their full egress set ahead of time.
        McpTransportConfig::Stdio { .. } => Some(NetworkPolicy::Full),
        McpTransportConfig::StreamableHttp { .. } => None,
    }
}

fn transport_label(transport: &McpTransportConfig) -> &'static str {
    match transport {
        McpTransportConfig::Stdio { .. } => "stdio",
        McpTransportConfig::StreamableHttp { .. } => "http",
    }
}

fn transport_label_from_connected(server: &ConnectedMcpServer) -> &'static str {
    match server.boundary.transport {
        agent::types::McpTransportKind::Stdio => "stdio",
        agent::types::McpTransportKind::StreamableHttp => "http",
    }
}

pub fn list_mcp_prompts(servers: &[ConnectedMcpServer]) -> Vec<McpPromptSummary> {
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

pub fn list_mcp_resources(servers: &[ConnectedMcpServer]) -> Vec<McpResourceSummary> {
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

pub async fn load_mcp_prompt(
    servers: &[ConnectedMcpServer],
    server_name: &str,
    prompt_name: &str,
) -> Result<LoadedMcpPrompt> {
    let server = find_server(servers, server_name)?;
    let prompt = server.client.get_prompt(prompt_name, Value::Null).await?;
    Ok(LoadedMcpPrompt {
        input_text: prompt_to_text(&prompt),
        input_messages: prompt.messages.clone(),
        server_name: server_name.to_string(),
        prompt_name: prompt_name.to_string(),
        arguments_summary: render_prompt_argument_names(&prompt.arguments),
    })
}

pub async fn load_mcp_resource(
    servers: &[ConnectedMcpServer],
    server_name: &str,
    uri: &str,
) -> Result<LoadedMcpResource> {
    let server = find_server(servers, server_name)?;
    let resource = server.client.read_resource(uri).await?;
    Ok(LoadedMcpResource {
        input_text: resource_to_text(&resource),
        input_parts: resource.parts.clone(),
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
        mcp_connection_sandbox_policy, prompt_to_text, resource_to_text, summarize_mcp_servers,
    };
    use agent::DriverActivationOutcome;
    use agent::mcp::{McpPrompt, McpResource, McpServerConfig, McpTransportConfig};
    use agent::tools::{
        FilesystemPolicy, HostEscapePolicy, NetworkPolicy, SandboxMode, SandboxPolicy,
    };
    use agent::types::{
        Message, MessagePart, MessageRole, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;
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
                    freeform_availability: None,
                    output_mode: ToolOutputMode::Text,
                    output_schema: None,
                    defer_loading: false,
                    origin: ToolOrigin::Local,
                    source: ToolSource::Builtin,
                    aliases: Vec::new(),
                    supports_parallel_tool_calls: false,
                    availability: Default::default(),
                    approval: Default::default(),
                    mcp_boundary: None,
                    mcp_server_boundaries: Default::default(),
                },
                ToolSpec {
                    name: "remote".into(),
                    description: "remote".to_string(),
                    kind: Default::default(),
                    input_schema: Some(serde_json::json!({})),
                    freeform_format: None,
                    freeform_availability: None,
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
                    mcp_boundary: None,
                    mcp_server_boundaries: Default::default(),
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

    #[test]
    fn stdio_mcp_without_explicit_network_config_defaults_to_host_managed_policy() {
        let workspace_root = PathBuf::from("/tmp/workspace");
        let base = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: vec![workspace_root.clone()],
                writable_roots: vec![workspace_root.clone()],
                executable_roots: vec![workspace_root.clone()],
                protected_paths: vec![workspace_root.join(".git")],
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: true,
        };
        let server = McpServerConfig {
            name: "context7".into(),
            enabled: true,
            bootstrap_network: None,
            runtime_network: None,
            transport: McpTransportConfig::Stdio {
                command: "pnpm".to_string(),
                args: vec![
                    "dlx".to_string(),
                    "@upstash/context7-mcp@latest".to_string(),
                ],
                env: BTreeMap::new(),
                cwd: None,
            },
        };

        let widened = mcp_connection_sandbox_policy(&base, &server);

        assert_eq!(
            widened,
            SandboxPolicy::permissive().with_fail_if_unavailable(true)
        );
    }

    #[test]
    fn summarize_mcp_servers_keeps_disconnected_servers_visible_with_errors() {
        let configured = vec![McpServerConfig {
            name: "gh_grep".into(),
            enabled: true,
            bootstrap_network: None,
            runtime_network: None,
            transport: McpTransportConfig::StreamableHttp {
                url: "https://mcp.grep.app".to_string(),
                headers: BTreeMap::new(),
            },
        }];
        let failures =
            BTreeMap::from([("gh_grep".to_string(), "connection timed out".to_string())]);

        assert_eq!(
            summarize_mcp_servers(&configured, &[], &failures),
            vec![McpServerSummary {
                server_name: "gh_grep".to_string(),
                transport: "http".to_string(),
                enabled: true,
                connected: false,
                tool_count: 0,
                prompt_count: 0,
                resource_count: 0,
                last_error: Some("connection timed out".to_string()),
            }]
        );
    }
}
