use crate::config::PluginResolverConfig;
use crate::discovery::{DiscoveredPlugin, PluginDiagnostic, PluginDiscovery};
use crate::manifest::PluginKind;
use mcp::McpServerConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use toml::map::Map;
use types::HookRegistration;

#[derive(Clone, Debug, PartialEq)]
pub struct DriverActivationRequest {
    pub plugin_id: String,
    pub driver_id: String,
    pub config: Map<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginState {
    pub plugin_id: String,
    pub enabled: bool,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PluginSlotSelection {
    pub memory: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PluginActivationPlan {
    pub instructions: Vec<String>,
    pub skill_roots: Vec<PathBuf>,
    pub hooks: Vec<HookRegistration>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub driver_activations: Vec<DriverActivationRequest>,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub plugin_states: Vec<PluginState>,
    pub slots: PluginSlotSelection,
}

pub fn build_activation_plan(
    discovery: PluginDiscovery,
    resolver: &PluginResolverConfig,
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
        let state = resolve_plugin_state(plugin, resolver, &allow_set, &deny_set);
        if state.enabled {
            collect_plugin_activation(&mut plan, plugin, resolver);
        }
        plan.plugin_states.push(state);
    }

    resolve_memory_slot(&mut plan, &plugins_by_id, resolver, &allow_set, &deny_set);

    // Deterministic order keeps startup behavior stable across runs.
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
        return PluginState {
            plugin_id,
            enabled: false,
            reason: "plugins disabled globally".to_string(),
        };
    }
    if deny_set.contains(&plugin.manifest.id) {
        return PluginState {
            plugin_id,
            enabled: false,
            reason: "denied by plugins.deny".to_string(),
        };
    }
    if !allow_set.is_empty() && !allow_set.contains(&plugin.manifest.id) {
        return PluginState {
            plugin_id,
            enabled: false,
            reason: "not present in plugins.allow".to_string(),
        };
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
    }
}

fn collect_plugin_activation(
    plan: &mut PluginActivationPlan,
    plugin: &DiscoveredPlugin,
    resolver: &PluginResolverConfig,
) {
    plan.instructions.extend(
        plugin
            .manifest
            .instructions
            .iter()
            .map(|instruction| instruction.text.clone()),
    );
    plan.skill_roots.extend(plugin.skill_roots.clone());
    plan.hooks.extend(plugin.hooks.clone());
    plan.mcp_servers.extend(plugin.mcp_servers.clone());
    if let Some(driver_id) = &plugin.manifest.driver {
        let mut config = plugin.manifest.defaults.clone();
        let entry_config = resolver
            .entries
            .get(&plugin.manifest.id)
            .map(|entry| entry.config.clone())
            .unwrap_or_default();
        merge_toml_table(&mut config, entry_config);
        plan.driver_activations.push(DriverActivationRequest {
            plugin_id: plugin.manifest.id.clone(),
            driver_id: driver_id.clone(),
            config,
        });
    }
}

fn merge_toml_table(base: &mut Map<String, toml::Value>, overlay: Map<String, toml::Value>) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) => {
                // Nested plugin config tables should compose without forcing
                // callers to restate every default sibling key.
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
        if let Some(state) = plan
            .plugin_states
            .iter_mut()
            .find(|state| state.plugin_id == selected)
        {
            state.enabled = true;
            state.reason = "enabled by plugins.slots.memory".to_string();
        }
        collect_plugin_activation(plan, plugin, resolver);
    }
    plan.slots.memory = Some(plugin.manifest.id.clone());
}
