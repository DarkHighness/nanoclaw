use super::background_sync::maybe_spawn_memory_background_sync;
use super::driver_env::materialize_api_key_envs;
use super::registry::{
    DriverActivationOutcome, PluginDriverContext, PluginDriverFactory, PluginDriverRegistry,
};
use anyhow::{Context, Result};
use memory::{
    MemoryBackend, MemoryCoreBackend, MemoryCoreConfig, MemoryEmbedBackend, MemoryEmbedConfig,
    MemoryGetTool, MemorySearchTool,
};
use plugins::PluginExecutableActivation;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) fn builtin_registry() -> PluginDriverRegistry {
    let mut registry = PluginDriverRegistry::new();
    registry.register(Arc::new(MemoryCoreDriverFactory));
    registry.register(Arc::new(MemoryEmbedDriverFactory));
    registry.register(Arc::new(WasmHookRuntimeDriverFactory));
    registry
}

struct MemoryCoreDriverFactory;

impl PluginDriverFactory for MemoryCoreDriverFactory {
    fn driver_id(&self) -> &'static str {
        "builtin.memory-core"
    }

    fn activate(
        &self,
        activation: &PluginExecutableActivation,
        context: &mut PluginDriverContext<'_>,
        outcome: &mut DriverActivationOutcome,
    ) -> Result<()> {
        let config: MemoryCoreConfig = toml::Value::Table(activation.config.clone())
            .try_into()
            .with_context(|| {
                format!(
                    "failed to parse config for plugin `{}`",
                    activation.plugin_id
                )
            })?;
        if config.corpus.runtime_export.enabled && context.run_store.is_none() {
            outcome.warnings.push(format!(
                "plugin `{}` enabled runtime memory export without a run store; only existing sidecars will be indexed",
                activation.plugin_id
            ));
        }
        let backend =
            memory_core_backend(context.workspace_root, &config, context.run_store.clone());
        context
            .tools
            .register_arc(Arc::new(MemorySearchTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryGetTool::new(backend.clone())));
        maybe_spawn_memory_background_sync(
            backend,
            &activation.plugin_id,
            config.background_sync.enabled,
            config.background_sync.run_on_start,
            config.background_sync.interval_ms,
            &mut outcome.warnings,
        );
        Ok(())
    }
}

struct MemoryEmbedDriverFactory;

impl PluginDriverFactory for MemoryEmbedDriverFactory {
    fn driver_id(&self) -> &'static str {
        "builtin.memory-embed"
    }

    fn activate(
        &self,
        activation: &PluginExecutableActivation,
        context: &mut PluginDriverContext<'_>,
        outcome: &mut DriverActivationOutcome,
    ) -> Result<()> {
        let mut table = activation.config.clone();
        materialize_api_key_envs(&mut table, context.env_map, &activation.plugin_id)?;
        let config: MemoryEmbedConfig =
            toml::Value::Table(table).try_into().with_context(|| {
                format!(
                    "failed to parse config for plugin `{}`",
                    activation.plugin_id
                )
            })?;
        if config.corpus.runtime_export.enabled && context.run_store.is_none() {
            outcome.warnings.push(format!(
                "plugin `{}` enabled runtime memory export without a run store; only existing sidecars will be indexed",
                activation.plugin_id
            ));
        }
        let backend = memory_embed_backend(
            context.workspace_root,
            config.clone(),
            context.run_store.clone(),
        )
        .with_context(|| {
            format!(
                "failed to initialize memory-embed backend for plugin `{}`",
                activation.plugin_id
            )
        })?;
        context
            .tools
            .register_arc(Arc::new(MemorySearchTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryGetTool::new(backend.clone())));
        maybe_spawn_memory_background_sync(
            backend,
            &activation.plugin_id,
            config.background_sync.enabled,
            config.background_sync.run_on_start,
            config.background_sync.interval_ms,
            &mut outcome.warnings,
        );
        Ok(())
    }
}

struct WasmHookRuntimeDriverFactory;

impl PluginDriverFactory for WasmHookRuntimeDriverFactory {
    fn driver_id(&self) -> &'static str {
        "builtin.wasm-hook-runtime"
    }

    fn activate(
        &self,
        activation: &PluginExecutableActivation,
        _context: &mut PluginDriverContext<'_>,
        outcome: &mut DriverActivationOutcome,
    ) -> Result<()> {
        let Some(module) = activation.runtime.module.as_deref() else {
            anyhow::bail!(
                "plugin `{}` uses builtin.wasm-hook-runtime without runtime.module",
                activation.plugin_id
            );
        };
        let module_path = PathBuf::from(module);
        if !activation
            .granted_permissions
            .exec_roots
            .iter()
            .any(|root| module_path.starts_with(root))
        {
            anyhow::bail!(
                "plugin `{}` references wasm module {} outside granted exec roots",
                activation.plugin_id,
                module_path.display()
            );
        }
        outcome.diagnostics.push(format!(
            "plugin `{}` prepared wasm runtime module {}",
            activation.plugin_id,
            module_path.display()
        ));
        Ok(())
    }
}

fn memory_core_backend(
    workspace_root: &Path,
    config: &MemoryCoreConfig,
    run_store: Option<Arc<dyn store::RunStore>>,
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
    run_store: Option<Arc<dyn store::RunStore>>,
) -> Result<Arc<dyn MemoryBackend>> {
    let backend = MemoryEmbedBackend::from_http_config(workspace_root.to_path_buf(), config)?;
    Ok(if let Some(run_store) = run_store {
        Arc::new(backend.with_run_store(run_store))
    } else {
        Arc::new(backend)
    })
}
