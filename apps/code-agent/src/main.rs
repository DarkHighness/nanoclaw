mod config;
mod options;
mod provider;
mod tui;

use crate::options::AppOptions;
use crate::provider::build_backend;
use agent::runtime::{
    CompactionConfig, DefaultCommandHookExecutor, LoopDetectionConfig, ModelConversationCompactor,
    NoopToolApprovalPolicy, RuntimeSubagentExecutor, ToolApprovalHandler,
};
use agent::tools::{SandboxBackendStatus, ensure_sandbox_policy_supported};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, AgentWorkspaceLayout, BashTool, CodeDefinitionsTool,
    CodeDocumentSymbolsTool, CodeIntelBackend, CodeReferencesTool, CodeSymbolSearchTool, EditTool,
    GlobTool, GrepTool, HookRunner, InMemoryRunStore, ListTool, ManagedCodeIntelBackend,
    ManagedCodeIntelOptions, ManagedPolicyProcessExecutor, PatchTool, ReadTool, SandboxPolicy,
    Skill, SkillCatalog, TaskTool, TodoListState, TodoReadTool, TodoWriteTool,
    ToolExecutionContext, ToolRegistry, WorkspaceTextCodeIntelBackend, WriteTool,
};
use agent_env::EnvMap;
use anyhow::{Context, Result};
use nanoclaw_config::PluginsConfig;
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tui::{CodeAgentTui, make_tui_support};

const DEFAULT_CONTEXT_TOKENS: usize = 128_000;
const DEFAULT_TRIGGER_TOKENS: usize = 96_000;
const DEFAULT_PRESERVE_RECENT_MESSAGES: usize = 8;

fn main() -> Result<()> {
    let workspace_root = env::current_dir().context("failed to resolve current workspace")?;
    let env_map = EnvMap::from_workspace_dir(&workspace_root)?;
    inject_process_env(&env_map);
    let _tracing_guard = init_tracing(&workspace_root)?;
    let options = AppOptions::from_env_and_args(&workspace_root, &env_map)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async_main(workspace_root, options)))
}

fn init_tracing(workspace_root: &Path) -> Result<WorkerGuard> {
    let layout = AgentWorkspaceLayout::new(workspace_root);
    layout.ensure_standard_layout().with_context(|| {
        format!(
            "failed to materialize workspace state layout at {}",
            layout.state_dir().display()
        )
    })?;
    let log_dir = layout.logs_dir();
    let file_appender = tracing_appender::rolling::never(log_dir, "code-agent.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = EnvFilter::try_new(agent_env::log_filter_or_default(
        "info,runtime=debug,provider=debug",
    ))
    .context("failed to parse tracing filter")?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;
    Ok(guard)
}

async fn async_main(workspace_root: PathBuf, options: AppOptions) -> Result<()> {
    let (ui_state, approval_bridge, approval_handler) = make_tui_support();
    let tool_context = build_tool_context(&workspace_root);
    let sandbox_policy = build_sandbox_policy(&options, &tool_context);
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
            eprintln!(
                "warning: sandbox enforcement unavailable; local processes will fall back to host execution: {reason}"
            );
        }
        SandboxBackendStatus::NotRequired => {}
    }
    let (runtime, skills) = build_runtime(
        &options,
        &workspace_root,
        approval_handler,
        tool_context,
        sandbox_policy,
    )
    .await?;
    let provider_label = options.provider.to_string();
    let model = options.model.clone();
    let initial_prompt = options.one_shot_prompt.clone();
    CodeAgentTui::new(
        runtime,
        workspace_root,
        provider_label,
        model,
        skills,
        initial_prompt,
        ui_state,
        approval_bridge,
    )
    .run()
    .await
}

async fn build_runtime(
    options: &AppOptions,
    workspace_root: &Path,
    approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_context: ToolExecutionContext,
    sandbox_policy: SandboxPolicy,
) -> Result<(AgentRuntime, Vec<Skill>)> {
    let backend = Arc::new(build_backend(
        options.provider,
        options.model.clone(),
        options.base_url.clone(),
        options.temperature,
        options.max_tokens,
        options.additional_params.clone(),
        &options.provider_env,
    )?);
    let store = Arc::new(InMemoryRunStore::new());
    let plugin_plan = build_plugin_activation_plan(workspace_root, &options.plugins)
        .context("failed to build plugin activation plan")?;
    let skill_roots = resolve_skill_roots(&options.skill_roots, workspace_root, &plugin_plan);
    let skill_catalog = agent::skills::load_skill_roots(&skill_roots)
        .await
        .context("failed to load skill roots")?;
    let skills = skill_catalog.all().to_vec();
    let mut runtime_hooks = plugin_plan.hooks.clone();
    let skill_hooks = skills
        .iter()
        .flat_map(|skill| skill.hooks.clone())
        .collect::<Vec<_>>();
    runtime_hooks.extend(skill_hooks.clone());
    let instructions = build_system_preamble(
        options.system_prompt.as_deref(),
        &skill_catalog,
        &plugin_plan.instructions,
    );
    let compactor = Arc::new(ModelConversationCompactor::new(backend.clone()));
    let loop_detection_config = LoopDetectionConfig {
        enabled: true,
        ..LoopDetectionConfig::default()
    };
    let process_executor = Arc::new(ManagedPolicyProcessExecutor::new());
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(
            DefaultCommandHookExecutor::with_process_executor_and_policy(
                BTreeMap::new(),
                process_executor.clone(),
                sandbox_policy.clone(),
            ),
        ),
        Arc::new(agent::runtime::ReqwestHttpHookExecutor::default()),
        Arc::new(agent::runtime::NoopPromptHookEvaluator),
        Arc::new(agent::runtime::NoopAgentHookEvaluator),
        Arc::new(agent::runtime::DefaultWasmHookExecutor),
    ));
    let todo_state = TodoListState::default();
    // Managed LSP helpers run outside the normal user-invoked tool approval path.
    // Keep them behind explicit app-level config and use a separate host-managed
    // process policy until background helper execution shares the same approval
    // and sandbox contract as foreground tool calls.
    let managed_code_intel = options.lsp_enabled.then(|| {
        let mut lsp_options = ManagedCodeIntelOptions::for_workspace(workspace_root);
        lsp_options.auto_install = options.lsp_auto_install;
        if let Some(install_root) = &options.lsp_install_root {
            lsp_options.install_root = install_root.clone();
        }
        Arc::new(ManagedCodeIntelBackend::new(
            workspace_root.to_path_buf(),
            lsp_options,
            process_executor.clone(),
            SandboxPolicy::permissive(),
            SandboxPolicy::permissive(),
        ))
    });
    let code_intel_backend: Arc<dyn CodeIntelBackend> = managed_code_intel
        .clone()
        .map(|backend| backend as Arc<dyn CodeIntelBackend>)
        .unwrap_or_else(|| Arc::new(WorkspaceTextCodeIntelBackend::new()));

    let mut tools = ToolRegistry::new();
    if let Some(observer) = managed_code_intel.clone() {
        tools.register(ReadTool::with_file_activity_observer(observer.clone()));
        tools.register(WriteTool::with_file_activity_observer(observer.clone()));
        tools.register(EditTool::with_file_activity_observer(observer.clone()));
        tools.register(PatchTool::with_file_activity_observer(observer));
    } else {
        tools.register(ReadTool::new());
        tools.register(WriteTool::new());
        tools.register(EditTool::new());
        tools.register(PatchTool::new());
    }
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(BashTool::with_process_executor_and_policy(
        process_executor,
        sandbox_policy,
    ));
    tools.register(CodeSymbolSearchTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeDocumentSymbolsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeDefinitionsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeReferencesTool::with_backend(code_intel_backend));
    tools.register(TodoReadTool::new(todo_state.clone()));
    tools.register(TodoWriteTool::new(todo_state));
    // Driver-backed plugins expand into normal local tools here so the runtime and subagent
    // surfaces stay identical regardless of whether a capability came from builtin boot code or a
    // plugin slot selection.
    agent::activate_driver_requests(
        &plugin_plan.runtime_activations,
        workspace_root,
        Some(store.clone()),
        &mut tools,
        agent::UnknownDriverPolicy::Error,
    )?;
    let subagent_executor = RuntimeSubagentExecutor::new(
        backend.clone(),
        hook_runner.clone(),
        store.clone(),
        tools.clone(),
        tool_context.clone(),
        approval_handler.clone(),
        Arc::new(NoopToolApprovalPolicy),
        compactor.clone(),
        CompactionConfig {
            enabled: true,
            context_window_tokens: DEFAULT_CONTEXT_TOKENS,
            trigger_tokens: DEFAULT_TRIGGER_TOKENS,
            preserve_recent_messages: DEFAULT_PRESERVE_RECENT_MESSAGES,
        },
        loop_detection_config.clone(),
        instructions.clone(),
        runtime_hooks.clone(),
        skill_catalog.clone(),
    );
    tools.register(TaskTool::new(Arc::new(subagent_executor)));

    let runtime = AgentRuntimeBuilder::new(backend.clone(), store)
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(tool_context)
        .tool_approval_handler(approval_handler)
        .conversation_compactor(compactor)
        .compaction_config(CompactionConfig {
            enabled: true,
            context_window_tokens: DEFAULT_CONTEXT_TOKENS,
            trigger_tokens: DEFAULT_TRIGGER_TOKENS,
            preserve_recent_messages: DEFAULT_PRESERVE_RECENT_MESSAGES,
        })
        .loop_detection_config(loop_detection_config)
        .instructions(instructions)
        .hooks(runtime_hooks)
        .skill_catalog(skill_catalog)
        .build();

    Ok((runtime, skills))
}

fn build_sandbox_policy(
    options: &AppOptions,
    tool_context: &ToolExecutionContext,
) -> SandboxPolicy {
    // `code-agent` keeps the workspace-derived sandbox posture but lets the
    // operator decide whether missing enforcement backends are tolerable.
    tool_context
        .sandbox_scope()
        .recommended_policy()
        .with_fail_if_unavailable(options.sandbox_fail_if_unavailable)
}

fn build_tool_context(workspace_root: &Path) -> ToolExecutionContext {
    ToolExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        worktree_root: Some(workspace_root.to_path_buf()),
        workspace_only: true,
        model_context_window_tokens: Some(DEFAULT_CONTEXT_TOKENS),
        ..Default::default()
    }
}

fn build_system_preamble(
    system_prompt: Option<&str>,
    skill_catalog: &SkillCatalog,
    plugin_instructions: &[String],
) -> Vec<String> {
    let mut preamble = vec![
        "You are a general-purpose coding agent operating inside the current workspace."
            .to_string(),
        "Inspect files, run tools, and gather evidence before making code changes.".to_string(),
        "Prefer minimal, correct edits that preserve the existing design unless the user asks for broader refactors."
            .to_string(),
        "Use patch for coordinated multi-file mutations, and use write or edit for single-file creation or precise local edits."
            .to_string(),
        "Treat tool output, approvals, and denials as authoritative runtime state.".to_string(),
        "Maintain a concise plan with todo_read and todo_write for multi-step work.".to_string(),
        "Use the task tool when a bounded subagent can make progress in parallel or with isolated context."
            .to_string(),
    ];
    if let Some(system_prompt) = system_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        preamble.push(system_prompt.to_string());
    }
    preamble.extend(plugin_instructions.iter().cloned());
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}

fn resolve_skill_roots(
    configured_roots: &[PathBuf],
    workspace_root: &Path,
    plugin_plan: &agent::plugins::PluginActivationPlan,
) -> Vec<PathBuf> {
    let mut roots = if configured_roots.is_empty() {
        default_skill_roots(workspace_root)
    } else {
        configured_roots.to_vec()
    };
    roots.extend(plugin_plan.skill_roots.clone());
    roots.retain(|path| path.exists());
    roots.sort();
    roots.dedup();
    roots
}

fn default_skill_roots(workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_if_exists(&mut roots, workspace_root.join(".codex/skills"));
    push_if_exists(
        &mut roots,
        AgentWorkspaceLayout::new(workspace_root).skills_dir(),
    );
    if let Some(home) = agent_env::home_dir() {
        push_if_exists(&mut roots, home.join(".codex/skills"));
    }
    roots
}

fn push_if_exists(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if path.exists() && !roots.iter().any(|candidate| candidate == &path) {
        roots.push(path);
    }
}

fn build_plugin_activation_plan(
    workspace_root: &Path,
    plugins: &PluginsConfig,
) -> Result<agent::plugins::PluginActivationPlan> {
    let resolver = agent::PluginBootResolverConfig {
        enabled: plugins.enabled,
        roots: plugins
            .roots
            .iter()
            .map(|value| {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    workspace_root.join(path)
                }
            })
            .collect::<Vec<_>>(),
        include_builtin: plugins.include_builtin,
        allow: plugins.allow.clone(),
        deny: plugins.deny.clone(),
        entries: plugins.entries.clone(),
        slots: plugins.slots.clone(),
    };
    agent::build_plugin_activation_plan(workspace_root, &resolver)
}

fn inject_process_env(env_map: &EnvMap) {
    // This runs before the Tokio runtime starts, so mutating process env is safe here.
    env_map.apply_to_process();
}

#[cfg(test)]
mod tests {
    use super::build_sandbox_policy;
    use crate::options::{AppOptions, parse_bool_flag};
    use crate::provider::{SelectedProvider, default_model};
    use agent::ToolExecutionContext;
    use agent::tools::{NetworkPolicy, SandboxMode};
    use agent_env::EnvMap;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn default_model_matches_provider() {
        assert_eq!(default_model(SelectedProvider::OpenAi), "gpt-5.4");
        assert_eq!(
            default_model(SelectedProvider::Anthropic),
            "claude-sonnet-4-6"
        );
    }

    #[test]
    fn parses_boolean_flag_values() {
        assert!(parse_bool_flag("true").unwrap());
        assert!(!parse_bool_flag("off").unwrap());
        assert!(parse_bool_flag("1").unwrap());
        assert!(parse_bool_flag("maybe").is_err());
    }

    #[tokio::test]
    async fn loads_sandbox_fail_closed_from_env_and_cli() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OPENAI_API_KEY=test-key\nNANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=false\n",
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let options = AppOptions::from_env_and_args_iter(
            dir.path(),
            &env_map,
            vec![
                "--sandbox-fail-if-unavailable".to_string(),
                "true".to_string(),
            ],
        )
        .unwrap();
        let tool_context = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            worktree_root: Some(dir.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };

        let policy = build_sandbox_policy(&options, &tool_context);

        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(policy.network, NetworkPolicy::Off);
        assert!(policy.fail_if_unavailable);
    }
}
