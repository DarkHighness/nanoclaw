mod backend;
mod config;
mod frontend;
mod options;
mod provider;

use crate::backend::CodeAgentSession;
use crate::frontend::tui::{CodeAgentTui, make_tui_support};
use crate::options::AppOptions;
use crate::provider::{
    agent_backend_capabilities, build_agent_backend, build_internal_backend,
    build_memory_reasoning_service, provider_label, provider_summary,
};
use agent::mcp::{
    McpConnectOptions, McpServerConfig, McpTransportConfig, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_options,
};
use agent::runtime::{
    CompactionConfig, ConversationCompactor, DefaultCommandHookExecutor, HostRuntimeLimits,
    LoopDetectionConfig, ModelBackend, ModelConversationCompactor, NoopToolApprovalPolicy,
    RuntimeSubagentExecutor, SubagentProfileResolver, SubagentRuntimeProfile, ToolApprovalHandler,
    build_host_tokio_runtime,
};
use agent::tools::{
    AgentCancelTool, AgentListTool, AgentSendTool, AgentSpawnTool, AgentWaitTool,
    SandboxBackendStatus, TaskBatchTool, ensure_sandbox_policy_supported,
};
use agent::types::{AgentTaskSpec, HookRegistration};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, AgentWorkspaceLayout, BashTool, CodeDefinitionsTool,
    CodeDocumentSymbolsTool, CodeIntelBackend, CodeReferencesTool, CodeSymbolSearchTool, EditTool,
    GlobTool, GrepTool, HookRunner, InMemoryRunStore, ListTool, ManagedCodeIntelBackend,
    ManagedCodeIntelOptions, ManagedPolicyProcessExecutor, PatchTool, ReadTool, SandboxPolicy,
    Skill, SkillCatalog, TaskTool, TodoListState, TodoReadTool, TodoWriteTool,
    ToolExecutionContext, ToolRegistry, WorkspaceTextCodeIntelBackend, WriteTool,
};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use nanoclaw_config::{AgentSandboxMode, CoreConfig, PluginsConfig, ResolvedAgentProfile};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

struct DriverHostInputs {
    runtime_hooks: Vec<HookRegistration>,
    mcp_servers: Vec<McpServerConfig>,
    instructions: Vec<String>,
}

#[derive(Clone)]
struct CodeAgentSubagentProfileResolver {
    core: CoreConfig,
    env_map: EnvMap,
    base_tool_context: ToolExecutionContext,
    skill_catalog: SkillCatalog,
    plugin_instructions: Vec<String>,
}

impl SubagentProfileResolver for CodeAgentSubagentProfileResolver {
    fn resolve_profile(
        &self,
        task: &AgentTaskSpec,
    ) -> agent::runtime::Result<SubagentRuntimeProfile> {
        let profile = self
            .core
            .resolve_subagent_profile(Some(task.role.as_str()))
            .map_err(|error| {
                agent::runtime::RuntimeError::invalid_state(format!(
                    "failed to resolve subagent profile for role `{}`: {error}",
                    task.role
                ))
            })?;
        let backend: Arc<dyn ModelBackend> = Arc::new(
            build_agent_backend(&profile, &self.env_map).map_err(|error| {
                agent::runtime::RuntimeError::invalid_state(format!(
                    "failed to build backend for subagent profile `{}`: {error}",
                    profile.profile_name
                ))
            })?,
        );
        let compactor: Arc<dyn ConversationCompactor> =
            Arc::new(ModelConversationCompactor::new(backend.clone()));
        Ok(SubagentRuntimeProfile {
            profile_name: profile.profile_name.clone(),
            backend,
            tool_context: tool_context_for_profile(&self.base_tool_context, &profile),
            conversation_compactor: compactor,
            compaction_config: CompactionConfig {
                enabled: profile.auto_compact,
                context_window_tokens: profile.context_window_tokens,
                trigger_tokens: profile.compact_trigger_tokens,
                preserve_recent_messages: profile.compact_preserve_recent_messages,
            },
            instructions: build_system_preamble(
                &profile,
                &self.skill_catalog,
                &self.plugin_instructions,
            ),
            supports_tool_calls: profile.model.capabilities.tool_calls,
        })
    }
}

fn merge_driver_host_inputs(
    runtime_hooks: Vec<HookRegistration>,
    mcp_servers: Vec<McpServerConfig>,
    instructions: Vec<String>,
    driver_outcome: &agent::DriverActivationOutcome,
) -> DriverHostInputs {
    let mut merged = DriverHostInputs {
        runtime_hooks,
        mcp_servers,
        instructions,
    };
    // Code Agent has both declarative plugin contributions and runtime driver
    // output. Merge them once here so the foreground runtime, subagents, and
    // MCP bootstrap all see the same effective startup inputs.
    driver_outcome.extend_host_inputs(
        &mut merged.runtime_hooks,
        &mut merged.mcp_servers,
        &mut merged.instructions,
    );
    merged
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

fn dedup_mcp_servers(servers: Vec<McpServerConfig>) -> Vec<McpServerConfig> {
    let mut by_name = BTreeMap::new();
    for server in servers {
        by_name.entry(server.name.clone()).or_insert(server);
    }
    by_name.into_values().collect()
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
fn driver_host_output_lines(driver_outcome: &agent::DriverActivationOutcome) -> Vec<String> {
    driver_outcome
        .host_messages()
        .map(|message| match message.level {
            agent::DriverHostMessageLevel::Warning => {
                format!("warning: plugin driver warning: {}", message.message)
            }
            agent::DriverHostMessageLevel::Diagnostic => {
                format!("info: plugin driver diagnostic: {}", message.message)
            }
        })
        .collect()
}

fn main() -> Result<()> {
    let workspace_root = env::current_dir().context("failed to resolve current workspace")?;
    let env_map = EnvMap::from_workspace_dir(&workspace_root)?;
    inject_process_env(&env_map);
    let _tracing_guard = init_tracing(&workspace_root)?;
    let options = AppOptions::from_env_and_args(&workspace_root, &env_map)?;

    let runtime = build_host_tokio_runtime(HostRuntimeLimits {
        worker_threads: options.tokio_worker_threads,
        max_blocking_threads: options.tokio_max_blocking_threads,
    })
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
    let base_tool_context = build_tool_context(&workspace_root, &options);
    let sandbox_policy = build_sandbox_policy(&options, &base_tool_context);
    let tool_context = base_tool_context.with_sandbox_policy(sandbox_policy.clone());
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
    let provider_label = provider_label(&options.primary_profile);
    let model = options.primary_profile.model.model.clone();
    let summary_model = provider_summary(&options.summary_profile.model);
    let memory_model = provider_summary(&options.memory_profile.model);
    let initial_prompt = options.one_shot_prompt.clone();
    let session = CodeAgentSession::new(
        runtime,
        workspace_root.clone(),
        provider_label,
        model,
        summary_model,
        memory_model,
        skills,
    );
    CodeAgentTui::new(session, initial_prompt, ui_state, approval_bridge)
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
    let backend = Arc::new(build_agent_backend(
        &options.primary_profile,
        &options.env_map,
    )?);
    let summary_backend = Arc::new(build_internal_backend(
        &options.summary_profile,
        &options.env_map,
    )?);
    let store = Arc::new(InMemoryRunStore::new());
    let plugin_plan = build_plugin_activation_plan(workspace_root, &options.plugins)
        .context("failed to build plugin activation plan")?;
    let skill_roots = resolve_skill_roots(&options.skill_roots, workspace_root, &plugin_plan);
    let skill_catalog = agent::skills::load_skill_roots(&skill_roots)
        .await
        .context("failed to load skill roots")?;
    let skills = skill_catalog.all().to_vec();
    let runtime_hooks = plugin_plan.hooks.clone();
    let plugin_mcp_servers = plugin_plan.mcp_servers.clone();
    let plugin_instructions = plugin_plan.instructions.clone();
    let compactor = Arc::new(ModelConversationCompactor::new(summary_backend));
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
        Arc::new(agent::runtime::FailClosedPromptHookEvaluator),
        Arc::new(agent::runtime::FailClosedAgentHookEvaluator),
        Arc::new(agent::runtime::DefaultWasmHookExecutor::default()),
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
        process_executor.clone(),
        sandbox_policy.clone(),
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
    let driver_outcome = agent::activate_driver_requests(
        &plugin_plan.runtime_activations,
        workspace_root,
        Some(store.clone()),
        Some(build_memory_reasoning_service(
            &options.memory_profile,
            &options.env_map,
        )),
        &mut tools,
        agent::UnknownDriverPolicy::Error,
    )?;
    for message in driver_outcome.host_messages() {
        match message.level {
            agent::DriverHostMessageLevel::Warning => {
                let line = format!("warning: plugin driver warning: {}", message.message);
                warn!("{line}");
                eprintln!("{line}");
            }
            agent::DriverHostMessageLevel::Diagnostic => {
                let line = format!("info: plugin driver diagnostic: {}", message.message);
                info!("{line}");
                eprintln!("{line}");
            }
        }
    }
    let DriverHostInputs {
        mut runtime_hooks,
        mcp_servers,
        instructions: plugin_instructions,
    } = merge_driver_host_inputs(
        runtime_hooks,
        plugin_mcp_servers,
        plugin_instructions,
        &driver_outcome,
    );
    if !mcp_servers.is_empty() {
        let resolved_mcp_servers =
            dedup_mcp_servers(resolve_mcp_servers(&mcp_servers, workspace_root));
        let connected = connect_and_catalog_mcp_servers_with_options(
            &resolved_mcp_servers,
            McpConnectOptions {
                process_executor: process_executor.clone(),
                sandbox_policy: sandbox_policy.clone(),
                ..Default::default()
            },
        )
        .await
        .context("failed to connect plugin MCP servers")?;
        for server in &connected {
            for adapter in catalog_tools_as_registry_entries(server.client.clone())
                .await
                .context("failed to register plugin MCP tools")?
            {
                tools.register(adapter);
            }
        }
    }
    ensure_model_supports_registered_tools(
        &options.primary_profile,
        agent_backend_capabilities(&options.primary_profile),
        &tools,
        "primary",
    )?;
    let skill_hooks = skills
        .iter()
        .flat_map(|skill| skill.hooks.clone())
        .collect::<Vec<_>>();
    runtime_hooks.extend(skill_hooks.clone());
    let instructions = build_system_preamble(
        &options.primary_profile,
        &skill_catalog,
        &plugin_instructions,
    );
    let subagent_profile_resolver = Arc::new(CodeAgentSubagentProfileResolver {
        core: options.core.clone(),
        env_map: options.env_map.clone(),
        base_tool_context: tool_context.clone(),
        skill_catalog: skill_catalog.clone(),
        plugin_instructions: plugin_instructions.clone(),
    });
    let subagent_executor = RuntimeSubagentExecutor::new(
        hook_runner.clone(),
        store.clone(),
        tools.clone(),
        tool_context.clone(),
        approval_handler.clone(),
        Arc::new(NoopToolApprovalPolicy),
        loop_detection_config.clone(),
        runtime_hooks.clone(),
        skill_catalog.clone(),
        subagent_profile_resolver,
    );
    let subagent_executor = Arc::new(subagent_executor);
    tools.register(TaskTool::new(subagent_executor.clone()));
    tools.register(TaskBatchTool::new(subagent_executor.clone()));
    tools.register(AgentSpawnTool::new(subagent_executor.clone()));
    tools.register(AgentSendTool::new(subagent_executor.clone()));
    tools.register(AgentWaitTool::new(subagent_executor.clone()));
    tools.register(AgentListTool::new(subagent_executor.clone()));
    tools.register(AgentCancelTool::new(subagent_executor.clone()));

    let runtime = AgentRuntimeBuilder::new(backend.clone(), store)
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(tool_context)
        .tool_approval_handler(approval_handler)
        .conversation_compactor(compactor)
        .compaction_config(CompactionConfig {
            enabled: options.primary_profile.auto_compact,
            context_window_tokens: options.primary_profile.context_window_tokens,
            trigger_tokens: options.primary_profile.compact_trigger_tokens,
            preserve_recent_messages: options.primary_profile.compact_preserve_recent_messages,
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
    let base_policy = tool_context.sandbox_scope().recommended_policy();
    match options.primary_profile.sandbox {
        AgentSandboxMode::DangerFullAccess => SandboxPolicy::permissive()
            .with_fail_if_unavailable(options.sandbox_fail_if_unavailable),
        AgentSandboxMode::WorkspaceWrite => {
            base_policy.with_fail_if_unavailable(options.sandbox_fail_if_unavailable)
        }
        AgentSandboxMode::ReadOnly => SandboxPolicy {
            mode: agent::tools::SandboxMode::ReadOnly,
            filesystem: agent::tools::FilesystemPolicy {
                readable_roots: base_policy.filesystem.readable_roots,
                writable_roots: Vec::new(),
                executable_roots: base_policy.filesystem.executable_roots,
                protected_paths: base_policy.filesystem.protected_paths,
            },
            network: match base_policy.network {
                agent::tools::NetworkPolicy::Full => agent::tools::NetworkPolicy::Off,
                other => other,
            },
            host_escape: agent::tools::HostEscapePolicy::Deny,
            fail_if_unavailable: options.sandbox_fail_if_unavailable,
        },
    }
}

fn ensure_model_supports_registered_tools(
    profile: &ResolvedAgentProfile,
    capabilities: agent::runtime::ModelBackendCapabilities,
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

fn build_tool_context(workspace_root: &Path, options: &AppOptions) -> ToolExecutionContext {
    ToolExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        worktree_root: Some(workspace_root.to_path_buf()),
        workspace_only: options.workspace_only,
        model_context_window_tokens: Some(options.primary_profile.context_window_tokens),
        ..Default::default()
    }
}

fn tool_context_for_profile(
    base: &ToolExecutionContext,
    profile: &ResolvedAgentProfile,
) -> ToolExecutionContext {
    let mut context = base.clone();
    context.model_context_window_tokens = Some(profile.context_window_tokens);
    let base_policy = base.sandbox_policy();
    match profile.sandbox {
        AgentSandboxMode::DangerFullAccess => {
            context.workspace_only = false;
            context.read_only_roots.clear();
            context.writable_roots.clear();
            context.exec_roots.clear();
            context.network_policy = Some(agent::tools::NetworkPolicy::Full);
            context.effective_sandbox_policy = Some(
                agent::tools::SandboxPolicy::permissive()
                    .with_fail_if_unavailable(base_policy.fail_if_unavailable),
            );
        }
        AgentSandboxMode::WorkspaceWrite => {
            context.workspace_only = true;
            context.effective_sandbox_policy = Some(
                context
                    .sandbox_scope()
                    .recommended_policy()
                    .with_fail_if_unavailable(base_policy.fail_if_unavailable),
            );
        }
        AgentSandboxMode::ReadOnly => {
            context.workspace_only = true;
            context.read_only_roots = profile_read_only_roots(base);
            context.writable_roots.clear();
            context.network_policy = Some(
                match base
                    .network_policy
                    .clone()
                    .unwrap_or(agent::tools::NetworkPolicy::Off)
                {
                    agent::tools::NetworkPolicy::Full => agent::tools::NetworkPolicy::Off,
                    other => other,
                },
            );
            let derived = context
                .sandbox_scope()
                .recommended_policy()
                .with_fail_if_unavailable(base_policy.fail_if_unavailable);
            context.effective_sandbox_policy = Some(agent::tools::SandboxPolicy {
                mode: agent::tools::SandboxMode::ReadOnly,
                filesystem: agent::tools::FilesystemPolicy {
                    readable_roots: derived.filesystem.readable_roots,
                    writable_roots: Vec::new(),
                    executable_roots: derived.filesystem.executable_roots,
                    protected_paths: derived.filesystem.protected_paths,
                },
                network: derived.network,
                host_escape: agent::tools::HostEscapePolicy::Deny,
                fail_if_unavailable: derived.fail_if_unavailable,
            });
        }
    }
    context
}

fn profile_read_only_roots(base: &ToolExecutionContext) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    roots.insert(base.effective_root().to_path_buf());
    if let Some(worktree_root) = base.worktree_root.clone() {
        roots.insert(worktree_root);
    }
    roots.extend(base.additional_roots.iter().cloned());
    roots.extend(base.read_only_roots.iter().cloned());
    roots.extend(base.writable_roots.iter().cloned());
    roots.extend(base.exec_roots.iter().cloned());
    roots.into_iter().collect()
}

fn build_system_preamble(
    profile: &ResolvedAgentProfile,
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
    for prompt in [
        profile.global_system_prompt.as_deref(),
        profile.system_prompt.as_deref(),
    ] {
        if let Some(system_prompt) = prompt.map(str::trim).filter(|value| !value.is_empty()) {
            preamble.push(system_prompt.to_string());
        }
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
    use super::{
        CodeAgentSubagentProfileResolver, build_sandbox_policy, dedup_mcp_servers,
        driver_host_output_lines, merge_driver_host_inputs, resolve_mcp_servers,
        tool_context_for_profile,
    };
    use crate::options::{AppOptions, parse_bool_flag};
    use agent::DriverActivationOutcome;
    use agent::ToolExecutionContext;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent::runtime::SubagentProfileResolver;
    use agent::tools::{NetworkPolicy, SandboxMode};
    use agent::types::{HookEvent, HookHandler, HookRegistration, HttpHookHandler};
    use agent_env::EnvMap;
    use nanoclaw_config::{
        AgentProfileConfig, AgentSandboxMode, CoreConfig, ModelCapabilitiesConfig, ModelConfig,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn parses_boolean_flag_values() {
        assert!(parse_bool_flag("true").unwrap());
        assert!(!parse_bool_flag("off").unwrap());
        assert!(parse_bool_flag("1").unwrap());
        assert!(parse_bool_flag("maybe").is_err());
    }

    #[test]
    fn driver_outcome_extends_code_agent_runtime_inputs() {
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
                    timeout_ms: Some(500),
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
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp", "driver-mcp"]
        );
        assert_eq!(
            merged.instructions,
            vec![
                "existing instruction".to_string(),
                "driver instruction".to_string()
            ]
        );
    }

    #[test]
    fn tool_context_for_read_only_profile_promotes_accessible_roots_and_disables_full_network() {
        let profile = CoreConfig::default()
            .with_override(|config| {
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            })
            .resolve_subagent_profile(Some("reviewer"))
            .unwrap();
        let context = tool_context_for_profile(
            &ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/worktree")),
                additional_roots: vec![PathBuf::from("/refs")],
                writable_roots: vec![PathBuf::from("/workspace/tmp")],
                exec_roots: vec![PathBuf::from("/workspace/bin")],
                network_policy: Some(NetworkPolicy::Full),
                workspace_only: false,
                ..Default::default()
            },
            &profile,
        );

        assert!(context.workspace_only);
        assert!(context.writable_roots.is_empty());
        assert_eq!(context.network_policy, Some(NetworkPolicy::Off));
        assert_eq!(
            context.read_only_roots,
            vec![
                PathBuf::from("/refs"),
                PathBuf::from("/workspace"),
                PathBuf::from("/workspace/bin"),
                PathBuf::from("/workspace/tmp"),
                PathBuf::from("/worktree"),
            ]
        );
    }

    #[test]
    fn subagent_profile_resolver_routes_role_profiles_and_honors_tool_capability() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let resolver = CodeAgentSubagentProfileResolver {
            core: CoreConfig::default().with_override(|config| {
                let base_model = config.models["gpt_5_4_default"].clone();
                config.models.insert(
                    "reviewer_no_tools".to_string(),
                    ModelConfig {
                        capabilities: ModelCapabilitiesConfig {
                            tool_calls: false,
                            ..base_model.capabilities.clone()
                        },
                        ..base_model
                    },
                );
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        model: Some("reviewer_no_tools".to_string()),
                        system_prompt: Some("Review only".to_string()),
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            }),
            env_map: EnvMap::from_workspace_dir(dir.path()).unwrap(),
            base_tool_context: ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/workspace")),
                workspace_only: true,
                ..Default::default()
            },
            skill_catalog: agent::SkillCatalog::default(),
            plugin_instructions: vec!["Plugin instruction".to_string()],
        };

        let profile = resolver
            .resolve_profile(&agent::types::AgentTaskSpec {
                task_id: "review".to_string(),
                role: "reviewer".to_string(),
                prompt: "review".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            })
            .unwrap();

        assert_eq!(profile.profile_name, "roles.reviewer");
        assert!(!profile.supports_tool_calls);
        assert!(profile.instructions.join("\n").contains("Review only"));
        assert_eq!(
            profile.tool_context.model_context_window_tokens,
            Some(400_000)
        );
        assert_eq!(
            profile.tool_context.network_policy,
            Some(NetworkPolicy::Off)
        );
    }

    #[test]
    fn empty_driver_outcome_keeps_code_agent_runtime_inputs_stable() {
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
            &DriverActivationOutcome::default(),
        );

        assert_eq!(
            merged
                .runtime_hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook"]
        );
        assert_eq!(
            merged
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp"]
        );
        assert_eq!(
            merged.instructions,
            vec!["existing instruction".to_string()]
        );
    }

    #[test]
    fn driver_diagnostics_are_rendered_for_host_output() {
        let lines = driver_host_output_lines(&DriverActivationOutcome {
            warnings: vec!["slow startup".to_string()],
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
            instructions: Vec::new(),
            diagnostics: vec!["validated wasm hook module".to_string()],
        });

        assert_eq!(
            lines,
            vec![
                "warning: plugin driver warning: slow startup".to_string(),
                "info: plugin driver diagnostic: validated wasm hook module".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_and_dedup_plugin_mcp_servers_matches_host_boot_expectations() {
        let dir = tempdir().unwrap();
        let resolved = dedup_mcp_servers(resolve_mcp_servers(
            &[
                McpServerConfig {
                    name: "dup".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: "first".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: Some("relative".to_string()),
                    },
                },
                McpServerConfig {
                    name: "dup".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: "second".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: Some("ignored".to_string()),
                    },
                },
            ],
            dir.path(),
        ));

        assert_eq!(resolved.len(), 1);
        match &resolved[0].transport {
            McpTransportConfig::Stdio { command, cwd, .. } => {
                let expected_cwd = dir.path().join("relative");
                assert_eq!(command, "first");
                assert_eq!(
                    cwd.as_deref(),
                    Some(expected_cwd.to_string_lossy().as_ref())
                );
            }
            McpTransportConfig::StreamableHttp { .. } => {
                panic!("expected stdio transport");
            }
        }
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
