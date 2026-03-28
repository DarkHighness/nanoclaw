use crate::backend::store::build_store;
use crate::backend::{
    build_plugin_activation_plan, build_sandbox_policy, build_system_preamble, build_tool_context,
    log_sandbox_status, resolve_skill_roots, tool_context_for_profile,
};
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
    CompactionConfig, ConversationCompactor, DefaultCommandHookExecutor, LoopDetectionConfig,
    ModelBackend, ModelConversationCompactor, NoopToolApprovalPolicy, RuntimeSubagentExecutor,
    SubagentProfileResolver, SubagentRuntimeProfile, ToolApprovalHandler,
};
use agent::tools::{
    AgentCancelTool, AgentListTool, AgentSendTool, AgentSpawnTool, AgentWaitTool, TaskBatchTool,
    describe_sandbox_policy, ensure_sandbox_policy_supported,
};
use agent::types::{AgentTaskSpec, HookRegistration};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, BashTool, CodeDefinitionsTool, CodeDocumentSymbolsTool,
    CodeIntelBackend, CodeReferencesTool, CodeSymbolSearchTool, EditTool, GlobTool, GrepTool,
    HookRunner, ListTool, ManagedCodeIntelBackend, ManagedCodeIntelOptions,
    ManagedPolicyProcessExecutor, PatchTool, ReadTool, SandboxPolicy, Skill, SkillCatalog,
    TaskTool, TodoListState, TodoReadTool, TodoWriteTool, ToolExecutionContext, ToolRegistry,
    WorkspaceTextCodeIntelBackend, WriteTool,
};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use nanoclaw_config::{CoreConfig, ResolvedAgentProfile};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

struct RuntimeBuildResult {
    runtime: AgentRuntime,
    store: Arc<dyn store::RunStore>,
    skills: Vec<Skill>,
    store_label: String,
    store_warning: Option<String>,
    stored_run_count: usize,
}

pub(crate) struct DriverHostInputs {
    pub(crate) runtime_hooks: Vec<HookRegistration>,
    pub(crate) mcp_servers: Vec<McpServerConfig>,
    pub(crate) instructions: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct CodeAgentSubagentProfileResolver {
    pub(crate) core: CoreConfig,
    pub(crate) env_map: EnvMap,
    pub(crate) base_tool_context: ToolExecutionContext,
    pub(crate) skill_catalog: SkillCatalog,
    pub(crate) plugin_instructions: Vec<String>,
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

pub(crate) fn merge_driver_host_inputs(
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

pub(crate) fn resolve_mcp_servers(
    configs: &[McpServerConfig],
    workspace_root: &Path,
) -> Vec<McpServerConfig> {
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

pub(crate) fn dedup_mcp_servers(servers: Vec<McpServerConfig>) -> Vec<McpServerConfig> {
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
pub(crate) fn driver_host_output_lines(
    driver_outcome: &agent::DriverActivationOutcome,
) -> Vec<String> {
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

pub(crate) async fn build_session(
    options: &AppOptions,
    workspace_root: &Path,
    approval_handler: Arc<dyn ToolApprovalHandler>,
) -> Result<super::CodeAgentSession> {
    let base_tool_context = build_tool_context(workspace_root, options);
    let sandbox_policy = build_sandbox_policy(options, &base_tool_context);
    let tool_context = base_tool_context.with_sandbox_policy(sandbox_policy.clone());
    let sandbox_status = ensure_sandbox_policy_supported(&sandbox_policy)
        .context("sandbox policy cannot be enforced on this host")?;
    log_sandbox_status(&sandbox_status);
    let sandbox_summary = describe_sandbox_policy(&sandbox_policy, &sandbox_status);

    let RuntimeBuildResult {
        runtime,
        store,
        skills,
        store_label,
        store_warning,
        stored_run_count,
    } = build_runtime(
        options,
        workspace_root,
        approval_handler,
        tool_context,
        sandbox_policy,
    )
    .await?;
    let tool_names = runtime.tool_registry_names();
    // Persisted history is still keyed by substrate `run_id`. Expose that ID as
    // the operator-facing session reference until the host grows a first-class
    // resumable session catalog above the raw runtime/store layer.
    let active_session_ref = runtime.run_id().to_string();
    let root_session_id = runtime.session_id().to_string();
    let skill_names = skills.iter().map(|skill| skill.name.clone()).collect();

    Ok(super::CodeAgentSession::new(
        runtime,
        store,
        super::SessionStartupSnapshot {
            workspace_name: workspace_root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("workspace")
                .to_string(),
            workspace_root: workspace_root.to_path_buf(),
            active_session_ref,
            root_session_id,
            provider_label: provider_label(&options.primary_profile),
            model: options.primary_profile.model.model.clone(),
            summary_model: provider_summary(&options.summary_profile.model),
            memory_model: provider_summary(&options.memory_profile.model),
            tool_names,
            skill_names,
            store_label,
            store_warning,
            stored_session_count: stored_run_count,
            sandbox_summary,
        },
        skills,
    ))
}

async fn build_runtime(
    options: &AppOptions,
    workspace_root: &Path,
    approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_context: ToolExecutionContext,
    sandbox_policy: SandboxPolicy,
) -> Result<RuntimeBuildResult> {
    let backend = Arc::new(build_agent_backend(
        &options.primary_profile,
        &options.env_map,
    )?);
    let summary_backend = Arc::new(build_internal_backend(
        &options.summary_profile,
        &options.env_map,
    )?);
    let store_handle = build_store(&options.core, workspace_root).await?;
    let store = store_handle.store.clone();
    let stored_run_count = match store_handle.store.list_runs().await {
        Ok(runs) => runs.len(),
        Err(error) => {
            warn!("failed to list persisted runs during startup: {error}");
            0
        }
    };
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
    // Managed LSP helpers run outside the normal user-invoked tool approval
    // path. Keep them behind explicit app-level config until background helper
    // execution shares the same approval and sandbox contract as foreground
    // tool calls.
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
    // Driver-backed plugins expand into normal local tools here so the runtime
    // and subagent surfaces stay identical regardless of whether a capability
    // came from builtin boot code or a plugin slot selection.
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

    let runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
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

    Ok(RuntimeBuildResult {
        runtime,
        store,
        skills,
        store_label: store_handle.label,
        store_warning: store_handle.warning,
        stored_run_count,
    })
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
