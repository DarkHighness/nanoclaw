use super::background_sync::maybe_spawn_memory_background_sync;
use super::driver_env::materialize_api_key_envs;
use anyhow::{Context, Result};
use memory::{
    MemoryBackend, MemoryCoreBackend, MemoryCoreConfig, MemoryEmbedBackend, MemoryEmbedConfig,
    MemoryGetTool, MemorySearchTool,
};
use plugins::DriverActivationRequest;
use std::path::Path;
use std::sync::Arc;
use store::RunStore;
use tools::ToolRegistry;

pub(super) fn activate_memory_core_request(
    request: &DriverActivationRequest,
    workspace_root: &Path,
    run_store: Option<Arc<dyn RunStore>>,
    tools: &mut ToolRegistry,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let config: MemoryCoreConfig = toml::Value::Table(request.config.clone())
        .try_into()
        .with_context(|| format!("failed to parse config for plugin `{}`", request.plugin_id))?;
    if config.corpus.runtime_export.enabled && run_store.is_none() {
        warnings.push(format!(
            "plugin `{}` enabled runtime memory export without a run store; only existing sidecars will be indexed",
            request.plugin_id
        ));
    }
    let backend = memory_core_backend(workspace_root, &config, run_store);
    tools.register_arc(Arc::new(MemorySearchTool::new(backend.clone())));
    tools.register_arc(Arc::new(MemoryGetTool::new(backend.clone())));
    maybe_spawn_memory_background_sync(
        backend,
        &request.plugin_id,
        config.background_sync.enabled,
        config.background_sync.run_on_start,
        config.background_sync.interval_ms,
        warnings,
    );
    Ok(())
}

pub(super) fn activate_memory_embed_request(
    request: &DriverActivationRequest,
    workspace_root: &Path,
    env_map: &agent_env::EnvMap,
    run_store: Option<Arc<dyn RunStore>>,
    tools: &mut ToolRegistry,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mut table = request.config.clone();
    // Keep plugin manifests declarative by allowing env indirection for secrets in
    // any nested service config (`embedding`, `query_expansion`, `rerank`, etc.).
    materialize_api_key_envs(&mut table, env_map, &request.plugin_id)?;
    let config: MemoryEmbedConfig = toml::Value::Table(table)
        .try_into()
        .with_context(|| format!("failed to parse config for plugin `{}`", request.plugin_id))?;
    if config.corpus.runtime_export.enabled && run_store.is_none() {
        warnings.push(format!(
            "plugin `{}` enabled runtime memory export without a run store; only existing sidecars will be indexed",
            request.plugin_id
        ));
    }
    let backend =
        memory_embed_backend(workspace_root, config.clone(), run_store).with_context(|| {
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
        warnings,
    );
    Ok(())
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
