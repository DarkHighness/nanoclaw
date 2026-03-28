mod plugins;
mod preamble;
mod provider;
mod runtime_settings;
mod store_support;
mod summary;

use crate::{
    InteractiveToolApprovalHandler, RuntimeTui, TuiStartupSummary, config::AgentCoreConfig,
};
use agent::mcp::{
    ConnectedMcpServer, McpConnectOptions, McpServerConfig, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_options,
};
use agent::skills::{Skill, load_skill_roots};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use plugins::{
    build_plugin_activation_plan, dedup_mcp_servers, resolve_mcp_servers, resolved_skill_roots,
};
#[cfg(test)]
use preamble::DEFAULT_AGENT_PREAMBLE;
use preamble::build_runtime_preamble;
use provider::{
    agent_backend_capabilities, build_backend, build_memory_reasoning_service,
    build_summary_backend, provider_summary,
};
use runtime::{
    AgentRuntime, AgentRuntimeBuilder, CompactionConfig, DefaultCommandHookExecutor, HookRunner,
    ModelConversationCompactor,
};
use runtime_settings::{build_sandbox_policy, context_tokens};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::RunStore;
use store_support::{StoreHandle, build_store};
use summary::build_startup_summary;
#[cfg(test)]
use tools::describe_sandbox_policy;
use tools::{
    BashTool, EditTool, GlobTool, GrepTool, ListTool, ManagedPolicyProcessExecutor, PatchTool,
    ReadTool, SandboxBackendStatus, ToolExecutionContext, ToolRegistry, WriteTool,
    ensure_sandbox_policy_supported,
};
#[cfg(feature = "web-tools")]
use tools::{WebFetchTool, WebSearchBackendsTool, WebSearchTool};
use tracing::{info, warn};
use types::HookRegistration;

pub struct BootArtifacts {
    pub workspace_root: PathBuf,
    pub config: AgentCoreConfig,
    pub runtime: AgentRuntime,
    pub store: Arc<dyn RunStore>,
    pub connected_mcp_servers: Vec<ConnectedMcpServer>,
    pub startup_summary: TuiStartupSummary,
    pub skills: Vec<Skill>,
    pub skill_names: Vec<String>,
    pub provider_summary: String,
    pub store_label: String,
    pub store_warning: Option<String>,
}

impl BootArtifacts {
    #[must_use]
    pub fn into_tui(self) -> RuntimeTui {
        let Self {
            runtime,
            store,
            workspace_root,
            config,
            connected_mcp_servers,
            startup_summary,
            skills,
            ..
        } = self;
        RuntimeTui::new(
            runtime,
            store,
            workspace_root,
            &config,
            connected_mcp_servers,
            skills,
            startup_summary,
        )
    }
}

struct DriverHostInputs {
    runtime_hooks: Vec<HookRegistration>,
    mcp_server_configs: Vec<McpServerConfig>,
    runtime_instructions: Vec<String>,
}

fn merge_driver_host_inputs(
    runtime_hooks: Vec<HookRegistration>,
    mcp_server_configs: Vec<McpServerConfig>,
    runtime_instructions: Vec<String>,
    driver_outcome: &agent::DriverActivationOutcome,
) -> DriverHostInputs {
    let mut merged = DriverHostInputs {
        runtime_hooks,
        mcp_server_configs,
        runtime_instructions,
    };
    // Driver output is part of the same host boot pipeline as declarative plugin
    // output. Keep the merge in one helper so both host apps preserve the same
    // contribution ordering and tests can assert the full startup input set.
    driver_outcome.extend_host_inputs(
        &mut merged.runtime_hooks,
        &mut merged.mcp_server_configs,
        &mut merged.runtime_instructions,
    );
    merged
}

pub async fn bootstrap_from_dir(dir: impl AsRef<Path>) -> Result<BootArtifacts> {
    let workspace_root = dir.as_ref().to_path_buf();
    info!(workspace = %workspace_root.display(), "bootstrapping reference TUI");
    let config = AgentCoreConfig::load_from_dir(&workspace_root)
        .context("failed to load agent-core config")?;
    bootstrap_from_parts(workspace_root, config).await
}

async fn bootstrap_from_parts(
    workspace_root: PathBuf,
    config: AgentCoreConfig,
) -> Result<BootArtifacts> {
    let plugin_plan = build_plugin_activation_plan(&config, &workspace_root)
        .context("failed to build plugin activation plan")?;
    let skill_roots = resolved_skill_roots(&config, &workspace_root, &plugin_plan);
    let env_map = EnvMap::from_workspace_dir(&workspace_root)
        .context("failed to resolve environment for memory reasoning service")?;

    let store_handle = build_store(&config, &workspace_root).await?;
    let store = store_handle.store.clone();
    let stored_run_count = store.list_runs().await.unwrap_or_default().len();
    let backend =
        Arc::new(build_backend(&config).context("failed to initialize provider backend")?);
    let summary_backend =
        Arc::new(build_summary_backend(&config).context("failed to initialize summary backend")?);
    let provider_summary = provider_summary(&config);
    let tool_context = ToolExecutionContext {
        workspace_root: workspace_root.clone(),
        worktree_root: Some(workspace_root.clone()),
        workspace_only: config.core.host.workspace_only,
        model_context_window_tokens: Some(context_tokens(&config)),
        ..Default::default()
    };
    let sandbox_policy = build_sandbox_policy(&config, &tool_context);
    let tool_context = tool_context.with_sandbox_policy(sandbox_policy.clone());
    let sandbox_status = ensure_sandbox_policy_supported(&sandbox_policy)
        .context("sandbox policy cannot be enforced on this host")?;
    match &sandbox_status {
        SandboxBackendStatus::Available { kind } => {
            info!(backend = kind.as_str(), "sandbox backend available");
        }
        SandboxBackendStatus::Unavailable { reason } => {
            warn!(
                "sandbox enforcement unavailable; local processes will fall back to host execution: {reason}"
            );
        }
        SandboxBackendStatus::NotRequired => {}
    }
    let process_executor = Arc::new(ManagedPolicyProcessExecutor::new());
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(
            DefaultCommandHookExecutor::with_process_executor_and_policy(
                config.core.hook_env.clone(),
                process_executor.clone(),
                sandbox_policy.clone(),
            ),
        ),
        Arc::new(runtime::ReqwestHttpHookExecutor::default()),
        Arc::new(runtime::FailClosedPromptHookEvaluator),
        Arc::new(runtime::FailClosedAgentHookEvaluator),
        Arc::new(runtime::DefaultWasmHookExecutor::default()),
    ));

    let mut tools = ToolRegistry::new();
    tools.register(ReadTool::new());
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(PatchTool::new());
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(BashTool::with_process_executor_and_policy(
        process_executor.clone(),
        sandbox_policy.clone(),
    ));
    #[cfg(feature = "web-tools")]
    {
        tools.register(WebSearchTool::new());
        tools.register(WebSearchBackendsTool::new());
        tools.register(WebFetchTool::new());
    }

    let driver_outcome = agent::activate_driver_requests(
        &plugin_plan.runtime_activations,
        &workspace_root,
        Some(store.clone()),
        Some(build_memory_reasoning_service(&config, &env_map)),
        &mut tools,
        agent::UnknownDriverPolicy::Warn,
    )?;
    let DriverHostInputs {
        mut runtime_hooks,
        mcp_server_configs,
        runtime_instructions,
    } = merge_driver_host_inputs(
        plugin_plan.hooks.clone(),
        config
            .core
            .mcp_servers
            .iter()
            .cloned()
            .chain(plugin_plan.mcp_servers.iter().cloned())
            .collect(),
        plugin_plan.instructions.clone(),
        &driver_outcome,
    );
    let mcp_servers = resolve_mcp_servers(&mcp_server_configs, &workspace_root);
    let connected_mcp_servers = connect_and_catalog_mcp_servers_with_options(
        &dedup_mcp_servers(mcp_servers),
        McpConnectOptions {
            process_executor,
            sandbox_policy: sandbox_policy.clone(),
            ..Default::default()
        },
    )
    .await
    .context("failed to connect configured MCP servers")?;
    for server in &connected_mcp_servers {
        for adapter in catalog_tools_as_registry_entries(server.client.clone())
            .await
            .context("failed to register MCP tools")?
        {
            tools.register(adapter);
        }
    }

    let skill_catalog = load_skill_roots(&skill_roots)
        .await
        .context("failed to load configured skill roots")?;
    let skills = skill_catalog.all().to_vec();
    ensure_model_supports_registered_tools(
        &config.primary_profile,
        agent_backend_capabilities(&config.primary_profile),
        &tools,
        "primary",
    )?;
    let instructions = build_runtime_preamble(&config, &skill_catalog, &runtime_instructions);
    let skill_hooks = skills
        .iter()
        .flat_map(|skill| skill.hooks.clone())
        .collect::<Vec<_>>();
    runtime_hooks.extend(skill_hooks.clone());
    let skill_names = skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect::<Vec<_>>();
    let runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(tool_context)
        .tool_approval_handler(Arc::new(InteractiveToolApprovalHandler::default()))
        .conversation_compactor(Arc::new(ModelConversationCompactor::new(summary_backend)))
        .compaction_config(CompactionConfig {
            enabled: config.primary_profile.auto_compact,
            context_window_tokens: config.primary_profile.context_window_tokens,
            trigger_tokens: config.primary_profile.compact_trigger_tokens,
            preserve_recent_messages: config.primary_profile.compact_preserve_recent_messages,
        })
        .instructions(instructions)
        .hooks(runtime_hooks)
        .skill_catalog(skill_catalog)
        .build();
    let startup_summary = build_startup_summary(
        &runtime.run_id(),
        &workspace_root,
        &provider_summary,
        &store_handle,
        stored_run_count,
        &runtime.tool_specs(),
        &skill_names,
        &connected_mcp_servers,
        &config,
        &plugin_plan,
        &driver_outcome.warnings,
        &driver_outcome.diagnostics,
        &sandbox_policy,
        &sandbox_status,
    );

    Ok(BootArtifacts {
        workspace_root,
        config,
        runtime,
        store,
        connected_mcp_servers,
        startup_summary,
        skills,
        skill_names,
        provider_summary,
        store_label: store_handle.label,
        store_warning: store_handle.warning,
    })
}

fn ensure_model_supports_registered_tools(
    profile: &nanoclaw_config::ResolvedAgentProfile,
    capabilities: runtime::ModelBackendCapabilities,
    tools: &ToolRegistry,
    profile_label: &str,
) -> Result<()> {
    let registered_tool_count = tools.names().len();
    if capabilities.tool_calls || registered_tool_count == 0 {
        return Ok(());
    }
    bail!(
        "{profile_label} profile `{}` uses model `{}` without tool-call support, but the host registered {registered_tool_count} tools",
        profile.profile_name,
        profile.model.model,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_AGENT_PREAMBLE, bootstrap_from_dir, build_plugin_activation_plan,
        build_runtime_preamble, build_sandbox_policy, describe_sandbox_policy,
        merge_driver_host_inputs, resolved_skill_roots,
    };
    use crate::config::AgentCoreConfig;
    use crate::test_support::lock_env_test;
    use agent::DriverActivationOutcome;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent::skills::load_skill_roots;
    use std::collections::BTreeMap;
    use tempfile::tempdir;
    use tokio::fs;
    use tools::{NetworkPolicy, SandboxBackendStatus, SandboxMode, ToolExecutionContext};
    use types::{HookEvent, HookHandler, HookRegistration, HttpHookHandler, ToolName, ToolOrigin};

    #[test]
    fn driver_outcome_extends_reference_tui_boot_inputs() {
        let merged = merge_driver_host_inputs(
            vec![HookRegistration {
                name: "existing-hook".to_string(),
                event: HookEvent::Stop,
                matcher: None,
                handler: HookHandler::Http(HttpHookHandler {
                    url: "https://example.test/existing".to_string(),
                    method: "POST".to_string(),
                    headers: BTreeMap::new(),
                }),
                timeout_ms: None,
                execution: None,
            }],
            vec![McpServerConfig {
                name: "existing-mcp".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: "stdio-server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            vec!["existing instruction".to_string()],
            &DriverActivationOutcome {
                warnings: Vec::new(),
                hooks: vec![HookRegistration {
                    name: "driver-hook".to_string(),
                    event: HookEvent::SessionStart,
                    matcher: None,
                    handler: HookHandler::Http(HttpHookHandler {
                        url: "https://example.test/hook".to_string(),
                        method: "POST".to_string(),
                        headers: BTreeMap::new(),
                    }),
                    timeout_ms: Some(250),
                    execution: None,
                }],
                mcp_servers: vec![McpServerConfig {
                    name: "driver-mcp".to_string(),
                    transport: McpTransportConfig::StreamableHttp {
                        url: "https://example.test/mcp".to_string(),
                        headers: BTreeMap::new(),
                    },
                }],
                instructions: vec!["driver instruction".to_string()],
                diagnostics: vec!["prepared runtime".to_string()],
            },
        );

        assert_eq!(
            merged
                .runtime_hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook", "driver-hook"]
        );
        assert_eq!(
            merged
                .mcp_server_configs
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp", "driver-mcp"]
        );
        assert_eq!(
            merged.runtime_instructions,
            vec![
                "existing instruction".to_string(),
                "driver instruction".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn bootstraps_runtime_from_configured_workspace() {
        let _guard = lock_env_test();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("skills").join("useful"))
            .await
            .unwrap();
        fs::write(
            dir.path().join("skills").join("useful").join("SKILL.md"),
            r#"---
name: useful
description: Helpful skill
---

Use this skill when asked.
"#,
        )
        .await
        .unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/config"))
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/apps"))
            .await
            .unwrap();
        fs::write(
            crate::config::core_config_path(dir.path()),
            r#"
                global_system_prompt = "Keep answers short."
                skill_roots = ["skills"]

                [host]
                workspace_only = true
                store_dir = ".nanoclaw/custom-store"

                [models.gpt_5_4_default]
                provider = "openai"
                model = "gpt-5.4"
                context_window_tokens = 400000
                max_output_tokens = 128000
                compact_trigger_tokens = 320000

                [models.gpt_5_4_default.env]
                OPENAI_API_KEY = "test-key"
            "#,
        )
        .await
        .unwrap();
        fs::write(
            AgentCoreConfig::app_config_path(dir.path()),
            r#"
                [tui]
                command_prefix = ":"
            "#,
        )
        .await
        .unwrap();

        let artifacts = bootstrap_from_dir(dir.path()).await.unwrap();

        assert_eq!(artifacts.config.tui.command_prefix, ":");
        assert_eq!(artifacts.skill_names, vec!["useful".to_string()]);
        assert_eq!(artifacts.connected_mcp_servers.len(), 0);
        assert_eq!(
            artifacts.store_label,
            format!(
                "file {}",
                dir.path().join(".nanoclaw/custom-store").display()
            )
        );
        assert!(dir.path().join(".nanoclaw/custom-store").is_dir());
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.contains("provider: gpt_5_4_default -> openai / gpt-5.4"))
        );
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.contains("stored runs: 0"))
        );
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.contains("command prefix: :"))
        );
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.contains("sandbox: workspace-write, network off, "))
        );
        #[cfg(feature = "web-tools")]
        assert!(
            artifacts
                .runtime
                .tool_specs()
                .iter()
                .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
                .any(|tool| tool.name == types::ToolName::from("web_fetch"))
        );
        #[cfg(not(feature = "web-tools"))]
        assert!(
            !artifacts
                .runtime
                .tool_specs()
                .iter()
                .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
                .any(|tool| tool.name == types::ToolName::from("web_fetch"))
        );
        assert!(artifacts.store.list_runs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_preamble_is_built_in_code_from_system_prompt_and_skills() {
        let _guard = lock_env_test();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("skills").join("useful"))
            .await
            .unwrap();
        fs::write(
            dir.path().join("skills").join("useful").join("SKILL.md"),
            r#"---
name: useful
description: Helpful skill
---

Use this skill when asked.
"#,
        )
        .await
        .unwrap();
        let config = AgentCoreConfig::default().with_override(|config| {
            config.core.global_system_prompt = Some("Project-specific prompt.".to_string());
            config.core.skill_roots = vec!["skills".to_string()];
        });
        let plugin_plan = build_plugin_activation_plan(&config, dir.path()).unwrap();
        let skill_catalog =
            load_skill_roots(&resolved_skill_roots(&config, dir.path(), &plugin_plan))
                .await
                .unwrap();

        let preamble = build_runtime_preamble(&config, &skill_catalog, &plugin_plan.instructions);

        assert_eq!(preamble[0], DEFAULT_AGENT_PREAMBLE[0]);
        assert_eq!(preamble[1], DEFAULT_AGENT_PREAMBLE[1]);
        assert!(
            preamble
                .iter()
                .any(|line| line == "Project-specific prompt.")
        );
        assert!(
            preamble
                .iter()
                .any(|line| line.contains("Available workspace skills are listed below."))
        );
    }

    #[tokio::test]
    async fn boot_registers_memory_tools_from_builtin_plugin_slot() {
        let _guard = lock_env_test();
        let dir = tempdir().unwrap();
        fs::create_dir_all(
            dir.path()
                .join("builtin-plugins/memory-core/.nanoclaw-plugin"),
        )
        .await
        .unwrap();
        fs::write(dir.path().join("MEMORY.md"), "workspace preference")
            .await
            .unwrap();
        fs::write(
            dir.path()
                .join("builtin-plugins/memory-core/.nanoclaw-plugin/plugin.toml"),
            r#"
                id = "memory-core"
                kind = "memory"
                enabled_by_default = false

                [runtime]
                driver = "builtin.memory-core"
            "#,
        )
        .await
        .unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/config"))
            .await
            .unwrap();
        fs::write(
            crate::config::core_config_path(dir.path()),
            r#"
                [models.gpt_5_4_default]
                provider = "openai"
                model = "gpt-5.4"
                context_window_tokens = 400000
                max_output_tokens = 128000
                compact_trigger_tokens = 320000

                [models.gpt_5_4_default.env]
                OPENAI_API_KEY = "test-key"

                [plugins.slots]
                memory = "memory-core"
            "#,
        )
        .await
        .unwrap();

        let artifacts = bootstrap_from_dir(dir.path()).await.unwrap();
        let tool_specs = artifacts.runtime.tool_specs();
        let tool_names = tool_specs
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert!(tool_names.contains(&ToolName::from("memory_search")));
        assert!(tool_names.contains(&ToolName::from("memory_get")));
        assert!(tool_names.contains(&ToolName::from("memory_list")));
        assert!(tool_names.contains(&ToolName::from("memory_record")));
        assert!(tool_names.contains(&ToolName::from("memory_promote")));
        assert!(tool_names.contains(&ToolName::from("memory_forget")));
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.contains("memory slot: memory-core"))
        );
    }

    #[tokio::test]
    async fn falls_back_to_memory_store_when_store_path_is_not_directory() {
        let _guard = lock_env_test();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/config"))
            .await
            .unwrap();
        fs::write(
            crate::config::core_config_path(dir.path()),
            r#"
                [models.gpt_5_4_default]
                provider = "openai"
                model = "gpt-5.4"
                context_window_tokens = 400000
                max_output_tokens = 128000
                compact_trigger_tokens = 320000

                [models.gpt_5_4_default.env]
                OPENAI_API_KEY = "test-key"

                [host]
                store_dir = "occupied"
            "#,
        )
        .await
        .unwrap();
        fs::write(dir.path().join("occupied"), "not a directory")
            .await
            .unwrap();

        let artifacts = bootstrap_from_dir(dir.path()).await.unwrap();

        assert_eq!(artifacts.store_label, "memory fallback");
        assert!(artifacts.store_warning.is_some());
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.contains("store: memory fallback"))
        );
        assert!(
            artifacts
                .startup_summary
                .sidebar
                .iter()
                .any(|line| line.starts_with("warning: failed to initialize file run store"))
        );
        assert!(artifacts.store.list_runs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn build_backend_applies_provider_additional_params() {
        let _guard = lock_env_test();
        let config = AgentCoreConfig::default().with_override(|config| {
            config
                .core
                .models
                .get_mut("gpt_5_4_default")
                .unwrap()
                .additional_params = Some(serde_json::json!({"metadata":{"tier":"priority"}}));
            config
                .core
                .models
                .get_mut("gpt_5_4_default")
                .unwrap()
                .env
                .insert("OPENAI_API_KEY".to_string(), "test-key".to_string());
        });

        let backend = super::build_backend(&config).unwrap();

        assert_eq!(
            backend.request_options().additional_params,
            Some(serde_json::json!({"metadata":{"tier":"priority"}}))
        );
    }

    #[test]
    fn runtime_sandbox_config_can_require_enforcing_backend() {
        let _guard = lock_env_test();
        let workspace = tempdir().unwrap();
        let config = AgentCoreConfig::default().with_override(|config| {
            config.core.host.sandbox_fail_if_unavailable = true;
        });
        let tool_context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };

        let policy = build_sandbox_policy(&config, &tool_context);

        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(policy.network, NetworkPolicy::Off);
        assert!(policy.fail_if_unavailable);
        assert_eq!(
            describe_sandbox_policy(
                &policy,
                &SandboxBackendStatus::Unavailable {
                    reason: "no backend".to_string()
                }
            ),
            "workspace-write, network off, backend required but unavailable (no backend)"
        );
    }
}
