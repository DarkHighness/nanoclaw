use super::background_sync::maybe_spawn_memory_background_sync;
use super::registry::{
    DriverActivationOutcome, PluginDriverContext, PluginDriverFactory, PluginDriverRegistry,
};
use anyhow::{Context, Result};
use mcp::McpServerConfig;
use memory::{
    MemoryBackend, MemoryCoreBackend, MemoryCoreConfig, MemoryForgetTool, MemoryGetTool,
    MemoryListTool, MemoryPromoteTool, MemoryRecordTool, MemorySearchTool,
};
use plugins::{PluginExecutableActivation, build_hook_execution_policy};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use types::{
    HookEvent, HookHandler, HookHandlerKind, HookMatcher, HookRegistration, WasmHookHandler,
};

const WASM_HOOK_RUNTIME_DRIVER_ID: &str = "builtin.wasm-hook-runtime";
const WASM_HOOK_VALIDATOR_DRIVER_ID: &str = "builtin.wasm-hook-validator";

pub(super) fn builtin_registry() -> PluginDriverRegistry {
    let mut registry = PluginDriverRegistry::new();
    registry.register(Arc::new(MemoryCoreDriverFactory));
    registry.register(Arc::new(WasmHookRuntimeDriverFactory));
    registry.register(Arc::new(WasmHookValidatorDriverFactory));
    registry
}

#[derive(Clone, Debug, Default, Deserialize)]
struct WasmHookRuntimeDriverConfig {
    #[serde(default)]
    hooks: Vec<WasmHookRuntimeSpec>,
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    instructions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct WasmHookRuntimeSpec {
    name: String,
    event: HookEvent,
    #[serde(default)]
    matcher: Option<HookMatcher>,
    entrypoint: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
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
        if config.corpus.runtime_export.enabled && context.session_store.is_none() {
            outcome.warnings.push(format!(
                "plugin `{}` enabled runtime memory export without a session store; only existing sidecars will be indexed",
                activation.plugin_id
            ));
        }
        let backend = memory_core_backend(
            context.workspace_root,
            &config,
            context.session_store.clone(),
        );
        outcome.remember_primary_memory_backend(backend.clone());
        context
            .tools
            .register_arc(Arc::new(MemorySearchTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryGetTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryListTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryRecordTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryPromoteTool::new(backend.clone())));
        context
            .tools
            .register_arc(Arc::new(MemoryForgetTool::new(backend.clone())));
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

struct WasmHookValidatorDriverFactory;
struct WasmHookRuntimeDriverFactory;

impl PluginDriverFactory for WasmHookRuntimeDriverFactory {
    fn driver_id(&self) -> &'static str {
        WASM_HOOK_RUNTIME_DRIVER_ID
    }

    fn activate(
        &self,
        activation: &PluginExecutableActivation,
        _context: &mut PluginDriverContext<'_>,
        outcome: &mut DriverActivationOutcome,
    ) -> Result<()> {
        let module_path = validated_wasm_module_path(activation)?;
        let config: WasmHookRuntimeDriverConfig = toml::Value::Table(activation.config.clone())
            .try_into()
            .with_context(|| {
                format!(
                    "failed to parse config for plugin `{}`",
                    activation.plugin_id
                )
            })?;
        if !config.hooks.is_empty()
            && !activation
                .capabilities
                .hook_handlers
                .contains(&HookHandlerKind::Wasm)
        {
            anyhow::bail!(
                "plugin `{}` uses builtin.wasm-hook-runtime without declaring wasm hook handler capability",
                activation.plugin_id
            );
        }
        if !config.mcp_servers.is_empty() && !activation.capabilities.mcp_exports {
            anyhow::bail!(
                "plugin `{}` uses builtin.wasm-hook-runtime to export MCP servers without mcp_exports capability",
                activation.plugin_id
            );
        }
        let execution = build_hook_execution_policy(
            &activation.plugin_id,
            &activation.root_dir,
            &activation.capabilities,
            &activation.granted_permissions,
        );
        let module = module_path.to_string_lossy().to_string();
        let hook_count = config.hooks.len();
        let mcp_count = config.mcp_servers.len();
        let instructions = config
            .instructions
            .into_iter()
            .map(|instruction| instruction.trim().to_string())
            .filter(|instruction| !instruction.is_empty())
            .collect::<Vec<_>>();
        let instruction_count = instructions.len();
        outcome
            .hooks
            .extend(config.hooks.into_iter().map(|hook| HookRegistration {
                name: hook.name.into(),
                event: hook.event,
                matcher: hook.matcher,
                handler: HookHandler::Wasm(WasmHookHandler {
                    module: module.clone(),
                    entrypoint: hook.entrypoint,
                }),
                timeout_ms: hook.timeout_ms,
                execution: Some(execution.clone()),
            }));
        outcome.mcp_servers.extend(config.mcp_servers);
        outcome.instructions.extend(instructions);
        outcome.diagnostics.push(format!(
            "plugin `{}` activated builtin.wasm-hook-runtime with {} hooks, {} MCP servers, and {} instructions",
            activation.plugin_id,
            hook_count,
            mcp_count,
            instruction_count,
        ));
        Ok(())
    }
}

impl PluginDriverFactory for WasmHookValidatorDriverFactory {
    fn driver_id(&self) -> &'static str {
        WASM_HOOK_VALIDATOR_DRIVER_ID
    }

    fn activate(
        &self,
        activation: &PluginExecutableActivation,
        _context: &mut PluginDriverContext<'_>,
        outcome: &mut DriverActivationOutcome,
    ) -> Result<()> {
        // This builtin participates in the same activation pipeline as real
        // runtime drivers, but it only validates wasm-module wiring and does
        // not contribute hooks, MCP servers, tools, or instructions on its own.
        let module_path = validated_wasm_module_path(activation)?;
        outcome.diagnostics.push(format!(
            "plugin `{}` validated wasm hook module {}",
            activation.plugin_id,
            module_path.display()
        ));
        Ok(())
    }
}

fn memory_core_backend(
    workspace_root: &Path,
    config: &MemoryCoreConfig,
    session_store: Option<Arc<dyn store::SessionStore>>,
) -> Arc<dyn MemoryBackend> {
    let backend = MemoryCoreBackend::new(workspace_root.to_path_buf(), config.clone());
    if let Some(session_store) = session_store {
        Arc::new(backend.with_session_store(session_store))
    } else {
        Arc::new(backend)
    }
}

fn validated_wasm_module_path(activation: &PluginExecutableActivation) -> Result<PathBuf> {
    let Some(module) = activation.runtime.module.as_deref() else {
        anyhow::bail!(
            "plugin `{}` uses {} without runtime.module",
            activation.plugin_id,
            activation.runtime.driver
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
    Ok(module_path)
}
