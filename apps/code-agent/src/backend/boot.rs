use crate::backend::boot_inputs::DriverHostInputs;
use crate::backend::boot_runtime::{build_runtime_tooling, register_subagent_tools};
use crate::backend::store::build_store;
use crate::backend::{
    build_plugin_activation_plan, build_sandbox_policy, build_system_preamble, build_tool_context,
    dedup_mcp_servers, log_sandbox_status, merge_driver_host_inputs, resolve_mcp_servers,
    resolve_skill_roots, tool_context_for_profile,
};
use crate::options::AppOptions;
use crate::provider::{
    agent_backend_capabilities, build_agent_backend, build_internal_backend,
    build_memory_reasoning_service, provider_label, provider_summary,
};
use agent::mcp::{
    McpConnectOptions, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_options,
};
use agent::runtime::{
    CompactionConfig, ConversationCompactor, ModelBackend, ModelConversationCompactor,
    NoopToolApprovalPolicy, RuntimeSubagentExecutor, SubagentProfileResolver,
    SubagentRuntimeProfile, ToolApprovalHandler,
};
use agent::tools::{describe_sandbox_policy, ensure_sandbox_policy_supported};
use agent::types::AgentTaskSpec;
use agent::{
    AgentRuntime, AgentRuntimeBuilder, SandboxPolicy, Skill, SkillCatalog, ToolExecutionContext,
    ToolRegistry,
};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use nanoclaw_config::{CoreConfig, ResolvedAgentProfile};
use std::path::Path;
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
    // Runtime tooling assembly is still host boot work, but it lives behind a
    // dedicated helper so later frontends inherit the same process-local tool,
    // hook, and LSP wiring without reopening this orchestration block.
    let runtime_tooling = build_runtime_tooling(options, workspace_root, &sandbox_policy);
    let loop_detection_config = runtime_tooling.loop_detection_config;
    let process_executor = runtime_tooling.process_executor.clone();
    let hook_runner = runtime_tooling.hook_runner.clone();
    let mut tools = runtime_tooling.tools;
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
    register_subagent_tools(&mut tools, subagent_executor);

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
