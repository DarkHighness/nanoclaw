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
    ConnectedMcpServer, McpConnectOptions, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_options,
};
use agent::skills::{Skill, load_skill_roots};
use anyhow::{Context, Result};
use plugins::{
    build_plugin_activation_plan, dedup_mcp_servers, resolve_mcp_servers, resolved_skill_roots,
};
#[cfg(test)]
use preamble::DEFAULT_AGENT_PREAMBLE;
use preamble::build_runtime_preamble;
use provider::{build_backend, provider_summary};
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

    let store_handle = build_store(&config, &workspace_root).await?;
    let store = store_handle.store.clone();
    let stored_run_count = store.list_runs().await.unwrap_or_default().len();
    let backend =
        Arc::new(build_backend(&config).context("failed to initialize provider backend")?);
    let provider_summary = provider_summary(&config, &backend);
    let tool_context = ToolExecutionContext {
        workspace_root: workspace_root.clone(),
        worktree_root: Some(workspace_root.clone()),
        workspace_only: config.runtime.workspace_only,
        model_context_window_tokens: Some(context_tokens(&config)),
        ..Default::default()
    };
    let sandbox_policy = build_sandbox_policy(&config, &tool_context);
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
                config.hook_env.clone(),
                process_executor.clone(),
                sandbox_policy.clone(),
            ),
        ),
        Arc::new(runtime::ReqwestHttpHookExecutor::default()),
        Arc::new(runtime::NoopPromptHookEvaluator),
        Arc::new(runtime::NoopAgentHookEvaluator),
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
        &plugin_plan.driver_activations,
        &workspace_root,
        Some(store.clone()),
        &mut tools,
        agent::UnknownDriverPolicy::Warn,
    )?;
    let mut mcp_server_configs = config.mcp_servers.clone();
    mcp_server_configs.extend(plugin_plan.mcp_servers.clone());
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
    let instructions = build_runtime_preamble(&config, &skill_catalog, &plugin_plan.instructions);
    let mut runtime_hooks = plugin_plan.hooks.clone();
    let skill_hooks = skills
        .iter()
        .flat_map(|skill| skill.hooks.clone())
        .collect::<Vec<_>>();
    runtime_hooks.extend(skill_hooks.clone());
    let skill_names = skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect::<Vec<_>>();
    let context_tokens = context_tokens(&config);
    let compact_trigger_tokens = config
        .runtime
        .compact_trigger_tokens
        .unwrap_or((context_tokens * 3) / 4);
    let compact_preserve_recent_messages =
        config.runtime.compact_preserve_recent_messages.unwrap_or(8);
    let runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(tool_context)
        .tool_approval_handler(Arc::new(InteractiveToolApprovalHandler::default()))
        .conversation_compactor(Arc::new(ModelConversationCompactor::new(backend.clone())))
        .compaction_config(CompactionConfig {
            enabled: config.runtime.auto_compact,
            context_window_tokens: context_tokens,
            trigger_tokens: compact_trigger_tokens,
            preserve_recent_messages: compact_preserve_recent_messages,
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

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_AGENT_PREAMBLE, bootstrap_from_dir, build_plugin_activation_plan,
        build_runtime_preamble, build_sandbox_policy, describe_sandbox_policy,
        resolved_skill_roots,
    };
    use crate::config::{AgentCoreConfig, ProviderKind};
    use crate::test_support::lock_env_test;
    use agent::skills::load_skill_roots;
    use tempfile::tempdir;
    use tokio::fs;
    use tools::{NetworkPolicy, SandboxBackendStatus, SandboxMode, ToolExecutionContext};
    use types::{ToolName, ToolOrigin};

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
                system_prompt = "Keep answers short."
                skill_roots = ["skills"]

                [provider]
                kind = "openai"
                model = "gpt-4.1-mini"

                [provider.env]
                OPENAI_API_KEY = "test-key"

                [runtime]
                workspace_only = true
                store_dir = ".nanoclaw/custom-store"
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
                .any(|line| line.contains("provider: openai / gpt-4.1-mini"))
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
            config.system_prompt = Some("Project-specific prompt.".to_string());
            config.skill_roots = vec!["skills".to_string()];
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
                [provider]
                kind = "openai"
                model = "gpt-4.1-mini"

                [provider.env]
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
                [provider]
                kind = "openai"
                model = "gpt-4.1-mini"

                [provider.env]
                OPENAI_API_KEY = "test-key"

                [runtime]
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
            config.provider.kind = Some(ProviderKind::OpenAi);
            config.provider.model = Some("gpt-4.1-mini".to_string());
            config.provider.additional_params =
                Some(serde_json::json!({"metadata":{"tier":"priority"}}));
            config
                .provider
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
            config.runtime.sandbox_fail_if_unavailable = true;
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
