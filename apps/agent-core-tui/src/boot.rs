use crate::{
    InteractiveToolApprovalHandler, RuntimeTui, TuiStartupSummary,
    config::{AgentCoreConfig, ProviderKind},
};
use agent_core::mcp::{
    ConnectedMcpServer, McpServerConfig, McpTransportConfig, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers,
};
use agent_core::rig::{
    RigBackendDescriptor, RigModelBackend, RigProviderDescriptor, RigRequestOptions,
};
use agent_core::skills::{Skill, load_skill_roots};
use agent_core_runtime::{
    AgentRuntime, AgentRuntimeBuilder, CompactionConfig, DefaultCommandHookExecutor, HookRunner,
    ModelConversationCompactor,
};
use agent_core_store::{FileRunStore, InMemoryRunStore, RunStore};
use agent_core_tools::{
    BashTool, EditTool, GlobTool, GrepTool, ListTool, PatchTool, ReadTool, ToolExecutionContext,
    ToolRegistry, WriteTool,
};
#[cfg(feature = "web-tools")]
use agent_core_tools::{WebFetchTool, WebSearchTool};
use agent_core_types::ToolOrigin;
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_AGENT_PREAMBLE: &[&str] = &[
    "You are a general-purpose software agent operating inside the current workspace.",
    "Inspect available state and use tools before guessing. Treat tool results, approvals, and denials as authoritative runtime feedback.",
];

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

struct StoreHandle {
    store: Arc<dyn RunStore>,
    label: String,
    warning: Option<String>,
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
    let config = AgentCoreConfig::load_from_dir(&workspace_root)
        .context("failed to load agent-core config")?;
    bootstrap_from_parts(workspace_root, config).await
}

async fn bootstrap_from_parts(
    workspace_root: PathBuf,
    config: AgentCoreConfig,
) -> Result<BootArtifacts> {
    let skill_roots = resolved_skill_roots(&config, &workspace_root);

    let store_handle = build_store(&config, &workspace_root).await?;
    let store = store_handle.store.clone();
    let stored_run_count = store.list_runs().await.unwrap_or_default().len();
    let backend =
        Arc::new(build_backend(&config).context("failed to initialize provider backend")?);
    let provider_summary = provider_summary(&config, &backend);
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(DefaultCommandHookExecutor::new(config.hook_env.clone())),
        Arc::new(agent_core_runtime::ReqwestHttpHookExecutor::default()),
        Arc::new(agent_core_runtime::NoopPromptHookEvaluator),
        Arc::new(agent_core_runtime::NoopAgentHookEvaluator),
    ));

    let mut tools = ToolRegistry::new();
    tools.register(ReadTool::new());
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(PatchTool::new());
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(BashTool::new());
    #[cfg(feature = "web-tools")]
    {
        tools.register(WebSearchTool::new());
        tools.register(WebFetchTool::new());
    }

    let mcp_servers = resolve_mcp_servers(&config.mcp_servers, &workspace_root);
    let connected_mcp_servers = connect_and_catalog_mcp_servers(&mcp_servers)
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
    let instructions = build_runtime_preamble(&config, &skill_catalog);
    let skill_hooks = skills
        .iter()
        .flat_map(|skill| skill.hooks.clone())
        .collect::<Vec<_>>();
    let skill_names = skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect::<Vec<_>>();
    let context_tokens = config.runtime.context_tokens.unwrap_or(128_000);
    let compact_trigger_tokens = config
        .runtime
        .compact_trigger_tokens
        .unwrap_or((context_tokens * 3) / 4);
    let compact_preserve_recent_messages =
        config.runtime.compact_preserve_recent_messages.unwrap_or(8);
    let runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(ToolExecutionContext {
            workspace_root: workspace_root.clone(),
            worktree_root: Some(workspace_root.clone()),
            workspace_only: config.runtime.workspace_only,
            model_context_window_tokens: Some(context_tokens),
            ..Default::default()
        })
        .tool_approval_handler(Arc::new(InteractiveToolApprovalHandler::default()))
        .conversation_compactor(Arc::new(ModelConversationCompactor::new(backend.clone())))
        .compaction_config(CompactionConfig {
            enabled: config.runtime.auto_compact,
            context_window_tokens: context_tokens,
            trigger_tokens: compact_trigger_tokens,
            preserve_recent_messages: compact_preserve_recent_messages,
        })
        .instructions(instructions)
        .hooks(skill_hooks)
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

fn build_runtime_preamble(
    config: &AgentCoreConfig,
    skill_catalog: &agent_core::skills::SkillCatalog,
) -> Vec<String> {
    let mut preamble = DEFAULT_AGENT_PREAMBLE
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    if let Some(system_prompt) = config.system_prompt.as_deref().map(str::trim) {
        if !system_prompt.is_empty() {
            preamble.push(system_prompt.to_string());
        }
    }
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}

async fn build_store(config: &AgentCoreConfig, workspace_root: &Path) -> Result<StoreHandle> {
    let store_dir = config.resolved_store_dir(workspace_root);
    match FileRunStore::open(&store_dir).await {
        Ok(store) => Ok(StoreHandle {
            store: Arc::new(store),
            label: format!("file {}", store_dir.display()),
            warning: None,
        }),
        Err(error) => {
            let warning = format!(
                "failed to initialize file run store at {}: {error}",
                store_dir.display()
            );
            eprintln!("warning: {warning}; falling back to in-memory store");
            Ok(StoreHandle {
                store: Arc::new(InMemoryRunStore::new()),
                label: "memory fallback".to_string(),
                warning: Some(warning),
            })
        }
    }
}

fn build_backend(config: &AgentCoreConfig) -> Result<RigModelBackend> {
    let model = config.provider.model.clone().ok_or_else(|| {
        anyhow!(
            "missing provider model; set `provider.model` in agent-core.toml or `AGENT_CORE_MODEL`"
        )
    })?;
    let provider_kind = resolved_provider_kind(config, &model);
    let descriptor = RigBackendDescriptor::new(match provider_kind {
        ProviderKind::OpenAi => RigProviderDescriptor::openai(model),
        ProviderKind::Anthropic => RigProviderDescriptor::anthropic(model),
    });

    Ok(RigModelBackend::from_settings_with_api_key(
        descriptor,
        RigRequestOptions {
            temperature: config.provider.temperature,
            max_tokens: config.provider.max_tokens,
            additional_params: config.provider.additional_params.clone(),
        },
        config.provider.base_url.clone(),
        configured_provider_api_key(config, &provider_kind),
    )?)
}

fn configured_provider_api_key(
    config: &AgentCoreConfig,
    provider_kind: &ProviderKind,
) -> Option<String> {
    let env_key = match provider_kind {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    config.provider.env.get(env_key).cloned()
}

fn provider_summary(config: &AgentCoreConfig, backend: &RigModelBackend) -> String {
    let provider = match resolved_provider_kind(config, &backend.descriptor().provider.model) {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    };
    format!("{provider} / {}", backend.descriptor().provider.model)
}

fn build_startup_summary(
    run_id: &agent_core_types::RunId,
    workspace_root: &Path,
    provider_summary: &str,
    store_handle: &StoreHandle,
    stored_run_count: usize,
    tool_specs: &[agent_core_types::ToolSpec],
    skill_names: &[String],
    mcp_servers: &[ConnectedMcpServer],
    config: &AgentCoreConfig,
) -> TuiStartupSummary {
    let local_tools = tool_specs
        .iter()
        .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
        .count();
    let mcp_tools = tool_specs.len().saturating_sub(local_tools);
    let mut sidebar = vec![
        format!("run: {}", preview_id(&run_id.0)),
        format!("workspace: {}", workspace_root.display()),
        format!("provider: {provider_summary}"),
        format!("store: {}", store_handle.label),
        format!("stored runs: {stored_run_count}"),
        format!(
            "tools: {} total ({local_tools} local, {mcp_tools} mcp)",
            tool_specs.len()
        ),
        format!("skills: {}", skill_names.len()),
        format!("mcp servers: {}", mcp_servers.len()),
        format!("command prefix: {}", config.tui.command_prefix),
        format!(
            "compaction: {}",
            if config.runtime.auto_compact {
                format!(
                    "auto at ~{} / {} tokens, keep {} recent messages",
                    config
                        .runtime
                        .compact_trigger_tokens
                        .unwrap_or(config.runtime.context_tokens.unwrap_or(128_000) * 3 / 4),
                    config.runtime.context_tokens.unwrap_or(128_000),
                    config.runtime.compact_preserve_recent_messages.unwrap_or(8),
                )
            } else {
                "disabled".to_string()
            }
        ),
    ];
    if let Some(warning) = &store_handle.warning {
        sidebar.push(format!("warning: {warning}"));
    }
    if !skill_names.is_empty() {
        sidebar.push(format!("skill names: {}", preview_list(skill_names, 4)));
    }
    if !mcp_servers.is_empty() {
        sidebar.push(format!(
            "mcp names: {}",
            preview_list(
                &mcp_servers
                    .iter()
                    .map(|server| server.server_name.clone())
                    .collect::<Vec<_>>(),
                4,
            )
        ));
    }
    sidebar.push(
        "commands: /status /runs [query] /run <id> /export_run <id> <path> /compact [/notes] /skills /skill <name>"
            .to_string(),
    );

    TuiStartupSummary {
        sidebar_title: "Overview".to_string(),
        sidebar,
        status: "Ready. /status restores the startup overview.".to_string(),
    }
}

fn preview_list(items: &[String], max_items: usize) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    let mut preview = items.iter().take(max_items).cloned().collect::<Vec<_>>();
    if items.len() > max_items {
        preview.push(format!("+{}", items.len() - max_items));
    }
    preview.join(", ")
}

fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn resolved_provider_kind(config: &AgentCoreConfig, model: &str) -> ProviderKind {
    if let Some(kind) = &config.provider.kind {
        return kind.clone();
    }
    if model.trim().starts_with("claude") {
        return ProviderKind::Anthropic;
    }
    let has_openai = config.provider.env.contains_key("OPENAI_API_KEY")
        || std::env::var("OPENAI_API_KEY").is_ok();
    let has_anthropic = config.provider.env.contains_key("ANTHROPIC_API_KEY")
        || std::env::var("ANTHROPIC_API_KEY").is_ok();
    match (has_openai, has_anthropic) {
        (false, true) => ProviderKind::Anthropic,
        _ => ProviderKind::OpenAi,
    }
}

fn resolved_skill_roots(config: &AgentCoreConfig, workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots = config.resolved_skill_roots(workspace_root);
    if roots.is_empty() {
        let default_root = workspace_root.join(".agent-core/skills");
        if default_root.exists() {
            roots.push(default_root);
        }
    }
    roots
}

fn resolve_mcp_servers(configs: &[McpServerConfig], workspace_root: &Path) -> Vec<McpServerConfig> {
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

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_AGENT_PREAMBLE, bootstrap_from_dir, build_runtime_preamble, resolved_skill_roots,
    };
    use crate::config::{AgentCoreConfig, ProviderKind};
    use agent_core::skills::load_skill_roots;
    use agent_core_types::ToolOrigin;
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn bootstraps_runtime_from_configured_workspace() {
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
        fs::write(
            dir.path().join("agent-core.toml"),
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
                store_dir = ".agent-core/custom-store"

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
                dir.path().join(".agent-core/custom-store").display()
            )
        );
        assert!(dir.path().join(".agent-core/custom-store").is_dir());
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
        #[cfg(feature = "web-tools")]
        assert!(
            artifacts
                .runtime
                .tool_specs()
                .iter()
                .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
                .any(|tool| tool.name == "web_fetch")
        );
        #[cfg(not(feature = "web-tools"))]
        assert!(
            !artifacts
                .runtime
                .tool_specs()
                .iter()
                .filter(|tool| matches!(tool.origin, ToolOrigin::Local))
                .any(|tool| tool.name == "web_fetch")
        );
        assert!(artifacts.store.list_runs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_preamble_is_built_in_code_from_system_prompt_and_skills() {
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
        let skill_catalog = load_skill_roots(&resolved_skill_roots(&config, dir.path()))
            .await
            .unwrap();

        let preamble = build_runtime_preamble(&config, &skill_catalog);

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
    async fn falls_back_to_memory_store_when_store_path_is_not_directory() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("agent-core.toml"),
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
}
