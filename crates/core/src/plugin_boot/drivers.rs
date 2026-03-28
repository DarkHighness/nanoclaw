use super::background_sync::maybe_spawn_memory_background_sync;
use super::driver_env::materialize_api_key_envs;
use super::registry::{
    DriverActivationOutcome, PluginDriverContext, PluginDriverFactory, PluginDriverRegistry,
};
use anyhow::{Context, Result};
use inference::{EmbeddingConfig, LlmServiceConfig, QueryExpansionConfig, RerankConfig};
use mcp::McpServerConfig;
use memory::{
    HybridWeights, MemoryBackend, MemoryBackgroundSyncConfig, MemoryCoreBackend, MemoryCoreConfig,
    MemoryCorpusConfig, MemoryEmbedBackend, MemoryEmbedConfig, MemoryForgetTool, MemoryGetTool,
    MemoryListTool, MemoryPromoteTool, MemoryRecordTool, MemorySearchConfig, MemorySearchTool,
    MemoryVectorStoreConfig,
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
#[cfg(test)]
const DEFAULT_MEMORY_REASONING_TIMEOUT_MS: u64 = 30_000;

pub(super) fn builtin_registry() -> PluginDriverRegistry {
    let mut registry = PluginDriverRegistry::new();
    registry.register(Arc::new(MemoryCoreDriverFactory));
    registry.register(Arc::new(MemoryEmbedDriverFactory));
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

#[derive(Clone, Debug, PartialEq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct MemoryEmbedDriverConfig {
    corpus: MemoryCorpusConfig,
    chunking: memory::MemoryChunkingConfig,
    search: MemorySearchConfig,
    background_sync: MemoryBackgroundSyncConfig,
    embedding: Option<EmbeddingConfig>,
    query_expansion: Option<MemoryQueryExpansionDriverConfig>,
    rerank: Option<MemoryRerankDriverConfig>,
    hybrid: HybridWeights,
    vector_store: MemoryVectorStoreConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryQueryExpansionDriverConfig {
    #[serde(flatten)]
    service: Option<LlmServiceConfig>,
    #[serde(default = "default_query_expansion_variants")]
    variants: usize,
    #[serde(default)]
    use_internal_profile: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryRerankDriverConfig {
    #[serde(flatten)]
    service: Option<LlmServiceConfig>,
    #[serde(default)]
    use_internal_profile: bool,
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
        // Keep plugin manifests declarative by allowing env indirection for
        // secrets in nested embedding/query/rerank service configs.
        materialize_api_key_envs(&mut table, context.env_map, &activation.plugin_id)?;
        let config = resolve_memory_embed_driver_config(table, activation, context, outcome)?;
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
                name: hook.name,
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

fn resolve_memory_embed_driver_config(
    table: toml::map::Map<String, toml::Value>,
    activation: &PluginExecutableActivation,
    context: &PluginDriverContext<'_>,
    outcome: &mut DriverActivationOutcome,
) -> Result<MemoryEmbedConfig> {
    let config: MemoryEmbedDriverConfig =
        toml::Value::Table(table).try_into().with_context(|| {
            format!(
                "failed to parse config for plugin `{}`",
                activation.plugin_id
            )
        })?;
    let (query_expansion, query_from_internal) =
        resolve_query_expansion_config(config.query_expansion, activation, context)?;
    let (rerank, rerank_from_internal) = resolve_rerank_config(config.rerank, activation, context)?;
    if query_from_internal || rerank_from_internal {
        let mut sourced = Vec::new();
        if query_from_internal {
            sourced.push("query expansion");
        }
        if rerank_from_internal {
            sourced.push("rerank");
        }
        outcome.diagnostics.push(format!(
            "plugin `{}` sourced {} service config from internal.memory",
            activation.plugin_id,
            sourced.join(" and "),
        ));
    }
    Ok(MemoryEmbedConfig {
        corpus: config.corpus,
        chunking: config.chunking,
        search: config.search,
        background_sync: config.background_sync,
        embedding: config.embedding,
        query_expansion,
        rerank,
        hybrid: config.hybrid,
        vector_store: config.vector_store,
    })
}

fn resolve_query_expansion_config(
    config: Option<MemoryQueryExpansionDriverConfig>,
    activation: &PluginExecutableActivation,
    context: &PluginDriverContext<'_>,
) -> Result<(Option<QueryExpansionConfig>, bool)> {
    let Some(config) = config else {
        return Ok((None, false));
    };
    if config.use_internal_profile && config.service.is_some() {
        anyhow::bail!(
            "plugin `{}` configured query_expansion with both explicit service fields and use_internal_profile = true",
            activation.plugin_id
        );
    }
    if config.use_internal_profile {
        let service =
            context
                .memory_reasoning_service
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "plugin `{}` configured query_expansion.use_internal_profile = true but no internal.memory service was provided by the host",
                        activation.plugin_id
                    )
                })?;
        return Ok((
            Some(QueryExpansionConfig {
                service,
                variants: config.variants,
            }),
            true,
        ));
    }
    let Some(service) = config.service else {
        anyhow::bail!(
            "plugin `{}` configured query_expansion without service fields; provide provider/model or set use_internal_profile = true",
            activation.plugin_id
        );
    };
    Ok((
        Some(QueryExpansionConfig {
            service,
            variants: config.variants,
        }),
        false,
    ))
}

fn resolve_rerank_config(
    config: Option<MemoryRerankDriverConfig>,
    activation: &PluginExecutableActivation,
    context: &PluginDriverContext<'_>,
) -> Result<(Option<RerankConfig>, bool)> {
    let Some(config) = config else {
        return Ok((None, false));
    };
    if config.use_internal_profile && config.service.is_some() {
        anyhow::bail!(
            "plugin `{}` configured rerank with both explicit service fields and use_internal_profile = true",
            activation.plugin_id
        );
    }
    if config.use_internal_profile {
        let service =
            context
                .memory_reasoning_service
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "plugin `{}` configured rerank.use_internal_profile = true but no internal.memory service was provided by the host",
                        activation.plugin_id
                    )
                })?;
        return Ok((Some(RerankConfig { service }), true));
    }
    let Some(service) = config.service else {
        anyhow::bail!(
            "plugin `{}` configured rerank without service fields; provide provider/model or set use_internal_profile = true",
            activation.plugin_id
        );
    };
    Ok((Some(RerankConfig { service }), false))
}

const fn default_query_expansion_variants() -> usize {
    1
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

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MEMORY_REASONING_TIMEOUT_MS, resolve_memory_embed_driver_config};
    use crate::plugin_boot::registry::{DriverActivationOutcome, PluginDriverContext};
    use agent_env::EnvMap;
    use inference::LlmServiceConfig;
    use plugins::{
        PluginCapabilitySet, PluginExecutableActivation, PluginResolvedPermissions,
        PluginRuntimeSpec,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    use tools::ToolRegistry;

    fn memory_embed_activation(config: &str) -> PluginExecutableActivation {
        PluginExecutableActivation {
            plugin_id: "memory-embed".to_string(),
            root_dir: std::env::temp_dir(),
            runtime: PluginRuntimeSpec {
                driver: "builtin.memory-embed".to_string(),
                module: None,
                abi: None,
            },
            config: toml::from_str::<toml::Value>(config)
                .unwrap()
                .as_table()
                .cloned()
                .unwrap(),
            capabilities: PluginCapabilitySet::default(),
            granted_permissions: PluginResolvedPermissions::default(),
        }
    }

    fn memory_reasoning_service(model: &str) -> LlmServiceConfig {
        LlmServiceConfig {
            provider: "openai".to_string(),
            model: model.to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            api_key: Some("memory-secret".to_string()),
            headers: BTreeMap::from([("x-trace".to_string(), "memory".to_string())]),
            timeout_ms: DEFAULT_MEMORY_REASONING_TIMEOUT_MS,
        }
    }

    #[test]
    fn memory_embed_driver_can_source_query_expansion_and_rerank_from_internal_memory() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let activation = memory_embed_activation(
            r#"
                [embedding]
                provider = "openai"
                model = "text-embedding-3-small"
                api_key = "embed-secret"

                [query_expansion]
                variants = 3
                use_internal_profile = true

                [rerank]
                use_internal_profile = true
            "#,
        );
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let mut tools = ToolRegistry::new();
        let mut outcome = DriverActivationOutcome::default();

        let config = resolve_memory_embed_driver_config(
            activation.config.clone(),
            &activation,
            &PluginDriverContext {
                workspace_root: dir.path(),
                env_map: &env_map,
                run_store: None,
                memory_reasoning_service: Some(&memory_reasoning_service("gpt-5.4-mini")),
                tools: &mut tools,
            },
            &mut outcome,
        )
        .unwrap();

        assert_eq!(
            config
                .query_expansion
                .as_ref()
                .map(|config| config.service.model.as_str()),
            Some("gpt-5.4-mini")
        );
        assert_eq!(
            config
                .query_expansion
                .as_ref()
                .map(|config| config.variants),
            Some(3)
        );
        assert_eq!(
            config
                .rerank
                .as_ref()
                .map(|config| config.service.model.as_str()),
            Some("gpt-5.4-mini")
        );
        assert_eq!(
            outcome.diagnostics,
            vec![
                "plugin `memory-embed` sourced query expansion and rerank service config from internal.memory"
                    .to_string()
            ]
        );
    }

    #[test]
    fn memory_embed_driver_preserves_explicit_query_expansion_and_rerank_services() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "").unwrap();
        let activation = memory_embed_activation(
            r#"
                [embedding]
                provider = "openai"
                model = "text-embedding-3-small"
                api_key = "embed-secret"

                [query_expansion]
                provider = "anthropic"
                model = "claude-query"
                api_key = "query-secret"
                variants = 2

                [rerank]
                provider = "anthropic"
                model = "claude-rerank"
                api_key = "rerank-secret"
            "#,
        );
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let mut tools = ToolRegistry::new();
        let mut outcome = DriverActivationOutcome::default();

        let config = resolve_memory_embed_driver_config(
            activation.config.clone(),
            &activation,
            &PluginDriverContext {
                workspace_root: dir.path(),
                env_map: &env_map,
                run_store: None,
                memory_reasoning_service: Some(&memory_reasoning_service("gpt-5.4-mini")),
                tools: &mut tools,
            },
            &mut outcome,
        )
        .unwrap();

        assert_eq!(
            config
                .query_expansion
                .as_ref()
                .map(|config| config.service.model.as_str()),
            Some("claude-query")
        );
        assert_eq!(
            config
                .rerank
                .as_ref()
                .map(|config| config.service.model.as_str()),
            Some("claude-rerank")
        );
        assert!(outcome.diagnostics.is_empty());
    }
}
