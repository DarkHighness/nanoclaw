//! Shared host-side plugin boot helpers.
//!
//! Hosts keep their own config-loading layers, but plugin discovery and builtin
//! driver activation should behave the same once that config has been resolved.

use anyhow::{Context, Result, anyhow, bail};
use memory::{
    MemoryBackend, MemoryCoreBackend, MemoryCoreConfig, MemoryEmbedBackend, MemoryEmbedConfig,
    MemoryGetTool, MemorySearchTool,
};
use plugins::{
    DriverActivationRequest, PluginActivationPlan, PluginEntryConfig, PluginResolverConfig,
    PluginSlotsConfig, build_activation_plan, discover_plugins,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownDriverPolicy {
    Error,
    Warn,
}

#[derive(Default)]
pub struct DriverActivationOutcome {
    pub warnings: Vec<String>,
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
    Ok(build_activation_plan(discovery, &resolver))
}

pub fn activate_driver_requests(
    requests: &[DriverActivationRequest],
    workspace_root: &Path,
    run_store: Option<Arc<dyn RunStore>>,
    tools: &mut ToolRegistry,
    unknown_driver_policy: UnknownDriverPolicy,
) -> Result<DriverActivationOutcome> {
    let mut outcome = DriverActivationOutcome::default();
    let env_map = agent_env::EnvMap::from_workspace_dir(workspace_root)
        .context("failed to resolve environment for plugin driver activation")?;

    for request in requests {
        match request.driver_id.as_str() {
            "builtin.memory-core" => {
                let config: MemoryCoreConfig = toml::Value::Table(request.config.clone())
                    .try_into()
                    .with_context(|| {
                        format!("failed to parse config for plugin `{}`", request.plugin_id)
                    })?;
                if config.corpus.runtime_export.enabled && run_store.is_none() {
                    outcome.warnings.push(format!(
                        "plugin `{}` enabled runtime memory export without a run store; only existing sidecars will be indexed",
                        request.plugin_id
                    ));
                }
                let backend = memory_core_backend(workspace_root, &config, run_store.clone());
                tools.register_arc(Arc::new(MemorySearchTool::new(backend.clone())));
                tools.register_arc(Arc::new(MemoryGetTool::new(backend.clone())));
                maybe_spawn_memory_background_sync(
                    backend,
                    &request.plugin_id,
                    config.background_sync.enabled,
                    config.background_sync.run_on_start,
                    config.background_sync.interval_ms,
                    &mut outcome.warnings,
                );
            }
            "builtin.memory-embed" => {
                let mut table = request.config.clone();
                // Keep plugin manifests declarative by allowing env indirection for secrets in
                // any nested service config (`embedding`, `query_expansion`, `rerank`, etc.).
                materialize_api_key_envs(&mut table, &env_map, &request.plugin_id)?;
                let config: MemoryEmbedConfig =
                    toml::Value::Table(table).try_into().with_context(|| {
                        format!("failed to parse config for plugin `{}`", request.plugin_id)
                    })?;
                if config.corpus.runtime_export.enabled && run_store.is_none() {
                    outcome.warnings.push(format!(
                        "plugin `{}` enabled runtime memory export without a run store; only existing sidecars will be indexed",
                        request.plugin_id
                    ));
                }
                let backend =
                    memory_embed_backend(workspace_root, config.clone(), run_store.clone())
                        .with_context(|| {
                            format!(
                                "failed to initialize memory-embed backend for plugin `{}`",
                                request.plugin_id
                            )
                        })?;
                tools.register_arc(Arc::new(MemorySearchTool::new(backend.clone())));
                tools.register_arc(Arc::new(MemoryGetTool::new(backend.clone())));
                maybe_spawn_memory_background_sync(
                    backend,
                    &request.plugin_id,
                    config.background_sync.enabled,
                    config.background_sync.run_on_start,
                    config.background_sync.interval_ms,
                    &mut outcome.warnings,
                );
            }
            other => match unknown_driver_policy {
                UnknownDriverPolicy::Error => bail!(
                    "plugin `{}` references unknown driver `{other}`",
                    request.plugin_id
                ),
                UnknownDriverPolicy::Warn => outcome.warnings.push(format!(
                    "plugin `{}` references unknown driver `{other}`",
                    request.plugin_id
                )),
            },
        }
    }

    Ok(outcome)
}

fn memory_core_backend(
    workspace_root: &Path,
    config: &MemoryCoreConfig,
    run_store: Option<Arc<dyn RunStore>>,
) -> Arc<dyn MemoryBackend> {
    let backend = MemoryCoreBackend::new(workspace_root.to_path_buf(), config.clone());
    if let Some(run_store) = run_store {
        Arc::new(backend.with_run_store(run_store))
    } else {
        Arc::new(backend)
    }
}

fn memory_embed_backend(
    workspace_root: &Path,
    config: MemoryEmbedConfig,
    run_store: Option<Arc<dyn RunStore>>,
) -> Result<Arc<dyn MemoryBackend>> {
    let backend = MemoryEmbedBackend::from_http_config(workspace_root.to_path_buf(), config)?;
    Ok(if let Some(run_store) = run_store {
        Arc::new(backend.with_run_store(run_store))
    } else {
        Arc::new(backend)
    })
}

fn maybe_spawn_memory_background_sync(
    backend: Arc<dyn MemoryBackend>,
    plugin_id: &str,
    enabled: bool,
    run_on_start: bool,
    interval_ms: u64,
    warnings: &mut Vec<String>,
) {
    if !enabled {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        warnings.push(format!(
            "plugin `{plugin_id}` requested background sync but no tokio runtime was active during boot"
        ));
        return;
    };
    let plugin_id = plugin_id.to_string();
    handle.spawn(async move {
        if run_on_start {
            if let Err(error) = backend.sync().await {
                tracing::warn!(plugin_id, error = %error, "memory background sync failed during startup");
            }
        }

        let mut interval =
            tokio::time::interval(Duration::from_millis(interval_ms.max(1_000)));
        // Tokio intervals tick immediately on first poll. Consume that eager
        // tick so `run_on_start = false` really means “wait one full interval”.
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = backend.sync().await {
                tracing::warn!(plugin_id, error = %error, "memory background sync failed");
            }
        }
    });
}

fn materialize_api_key_envs(
    table: &mut toml::map::Map<String, toml::Value>,
    env_map: &agent_env::EnvMap,
    plugin_id: &str,
) -> Result<()> {
    if let Some(api_key_env) = table
        .remove("api_key_env")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
    {
        let api_key = env_map.get_non_empty(&api_key_env).ok_or_else(|| {
            anyhow!("missing API key env `{api_key_env}` for plugin `{plugin_id}` service config")
        })?;
        table.insert("api_key".to_string(), toml::Value::String(api_key));
    }

    for (_, value) in table.iter_mut() {
        if let toml::Value::Table(child) = value {
            materialize_api_key_envs(child, env_map, plugin_id)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        PluginBootResolverConfig, UnknownDriverPolicy, activate_driver_requests,
        build_plugin_activation_plan,
    };
    use plugins::{DriverActivationRequest, PluginEntryConfig, PluginSlotsConfig};
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
        let requests = vec![DriverActivationRequest {
            plugin_id: "demo".to_string(),
            driver_id: "builtin.unknown".to_string(),
            config: toml::map::Map::new(),
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
