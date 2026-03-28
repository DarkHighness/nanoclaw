use crate::config::AgentCoreConfig;
use agent::AgentWorkspaceLayout;
use agent::mcp::{McpServerConfig, McpTransportConfig};
use agent::plugins::{PluginActivationPlan, PluginEntryConfig, PluginSlotsConfig};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn resolved_skill_roots(
    config: &AgentCoreConfig,
    workspace_root: &Path,
    plugin_plan: &PluginActivationPlan,
) -> Vec<PathBuf> {
    let mut roots = config.resolved_skill_roots(workspace_root);
    roots.extend(plugin_plan.skill_roots.clone());
    if roots.is_empty() {
        let default_root = AgentWorkspaceLayout::new(workspace_root).skills_dir();
        if default_root.exists() {
            roots.push(default_root);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

pub(super) fn build_plugin_activation_plan(
    config: &AgentCoreConfig,
    workspace_root: &Path,
) -> Result<PluginActivationPlan> {
    let plugins = config.plugins();
    let resolver = agent::PluginBootResolverConfig {
        enabled: plugins.enabled,
        roots: config.resolved_plugin_roots(workspace_root),
        include_builtin: plugins.include_builtin,
        allow: plugins.allow.clone(),
        deny: plugins.deny.clone(),
        entries: plugins
            .entries
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    PluginEntryConfig {
                        enabled: entry.enabled,
                        permissions: entry.permissions.clone(),
                        config: entry.config.clone().into_iter().collect(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
        slots: PluginSlotsConfig {
            memory: plugins.slots.memory.clone(),
        },
    };
    agent::build_plugin_activation_plan(workspace_root, &resolver)
}

pub(super) fn resolve_mcp_servers(
    configs: &[McpServerConfig],
    workspace_root: &Path,
) -> Vec<McpServerConfig> {
    configs
        .iter()
        .cloned()
        .map(|mut server| {
            if let McpTransportConfig::Stdio { cwd, .. } = &mut server.transport
                && let Some(current_dir) = cwd.as_deref()
            {
                let resolved = resolve_path(workspace_root, current_dir);
                *cwd = Some(resolved.to_string_lossy().to_string());
            }
            server
        })
        .collect()
}

pub(super) fn dedup_mcp_servers(servers: Vec<McpServerConfig>) -> Vec<McpServerConfig> {
    let mut by_name = BTreeMap::new();
    for server in servers {
        by_name.entry(server.name.clone()).or_insert(server);
    }
    by_name.into_values().collect()
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}
