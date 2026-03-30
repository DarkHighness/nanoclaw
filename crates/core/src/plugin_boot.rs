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
use store::SessionStore;
use tools::ToolRegistry;
use types::PluginId;

#[derive(Clone, Debug)]
pub struct PluginBootResolverConfig {
    pub enabled: bool,
    pub roots: Vec<PathBuf>,
    pub include_builtin: bool,
    pub allow: Vec<PluginId>,
    pub deny: Vec<PluginId>,
    pub entries: BTreeMap<PluginId, PluginEntryConfig>,
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
    session_store: Option<Arc<dyn SessionStore>>,
    memory_reasoning_service: Option<inference::LlmServiceConfig>,
    tools: &mut ToolRegistry,
    unknown_driver_policy: UnknownDriverPolicy,
) -> Result<DriverActivationOutcome> {
    let env_map = agent_env::EnvMap::from_workspace_dir(workspace_root)
        .context("failed to resolve environment for plugin driver activation")?;
    let registry = drivers::builtin_registry();
    let memory_reasoning_service = memory_reasoning_service.as_ref();
    registry.activate_all(
        requests,
        &mut registry::PluginDriverContext {
            workspace_root,
            env_map: &env_map,
            session_store,
            memory_reasoning_service,
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
        PluginEntryConfig, PluginExecutableActivation, PluginResolvedPermissions,
        PluginRuntimeSpec, PluginSlotsConfig,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    use tools::ToolRegistry;
    use types::{HookEvent, HookHandler, HookHandlerKind, PluginId};

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
            entries: BTreeMap::<PluginId, PluginEntryConfig>::new(),
            slots: PluginSlotsConfig::default(),
        };

        let plan = build_plugin_activation_plan(dir.path(), &config).unwrap();
        assert!(
            plan.plugin_states
                .iter()
                .any(|state| state.plugin_id == "demo".into() && state.enabled)
        );
    }

    #[test]
    fn unknown_driver_can_warn_without_failing() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let requests = vec![PluginExecutableActivation {
            plugin_id: "demo".into(),
            root_dir: dir.path().to_path_buf(),
            runtime: PluginRuntimeSpec {
                driver: "builtin.unknown".into(),
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
            None,
            &mut tools,
            UnknownDriverPolicy::Warn,
        )
        .unwrap();
        assert_eq!(outcome.warnings.len(), 1);
    }

    #[test]
    fn wasm_hook_validator_emits_diagnostic_for_module_inside_exec_root() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let exec_root = dir.path().join(".nanoclaw/plugins-cache/demo");
        std::fs::create_dir_all(&exec_root).unwrap();
        let module_path = exec_root.join("policy.wasm");
        let requests = vec![PluginExecutableActivation {
            plugin_id: "demo".into(),
            root_dir: dir.path().to_path_buf(),
            runtime: PluginRuntimeSpec {
                driver: "builtin.wasm-hook-validator".into(),
                module: Some(module_path.to_string_lossy().to_string()),
                abi: Some("nanoclaw.plugin.v1".to_string()),
            },
            config: toml::map::Map::new(),
            capabilities: plugins::PluginCapabilitySet::default(),
            granted_permissions: PluginResolvedPermissions {
                exec_roots: vec![exec_root.clone()],
                ..PluginResolvedPermissions::default()
            },
        }];

        let mut tools = ToolRegistry::new();
        let outcome = activate_driver_requests(
            &requests,
            dir.path(),
            None,
            None,
            &mut tools,
            UnknownDriverPolicy::Error,
        )
        .unwrap();

        assert_eq!(
            outcome.diagnostics,
            vec![format!(
                "plugin `demo` validated wasm hook module {}",
                module_path.display()
            )]
        );
    }

    #[test]
    fn wasm_hook_validator_rejects_module_outside_exec_root() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let requests = vec![PluginExecutableActivation {
            plugin_id: "demo".into(),
            root_dir: dir.path().to_path_buf(),
            runtime: PluginRuntimeSpec {
                driver: "builtin.wasm-hook-validator".into(),
                module: Some(
                    dir.path()
                        .join("wasm/policy.wasm")
                        .to_string_lossy()
                        .to_string(),
                ),
                abi: None,
            },
            config: toml::map::Map::new(),
            capabilities: plugins::PluginCapabilitySet::default(),
            granted_permissions: PluginResolvedPermissions {
                exec_roots: vec![dir.path().join(".nanoclaw/plugins-cache/demo")],
                ..PluginResolvedPermissions::default()
            },
        }];

        let mut tools = ToolRegistry::new();
        let error = match activate_driver_requests(
            &requests,
            dir.path(),
            None,
            None,
            &mut tools,
            UnknownDriverPolicy::Error,
        ) {
            Ok(_) => panic!("expected validator to reject module outside exec roots"),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("references wasm module")
                && error.to_string().contains("outside granted exec roots")
        );
    }

    #[test]
    fn wasm_hook_runtime_emits_runtime_contributions() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let exec_root = dir.path().join(".nanoclaw/plugins-cache/demo");
        std::fs::create_dir_all(&exec_root).unwrap();
        let module_path = exec_root.join("policy.wasm");
        let config = toml::from_str::<toml::Value>(
            r#"
instructions = ["use the driver instruction"]

[[hooks]]
name = "policy-start"
event = "SessionStart"
entrypoint = "on_session_start"
timeout_ms = 750

[[mcp_servers]]
name = "driver-docs"

[mcp_servers.transport]
transport = "stdio"
command = "uvx"
args = ["driver-mcp"]
"#,
        )
        .unwrap()
        .as_table()
        .cloned()
        .unwrap();
        let requests = vec![PluginExecutableActivation {
            plugin_id: "demo".into(),
            root_dir: dir.path().to_path_buf(),
            runtime: PluginRuntimeSpec {
                driver: "builtin.wasm-hook-runtime".into(),
                module: Some(module_path.to_string_lossy().to_string()),
                abi: Some("nanoclaw.plugin.v1".to_string()),
            },
            config,
            capabilities: plugins::PluginCapabilitySet {
                hook_handlers: vec![HookHandlerKind::Wasm],
                mcp_exports: true,
                ..plugins::PluginCapabilitySet::default()
            },
            granted_permissions: PluginResolvedPermissions {
                exec_roots: vec![exec_root.clone()],
                ..PluginResolvedPermissions::default()
            },
        }];

        let mut tools = ToolRegistry::new();
        let outcome = activate_driver_requests(
            &requests,
            dir.path(),
            None,
            None,
            &mut tools,
            UnknownDriverPolicy::Error,
        )
        .unwrap();

        assert_eq!(outcome.hooks.len(), 1);
        assert_eq!(outcome.hooks[0].name, "policy-start".into());
        assert_eq!(outcome.hooks[0].event, HookEvent::SessionStart);
        match &outcome.hooks[0].handler {
            HookHandler::Wasm(handler) => {
                assert_eq!(handler.module, module_path.to_string_lossy());
                assert_eq!(handler.entrypoint, "on_session_start");
            }
            other => panic!("unexpected hook handler: {other:?}"),
        }
        assert_eq!(outcome.hooks[0].timeout_ms, Some(750));
        assert_eq!(
            outcome.hooks[0]
                .execution
                .as_ref()
                .and_then(|execution| execution.plugin_id.as_ref().map(|id| id.as_str())),
            Some("demo")
        );
        assert_eq!(
            outcome.instructions,
            vec!["use the driver instruction".to_string()]
        );
        assert_eq!(
            outcome
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["driver-docs"]
        );
        assert_eq!(
            outcome.diagnostics,
            vec![
                "plugin `demo` activated builtin.wasm-hook-runtime with 1 hooks, 1 MCP servers, and 1 instructions"
                    .to_string()
            ]
        );
    }
}
