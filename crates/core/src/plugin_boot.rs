//! Shared host-side plugin boot helpers.
//!
//! Hosts keep their own config-loading layers, but plugin discovery and builtin
//! driver activation should behave the same once that config has been resolved.

mod background_sync;
mod driver_env;
mod drivers;
mod registry;

use anyhow::{Context, Result};
use plugins::{
    PluginActivationPlan, PluginEntryConfig, PluginExecutableActivation, PluginResolverConfig,
    PluginSlotsConfig, build_activation_plan, discover_plugins,
};
pub use registry::{
    DriverActivationOutcome, DriverHostMessage, DriverHostMessageLevel, PluginDriverRegistry,
    UnknownDriverPolicy,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::RunStore;
use tools::ToolRegistry;

#[derive(Clone, Debug)]
pub struct PluginBootResolverConfig {
    pub enabled: bool,
    pub roots: Vec<PathBuf>,
    pub include_builtin: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub entries: BTreeMap<String, PluginEntryConfig>,
    pub slots: PluginSlotsConfig,
}

impl Default for PluginBootResolverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: Vec::new(),
            include_builtin: true,
            allow: Vec::new(),
            deny: Vec::new(),
            entries: BTreeMap::new(),
            slots: PluginSlotsConfig::default(),
        }
    }
}

pub fn build_plugin_activation_plan(
    workspace_root: &Path,
    config: &PluginBootResolverConfig,
) -> Result<PluginActivationPlan> {
    let mut roots = config.roots.clone();
    // Builtin plugin manifests live under the workspace so app hosts can ship a
    // stable first-party bundle without hardcoding each manifest path.
    if config.include_builtin {
        roots.push(workspace_root.join("builtin-plugins"));
    }
    roots.sort();
    roots.dedup();
    let discovery = discover_plugins(&roots)?;
    let resolver = PluginResolverConfig {
        enabled: config.enabled,
        allow: config.allow.clone(),
        deny: config.deny.clone(),
        entries: config.entries.clone(),
        slots: config.slots.clone(),
    };
    Ok(build_activation_plan(discovery, &resolver, workspace_root))
}

pub fn activate_driver_requests(
    requests: &[PluginExecutableActivation],
    workspace_root: &Path,
    run_store: Option<Arc<dyn RunStore>>,
    tools: &mut ToolRegistry,
    unknown_driver_policy: UnknownDriverPolicy,
) -> Result<DriverActivationOutcome> {
    let env_map = agent_env::EnvMap::from_workspace_dir(workspace_root)
        .context("failed to resolve environment for plugin driver activation")?;
    let registry = drivers::builtin_registry();
    registry.activate_all(
        requests,
        &mut registry::PluginDriverContext {
            workspace_root,
            env_map: &env_map,
            run_store,
            tools,
        },
        unknown_driver_policy,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PluginBootResolverConfig, UnknownDriverPolicy, activate_driver_requests,
        build_plugin_activation_plan,
    };
    use plugins::{
        PluginEntryConfig, PluginExecutableActivation, PluginRuntimeSpec, PluginSlotsConfig,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    use tools::ToolRegistry;

    #[test]
    fn build_plan_includes_builtin_roots_when_enabled() {
        let dir = tempdir().unwrap();
        let builtin = dir.path().join("builtin-plugins/demo/.nanoclaw-plugin");
        std::fs::create_dir_all(&builtin).unwrap();
        std::fs::write(
            builtin.join("plugin.toml"),
            r#"
id = "demo"
kind = "bundle"
enabled_by_default = true
"#,
        )
        .unwrap();
        let config = PluginBootResolverConfig {
            enabled: true,
            roots: Vec::new(),
            include_builtin: true,
            allow: Vec::new(),
            deny: Vec::new(),
            entries: BTreeMap::<String, PluginEntryConfig>::new(),
            slots: PluginSlotsConfig::default(),
        };

        let plan = build_plugin_activation_plan(dir.path(), &config).unwrap();
        assert!(
            plan.plugin_states
                .iter()
                .any(|state| state.plugin_id == "demo" && state.enabled)
        );
    }

    #[test]
    fn unknown_driver_can_warn_without_failing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let requests = vec![PluginExecutableActivation {
            plugin_id: "demo".to_string(),
            root_dir: dir.path().to_path_buf(),
            runtime: PluginRuntimeSpec {
                driver: "builtin.unknown".to_string(),
                module: None,
                abi: None,
            },
            config: toml::map::Map::new(),
            capabilities: plugins::PluginCapabilitySet::default(),
            granted_permissions: plugins::PluginResolvedPermissions::default(),
        }];

        let mut tools = ToolRegistry::new();
        let outcome = activate_driver_requests(
            &requests,
            dir.path(),
            None,
            &mut tools,
            UnknownDriverPolicy::Warn,
        )
        .unwrap();
        assert_eq!(outcome.warnings.len(), 1);
    }
}
