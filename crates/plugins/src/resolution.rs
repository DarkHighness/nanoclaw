use crate::config::{PluginEntryConfig, PluginPermissionGrant, PluginResolverConfig};
use crate::discovery::{DiscoveredPlugin, PluginDiagnostic, PluginDiscovery};
use crate::manifest::{
    PluginCapabilitySet, PluginKind, PluginNetworkAccess, PluginPermissionRequest,
    PluginRuntimeSpec, PluginToolPolicyCapability,
};
use mcp::McpServerConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use toml::map::Map;
use types::{
    HookEffectPolicy, HookExecutionPolicy, HookHostApiGrant, HookMutationPermission,
    HookNetworkPolicy, HookRegistration,
};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginResolvedPermissions {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub exec_roots: Vec<PathBuf>,
    pub network: HookNetworkPolicy,
    pub message_mutation: HookMutationPermission,
    pub host_api: Vec<HookHostApiGrant>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginExecutableActivation {
    pub plugin_id: String,
    pub root_dir: PathBuf,
    pub runtime: PluginRuntimeSpec,
    pub config: Map<String, toml::Value>,
    pub capabilities: PluginCapabilitySet,
    pub granted_permissions: PluginResolvedPermissions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginCustomToolActivation {
    pub plugin_id: String,
    pub root_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub tool_roots: Vec<PathBuf>,
    pub granted_permissions: PluginResolvedPermissions,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginContributionSummary {
    pub instruction_count: usize,
    pub skill_roots: Vec<PathBuf>,
    pub custom_tool_root_count: usize,
    pub hook_names: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub runtime_driver: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginState {
    pub plugin_id: String,
    pub enabled: bool,
    pub reason: String,
    pub requested_permissions: PluginPermissionRequest,
    pub granted_permissions: PluginResolvedPermissions,
    pub contributions: PluginContributionSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginSlotSelection {
    pub memory: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PluginActivationPlan {
    pub instructions: Vec<String>,
    pub skill_roots: Vec<PathBuf>,
    pub custom_tool_activations: Vec<PluginCustomToolActivation>,
    pub hooks: Vec<HookRegistration>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub runtime_activations: Vec<PluginExecutableActivation>,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub plugin_states: Vec<PluginState>,
    pub slots: PluginSlotSelection,
}

pub fn build_activation_plan(
    discovery: PluginDiscovery,
    resolver: &PluginResolverConfig,
    workspace_root: &Path,
) -> PluginActivationPlan {
    let mut plan = PluginActivationPlan {
        diagnostics: discovery.diagnostics,
        ..PluginActivationPlan::default()
    };
    let mut plugins_by_id = discovery
        .plugins
        .into_iter()
        .map(|plugin| (plugin.manifest.id.clone(), plugin))
        .collect::<BTreeMap<_, _>>();

    for id in resolver
        .entries
        .keys()
        .chain(resolver.allow.iter())
        .chain(resolver.deny.iter())
    {
        if !plugins_by_id.contains_key(id) {
            plan.diagnostics.push(PluginDiagnostic::warning(
                "plugin_missing_reference",
                format!("plugin `{id}` is referenced in config but not discovered"),
                Some(id.clone()),
                None,
            ));
        }
    }

    let allow_set = resolver.allow.iter().cloned().collect::<BTreeSet<_>>();
    let deny_set = resolver.deny.iter().cloned().collect::<BTreeSet<_>>();

    for plugin in plugins_by_id.values_mut() {
        let mut state = resolve_plugin_state(plugin, resolver, &allow_set, &deny_set);
        if state.enabled {
            match resolve_plugin_permissions(
                plugin,
                resolver.entries.get(&plugin.manifest.id),
                workspace_root,
            ) {
                Ok(granted_permissions) => {
                    state.requested_permissions = plugin.manifest.permissions.clone();
                    state.granted_permissions = granted_permissions.clone();
                    state.contributions = contribution_summary(plugin);
                    collect_plugin_activation(&mut plan, plugin, resolver, granted_permissions);
                }
                Err(diagnostic) => {
                    state.enabled = false;
                    state.reason = "disabled by invalid plugin permission grants".to_string();
                    plan.diagnostics.push(diagnostic);
                }
            }
        } else {
            state.requested_permissions = plugin.manifest.permissions.clone();
            state.contributions = contribution_summary(plugin);
        }
        plan.plugin_states.push(state);
    }

    resolve_memory_slot(
        &mut plan,
        &plugins_by_id,
        resolver,
        &allow_set,
        &deny_set,
        workspace_root,
    );

    plan.skill_roots.sort();
    plan.skill_roots.dedup();
    plan.plugin_states
        .sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));

    plan
}

fn resolve_plugin_state(
    plugin: &DiscoveredPlugin,
    resolver: &PluginResolverConfig,
    allow_set: &BTreeSet<String>,
    deny_set: &BTreeSet<String>,
) -> PluginState {
    let plugin_id = plugin.manifest.id.clone();
    if !resolver.enabled {
        return disabled_state(plugin_id, "plugins disabled globally", plugin);
    }
    if deny_set.contains(&plugin.manifest.id) {
        return disabled_state(plugin_id, "denied by plugins.deny", plugin);
    }
    if !allow_set.is_empty() && !allow_set.contains(&plugin.manifest.id) {
        return disabled_state(plugin_id, "not present in plugins.allow", plugin);
    }
    if let Some(entry) = resolver.entries.get(&plugin.manifest.id)
        && let Some(enabled) = entry.enabled
    {
        return PluginState {
            plugin_id,
            enabled,
            reason: if enabled {
                "enabled by plugins.entries".to_string()
            } else {
                "disabled by plugins.entries".to_string()
            },
            requested_permissions: plugin.manifest.permissions.clone(),
            granted_permissions: PluginResolvedPermissions::default(),
            contributions: PluginContributionSummary::default(),
        };
    }
    PluginState {
        plugin_id,
        enabled: plugin.manifest.enabled_by_default,
        reason: if plugin.manifest.enabled_by_default {
            "enabled by manifest default".to_string()
        } else {
            "disabled by manifest default".to_string()
        },
        requested_permissions: plugin.manifest.permissions.clone(),
        granted_permissions: PluginResolvedPermissions::default(),
        contributions: PluginContributionSummary::default(),
    }
}

fn disabled_state(plugin_id: String, reason: &str, plugin: &DiscoveredPlugin) -> PluginState {
    PluginState {
        plugin_id,
        enabled: false,
        reason: reason.to_string(),
        requested_permissions: plugin.manifest.permissions.clone(),
        granted_permissions: PluginResolvedPermissions::default(),
        contributions: PluginContributionSummary::default(),
    }
}

fn contribution_summary(plugin: &DiscoveredPlugin) -> PluginContributionSummary {
    PluginContributionSummary {
        instruction_count: plugin.manifest.instructions.len(),
        skill_roots: plugin.skill_roots.clone(),
        custom_tool_root_count: plugin.tool_roots.len(),
        hook_names: plugin.hooks.iter().map(|hook| hook.name.clone()).collect(),
        mcp_servers: plugin
            .mcp_servers
            .iter()
            .map(|server| server.name.clone())
            .collect(),
        runtime_driver: plugin
            .manifest
            .runtime
            .as_ref()
            .map(|runtime| runtime.driver.clone()),
    }
}

fn collect_plugin_activation(
    plan: &mut PluginActivationPlan,
    plugin: &DiscoveredPlugin,
    resolver: &PluginResolverConfig,
    granted_permissions: PluginResolvedPermissions,
) {
    plan.instructions.extend(
        plugin
            .manifest
            .instructions
            .iter()
            .map(|instruction| instruction.text.clone()),
    );
    plan.skill_roots.extend(plugin.skill_roots.clone());
    if !plugin.tool_roots.is_empty() {
        plan.custom_tool_activations
            .push(PluginCustomToolActivation {
                plugin_id: plugin.manifest.id.clone(),
                root_dir: plugin.root_dir.clone(),
                manifest_path: plugin.manifest_path.clone(),
                tool_roots: plugin.tool_roots.clone(),
                granted_permissions: granted_permissions.clone(),
            });
    }
    plan.hooks.extend(
        plugin
            .hooks
            .iter()
            .cloned()
            .map(|hook| decorate_hook_registration(plugin, hook, &granted_permissions)),
    );
    plan.mcp_servers.extend(plugin.mcp_servers.clone());
    if let Some(runtime) = &plugin.manifest.runtime {
        let mut config = plugin.manifest.defaults.clone();
        let entry_config = resolver
            .entries
            .get(&plugin.manifest.id)
            .map(|entry| entry.config.clone())
            .unwrap_or_default();
        merge_toml_table(&mut config, entry_config);
        let mut runtime = runtime.clone();
        if let Some(module) = runtime.module.as_deref() {
            runtime.module = Some(plugin.root_dir.join(module).to_string_lossy().to_string());
        }
        plan.runtime_activations.push(PluginExecutableActivation {
            plugin_id: plugin.manifest.id.clone(),
            root_dir: plugin.root_dir.clone(),
            runtime,
            config,
            capabilities: plugin.manifest.capabilities.clone(),
            granted_permissions,
        });
    }
}

fn decorate_hook_registration(
    plugin: &DiscoveredPlugin,
    mut hook: HookRegistration,
    granted_permissions: &PluginResolvedPermissions,
) -> HookRegistration {
    hook.execution = Some(build_hook_execution_policy(
        &plugin.manifest.id,
        &plugin.root_dir,
        &plugin.manifest.capabilities,
        granted_permissions,
    ));
    hook
}

pub fn build_hook_execution_policy(
    plugin_id: &str,
    plugin_root: &Path,
    capabilities: &PluginCapabilitySet,
    granted_permissions: &PluginResolvedPermissions,
) -> HookExecutionPolicy {
    HookExecutionPolicy {
        plugin_id: Some(plugin_id.to_string()),
        plugin_root: Some(plugin_root.to_path_buf()),
        read_roots: granted_permissions.read_roots.clone(),
        write_roots: granted_permissions.write_roots.clone(),
        exec_roots: granted_permissions.exec_roots.clone(),
        network: granted_permissions.network.clone(),
        host_api_grants: granted_permissions.host_api.clone(),
        effects: derive_effect_policy(capabilities, granted_permissions),
    }
}

fn derive_effect_policy(
    capabilities: &PluginCapabilitySet,
    granted_permissions: &PluginResolvedPermissions,
) -> HookEffectPolicy {
    let declared_mutations = !capabilities.message_mutations.is_empty();
    let can_gate = capabilities.tool_policies.iter().any(|policy| {
        matches!(
            policy,
            PluginToolPolicyCapability::Deny | PluginToolPolicyCapability::Gate
        )
    });
    let can_permission = capabilities.tool_policies.iter().any(|policy| {
        matches!(
            policy,
            PluginToolPolicyCapability::Deny | PluginToolPolicyCapability::PermissionDecision
        )
    });

    HookEffectPolicy {
        message_mutation: if declared_mutations {
            granted_permissions.message_mutation
        } else {
            HookMutationPermission::Deny
        },
        allow_context_injection: true,
        allow_instruction_injection: true,
        allow_tool_arg_rewrite: capabilities
            .tool_policies
            .iter()
            .any(|policy| *policy == PluginToolPolicyCapability::RewriteArgs),
        allow_permission_decision: can_permission,
        allow_gate_decision: can_gate,
    }
}

fn resolve_plugin_permissions(
    plugin: &DiscoveredPlugin,
    entry: Option<&PluginEntryConfig>,
    workspace_root: &Path,
) -> Result<PluginResolvedPermissions, PluginDiagnostic> {
    let grant = entry
        .map(|entry| entry.permissions.clone())
        .unwrap_or_default();
    let requested = &plugin.manifest.permissions;

    ensure_supported_permission_modes(
        requested,
        &grant,
        &plugin.manifest.id,
        &plugin.manifest_path,
    )?;
    ensure_requested_contains_all(
        requested,
        &grant,
        &plugin.manifest.id,
        &plugin.manifest_path,
    )?;

    Ok(PluginResolvedPermissions {
        read_roots: resolve_workspace_paths(
            workspace_root,
            &grant.read,
            &plugin.manifest.id,
            &plugin.manifest_path,
        )?,
        write_roots: resolve_workspace_paths(
            workspace_root,
            &grant.write,
            &plugin.manifest.id,
            &plugin.manifest_path,
        )?,
        exec_roots: resolve_workspace_paths(
            workspace_root,
            &grant.exec,
            &plugin.manifest.id,
            &plugin.manifest_path,
        )?,
        network: to_hook_network_policy(&grant.network),
        message_mutation: grant.message_mutation,
        host_api: grant.host_api,
    })
}

fn ensure_supported_permission_modes(
    requested: &PluginPermissionRequest,
    granted: &PluginPermissionGrant,
    plugin_id: &str,
    manifest_path: &Path,
) -> Result<(), PluginDiagnostic> {
    if matches!(
        requested.message_mutation,
        HookMutationPermission::ReviewRequired
    ) {
        return Err(permission_diagnostic(
            "plugin_permission_review_required_unsupported",
            plugin_id,
            manifest_path,
            "message mutation mode `review_required` is not supported because host review is not implemented",
        ));
    }
    if matches!(
        granted.message_mutation,
        HookMutationPermission::ReviewRequired
    ) {
        return Err(permission_diagnostic(
            "plugin_permission_review_required_unsupported",
            plugin_id,
            manifest_path,
            "plugin permission grant uses `message_mutation = review_required`, but host review is not implemented",
        ));
    }
    Ok(())
}

fn ensure_requested_contains_all(
    requested: &PluginPermissionRequest,
    granted: &PluginPermissionGrant,
    plugin_id: &str,
    manifest_path: &Path,
) -> Result<(), PluginDiagnostic> {
    ensure_requested_paths(
        &requested.read,
        &granted.read,
        "read",
        plugin_id,
        manifest_path,
    )?;
    ensure_requested_paths(
        &requested.write,
        &granted.write,
        "write",
        plugin_id,
        manifest_path,
    )?;
    ensure_requested_paths(
        &requested.exec,
        &granted.exec,
        "exec",
        plugin_id,
        manifest_path,
    )?;

    if !network_allows(&requested.network, &granted.network) {
        return Err(permission_diagnostic(
            "plugin_permission_grant_exceeds_request",
            plugin_id,
            manifest_path,
            "network permission grant exceeds plugin request",
        ));
    }
    if !message_mutation_allows(requested.message_mutation, granted.message_mutation) {
        return Err(permission_diagnostic(
            "plugin_permission_grant_exceeds_request",
            plugin_id,
            manifest_path,
            "message mutation grant exceeds plugin request",
        ));
    }
    for api in &granted.host_api {
        if !requested.host_api.iter().any(|candidate| candidate == api) {
            return Err(permission_diagnostic(
                "plugin_permission_grant_exceeds_request",
                plugin_id,
                manifest_path,
                format!("host API grant `{api:?}` exceeds plugin request"),
            ));
        }
    }
    Ok(())
}

fn ensure_requested_paths(
    requested: &[String],
    granted: &[String],
    label: &str,
    plugin_id: &str,
    manifest_path: &Path,
) -> Result<(), PluginDiagnostic> {
    let requested = requested.iter().cloned().collect::<BTreeSet<_>>();
    for grant in granted {
        if !requested.contains(grant) {
            return Err(permission_diagnostic(
                "plugin_permission_grant_exceeds_request",
                plugin_id,
                manifest_path,
                format!("{label} permission grant `{grant}` exceeds plugin request"),
            ));
        }
    }
    Ok(())
}

fn permission_diagnostic(
    code: &'static str,
    plugin_id: &str,
    manifest_path: &Path,
    message: impl Into<String>,
) -> PluginDiagnostic {
    PluginDiagnostic::error(
        code,
        message.into(),
        Some(plugin_id.to_string()),
        Some(manifest_path.to_path_buf()),
    )
}

fn resolve_workspace_paths(
    workspace_root: &Path,
    values: &[String],
    plugin_id: &str,
    manifest_path: &Path,
) -> Result<Vec<PathBuf>, PluginDiagnostic> {
    values
        .iter()
        .map(|value| {
            resolve_workspace_relative_path(workspace_root, value).map_err(|message| {
                PluginDiagnostic::error(
                    "plugin_permission_invalid_path",
                    format!(
                        "plugin `{plugin_id}` has invalid permission path `{value}`: {message}"
                    ),
                    Some(plugin_id.to_string()),
                    Some(manifest_path.to_path_buf()),
                )
            })
        })
        .collect()
}

fn resolve_workspace_relative_path(workspace_root: &Path, value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err("parent/root/prefix path components are not allowed".to_string());
        }
    }
    Ok(workspace_root.join(path))
}

fn to_hook_network_policy(network: &PluginNetworkAccess) -> HookNetworkPolicy {
    match network {
        PluginNetworkAccess::Deny => HookNetworkPolicy::Deny,
        PluginNetworkAccess::Allow => HookNetworkPolicy::Allow,
        PluginNetworkAccess::AllowDomains(domains) => HookNetworkPolicy::AllowDomains {
            domains: domains.clone(),
        },
    }
}

fn network_allows(requested: &PluginNetworkAccess, granted: &PluginNetworkAccess) -> bool {
    match (requested, granted) {
        (_, PluginNetworkAccess::Deny) => true,
        (PluginNetworkAccess::Allow, PluginNetworkAccess::Allow) => true,
        (PluginNetworkAccess::Allow, PluginNetworkAccess::AllowDomains(_)) => true,
        (
            PluginNetworkAccess::AllowDomains(requested_domains),
            PluginNetworkAccess::AllowDomains(granted_domains),
        ) => granted_domains.iter().all(|domain| {
            requested_domains
                .iter()
                .any(|candidate| candidate == domain)
        }),
        _ => false,
    }
}

fn message_mutation_allows(
    requested: HookMutationPermission,
    granted: HookMutationPermission,
) -> bool {
    matches!(granted, HookMutationPermission::Deny)
        || matches!(
            (requested, granted),
            (HookMutationPermission::Allow, HookMutationPermission::Allow)
        )
}

fn merge_toml_table(base: &mut Map<String, toml::Value>, overlay: Map<String, toml::Value>) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) => {
                merge_toml_table(existing, incoming);
            }
            (_, replacement) => {
                base.insert(key, replacement);
            }
        }
    }
}

fn resolve_memory_slot(
    plan: &mut PluginActivationPlan,
    plugins_by_id: &BTreeMap<String, DiscoveredPlugin>,
    resolver: &PluginResolverConfig,
    allow_set: &BTreeSet<String>,
    deny_set: &BTreeSet<String>,
    workspace_root: &Path,
) {
    let Some(selected) = resolver.slots.memory.clone() else {
        return;
    };
    if selected == "none" {
        plan.slots.memory = None;
        return;
    }
    if !resolver.enabled {
        plan.diagnostics.push(PluginDiagnostic::error(
            "memory_slot_plugins_disabled",
            "memory slot is configured but plugins are disabled globally",
            Some(selected),
            None,
        ));
        return;
    }
    if deny_set.contains(&selected) {
        plan.diagnostics.push(PluginDiagnostic::error(
            "memory_slot_plugin_denied",
            format!("memory slot plugin `{selected}` is denied by plugins.deny"),
            Some(selected),
            None,
        ));
        return;
    }
    let Some(plugin) = plugins_by_id.get(&selected) else {
        plan.diagnostics.push(PluginDiagnostic::error(
            "memory_slot_missing",
            format!("memory slot references missing plugin `{selected}`"),
            Some(selected),
            None,
        ));
        return;
    };
    if plugin.manifest.kind != PluginKind::Memory {
        plan.diagnostics.push(PluginDiagnostic::error(
            "memory_slot_kind_mismatch",
            format!(
                "plugin `{}` is selected for memory slot but has kind `{:?}`",
                plugin.manifest.id, plugin.manifest.kind
            ),
            Some(plugin.manifest.id.clone()),
            Some(plugin.manifest_path.clone()),
        ));
        return;
    }
    if !allow_set.is_empty() && !allow_set.contains(&selected) {
        plan.diagnostics.push(PluginDiagnostic::error(
            "memory_slot_not_allowed",
            format!("memory slot plugin `{selected}` is not present in plugins.allow"),
            Some(selected),
            Some(plugin.manifest_path.clone()),
        ));
        return;
    }

    if !plan
        .plugin_states
        .iter()
        .any(|state| state.plugin_id == selected && state.enabled)
    {
        let granted_permissions = match resolve_plugin_permissions(
            plugin,
            resolver.entries.get(&selected),
            workspace_root,
        ) {
            Ok(granted_permissions) => granted_permissions,
            Err(diagnostic) => {
                plan.diagnostics.push(diagnostic);
                return;
            }
        };
        if let Some(state) = plan
            .plugin_states
            .iter_mut()
            .find(|state| state.plugin_id == selected)
        {
            state.enabled = true;
            state.reason = "enabled by plugins.slots.memory".to_string();
            state.granted_permissions = granted_permissions.clone();
            state.contributions = contribution_summary(plugin);
        }
        collect_plugin_activation(plan, plugin, resolver, granted_permissions);
    }
    plan.slots.memory = Some(plugin.manifest.id.clone());
}
