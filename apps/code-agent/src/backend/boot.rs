use crate::backend::boot_inputs::DriverHostInputs;
use crate::backend::boot_mcp::build_startup_diagnostics_snapshot;
use crate::backend::boot_runtime::{build_runtime_tooling, register_subagent_tools};
use crate::backend::memory_recall::WorkspaceMemoryRecallAugmentor;
use crate::backend::store::build_store;
use crate::backend::{
    ApprovalCoordinator, NonInteractivePermissionRequestHandler, NonInteractiveToolApprovalHandler,
    NonInteractiveUserInputHandler, PermissionRequestCoordinator, SessionEventStream,
    SessionPermissionRequestHandler, SessionToolApprovalHandler, SessionUserInputHandler,
    UserInputCoordinator, build_plugin_activation_plan, build_sandbox_policy,
    build_system_preamble, build_tool_context, dedup_mcp_servers, log_sandbox_status,
    merge_driver_host_inputs, resolve_mcp_servers, resolve_skill_roots, tool_context_for_profile,
};
use crate::options::AppOptions;
use crate::provider::{
    MutableAgentBackend, agent_backend_capabilities, build_agent_backend, build_internal_backend,
    build_memory_reasoning_service, build_mutable_agent_backend, provider_label, provider_name,
};
use agent::mcp::{
    ConnectedMcpServer, McpConnectOptions, McpServerConfig, McpTransportConfig,
    catalog_resource_tools_as_registry_entries, catalog_tools_as_registry_entries,
    connect_and_catalog_mcp_servers_with_options,
};
use agent::runtime::{
    CompactionConfig, ConversationCompactor, ModelBackend, ModelConversationCompactor,
    NoopToolApprovalPolicy, PermissionGrantStore, RuntimeSubagentExecutor, SubagentProfileResolver,
    SubagentRuntimeProfile, ToolApprovalHandler,
};
use agent::tools::{
    SubagentExecutor, SubagentLaunchSpec, describe_sandbox_policy, ensure_sandbox_policy_supported,
};
use agent::types::{HookHandler, HookRegistration};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, SandboxPolicy, Skill, SkillCatalog, ToolExecutionContext,
    ToolRegistry,
};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use nanoclaw_config::{CoreConfig, ResolvedAgentProfile};
use std::path::Path;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

struct RuntimeBuildResult {
    runtime: AgentRuntime,
    model_backend: MutableAgentBackend,
    memory_backend: Option<Arc<dyn agent::memory::MemoryBackend>>,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn store::SessionStore>,
    skill_catalog: SkillCatalog,
    skills: Vec<Skill>,
    plugin_instructions: Vec<String>,
    mcp_servers: Vec<ConnectedMcpServer>,
    host_process_surfaces_allowed: bool,
    store_label: String,
    store_warning: Option<String>,
    stored_session_count: usize,
    startup_diagnostics: crate::backend::StartupDiagnosticsSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionApprovalMode {
    Interactive,
    NonInteractive,
}

#[derive(Clone)]
pub(crate) struct CodeAgentSubagentProfileResolver {
    pub(crate) core: CoreConfig,
    pub(crate) env_map: EnvMap,
    pub(crate) base_tool_context: Arc<RwLock<ToolExecutionContext>>,
    pub(crate) skill_catalog: SkillCatalog,
    pub(crate) plugin_instructions: Vec<String>,
}

impl CodeAgentSubagentProfileResolver {
    pub(crate) fn resolve_agent_profile(
        &self,
        launch: &SubagentLaunchSpec,
    ) -> agent::runtime::Result<ResolvedAgentProfile> {
        let task = &launch.task;
        let mut profile = self
            .core
            .resolve_subagent_profile(Some(task.role.as_str()))
            .map_err(|error| {
                agent::runtime::RuntimeError::invalid_state(format!(
                    "failed to resolve subagent profile for role `{}`: {error}",
                    task.role
                ))
            })?;
        if let Some(model_alias) = launch.model.as_deref() {
            // Launch-time model overrides should reuse the existing config
            // resolver so provider capabilities and validation stay centralized.
            profile.model = self.core.resolve_model(model_alias).map_err(|error| {
                agent::runtime::RuntimeError::invalid_state(format!(
                    "failed to resolve subagent model override `{model_alias}`: {error}",
                ))
            })?;
        }
        if let Some(reasoning_effort) = launch.reasoning_effort.as_deref() {
            profile.reasoning_effort = Some(reasoning_effort.to_string());
        }
        Ok(profile)
    }
}

impl SubagentProfileResolver for CodeAgentSubagentProfileResolver {
    fn resolve_profile(
        &self,
        launch: &SubagentLaunchSpec,
    ) -> agent::runtime::Result<SubagentRuntimeProfile> {
        let base_tool_context = self.base_tool_context.read().unwrap().clone();
        let profile = self.resolve_agent_profile(launch)?;
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
            tool_context: tool_context_for_profile(&base_tool_context, &profile),
            conversation_compactor: compactor,
            compaction_config: CompactionConfig {
                enabled: profile.auto_compact,
                context_window_tokens: profile.context_window_tokens,
                trigger_tokens: profile.compact_trigger_tokens,
                preserve_recent_messages: profile.compact_preserve_recent_messages,
            },
            instructions: build_system_preamble(
                base_tool_context.workspace_root.as_path(),
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
) -> Result<super::CodeAgentSession> {
    build_session_with_approval_mode(options, workspace_root, SessionApprovalMode::Interactive)
        .await
}

pub(crate) async fn build_session_with_approval_mode(
    options: &AppOptions,
    workspace_root: &Path,
    approval_mode: SessionApprovalMode,
) -> Result<super::CodeAgentSession> {
    let approvals = ApprovalCoordinator::default();
    let user_inputs = UserInputCoordinator::default();
    let permission_requests = PermissionRequestCoordinator::default();
    let permission_grants = PermissionGrantStore::default();
    let events = SessionEventStream::default();
    let approval_handler: Arc<dyn ToolApprovalHandler> = match approval_mode {
        SessionApprovalMode::Interactive => {
            Arc::new(SessionToolApprovalHandler::new(approvals.clone()))
        }
        SessionApprovalMode::NonInteractive => Arc::new(NonInteractiveToolApprovalHandler::new(
            "non-interactive one-shot mode cannot resolve tool approvals",
        )),
    };
    let user_input_handler: Arc<dyn agent::tools::UserInputHandler> = match approval_mode {
        SessionApprovalMode::Interactive => {
            Arc::new(SessionUserInputHandler::new(user_inputs.clone()))
        }
        SessionApprovalMode::NonInteractive => Arc::new(NonInteractiveUserInputHandler::new(
            "non-interactive one-shot mode cannot request user input",
        )),
    };
    let permission_request_handler: Arc<dyn agent::tools::PermissionRequestHandler> =
        match approval_mode {
            SessionApprovalMode::Interactive => Arc::new(SessionPermissionRequestHandler::new(
                permission_requests.clone(),
                permission_grants.clone(),
            )),
            SessionApprovalMode::NonInteractive => {
                Arc::new(NonInteractivePermissionRequestHandler::new(
                    "non-interactive one-shot mode cannot request additional permissions",
                ))
            }
        };
    let mut base_tool_context = build_tool_context(workspace_root, options);
    base_tool_context.user_input_handler = Some(user_input_handler);
    base_tool_context.permission_request_handler = Some(permission_request_handler);
    let sandbox_policy = build_sandbox_policy(options, &base_tool_context);
    let tool_context = base_tool_context.with_sandbox_policy(sandbox_policy.clone());
    let session_tool_context = Arc::new(RwLock::new(tool_context.clone()));
    let sandbox_status = ensure_sandbox_policy_supported(&sandbox_policy)
        .context("sandbox policy cannot be enforced on this host")?;
    log_sandbox_status(&sandbox_status);
    let sandbox_summary = describe_sandbox_policy(&sandbox_policy, &sandbox_status);

    let RuntimeBuildResult {
        runtime,
        model_backend,
        memory_backend,
        subagent_executor,
        store,
        skill_catalog,
        skills,
        plugin_instructions,
        mcp_servers,
        host_process_surfaces_allowed,
        store_label,
        store_warning,
        stored_session_count,
        startup_diagnostics,
    } = build_runtime(
        options,
        workspace_root,
        approval_handler,
        tool_context,
        session_tool_context.clone(),
        sandbox_policy.clone(),
        sandbox_status,
        permission_grants.clone(),
    )
    .await?;
    let tool_names = runtime.tool_registry_names();
    let supported_model_reasoning_efforts = model_backend.supported_reasoning_efforts();
    let backend_capabilities = agent_backend_capabilities(&options.primary_profile);
    // Persisted history is still keyed by substrate `session_id`. Expose that ID as
    // the operator-facing session reference until the host grows a first-class
    // resumable session catalog above the raw runtime/store layer.
    let active_session_ref = runtime.session_id().to_string();
    let root_agent_session_id = runtime.agent_session_id().to_string();

    Ok(super::CodeAgentSession::new(
        runtime,
        Some(model_backend),
        subagent_executor,
        store,
        mcp_servers,
        approvals,
        user_inputs,
        permission_requests,
        events,
        permission_grants,
        session_tool_context,
        sandbox_policy.clone(),
        super::SessionStartupSnapshot {
            workspace_name: workspace_root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("workspace")
                .to_string(),
            workspace_root: workspace_root.to_path_buf(),
            active_session_ref,
            root_agent_session_id,
            provider_label: provider_label(&options.primary_profile),
            model: options.primary_profile.model.model.clone(),
            model_reasoning_effort: options.primary_profile.reasoning_effort.clone(),
            supported_model_reasoning_efforts,
            supports_image_input: backend_capabilities.vision,
            tool_names,
            store_label,
            store_warning,
            stored_session_count: stored_session_count,
            default_sandbox_summary: sandbox_summary.clone(),
            sandbox_summary,
            permission_mode: super::SessionPermissionMode::Default,
            host_process_surfaces_allowed,
            startup_diagnostics,
            statusline: options.statusline.clone(),
        },
        options.primary_profile.clone(),
        skill_catalog,
        plugin_instructions,
        skills,
        memory_backend,
    ))
}

async fn build_runtime(
    options: &AppOptions,
    workspace_root: &Path,
    approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_context: ToolExecutionContext,
    session_tool_context: Arc<RwLock<ToolExecutionContext>>,
    sandbox_policy: SandboxPolicy,
    sandbox_status: agent::tools::SandboxBackendStatus,
    permission_grants: PermissionGrantStore,
) -> Result<RuntimeBuildResult> {
    let model_backend = build_mutable_agent_backend(&options.primary_profile, &options.env_map)?;
    let backend: Arc<dyn ModelBackend> = Arc::new(model_backend.clone());
    let summary_backend = Arc::new(build_internal_backend(
        &options.summary_profile,
        &options.env_map,
    )?);
    let store_handle = build_store(&options.core, workspace_root).await?;
    let store = store_handle.store.clone();
    let stored_session_count = match store_handle.store.list_sessions().await {
        Ok(sessions) => sessions.len(),
        Err(error) => {
            warn!("failed to list persisted sessions during startup: {error}");
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
    let runtime_tooling =
        build_runtime_tooling(options, workspace_root, &sandbox_policy, &sandbox_status);
    let loop_detection_config = runtime_tooling.loop_detection_config;
    let process_executor = runtime_tooling.process_executor.clone();
    let host_process_surfaces_allowed = runtime_tooling.host_process_surfaces_allowed;
    let mut startup_warnings = runtime_tooling.startup_warnings.clone();
    let hook_runner = runtime_tooling.hook_runner.clone();
    let mut tools = runtime_tooling.tools;
    let custom_tool_executor: Option<Arc<dyn agent::tools::ProcessExecutor>> =
        host_process_surfaces_allowed.then(|| process_executor.clone() as Arc<_>);
    let custom_tool_outcome = agent::register_workspace_custom_tools(
        workspace_root,
        custom_tool_executor.clone(),
        &tools,
    )?;
    if !custom_tool_outcome.loaded_tools.is_empty() {
        info!(
            tools = ?custom_tool_outcome.loaded_tools,
            "registered workspace custom tools"
        );
    }
    startup_warnings.extend(custom_tool_outcome.warnings.clone());
    let plugin_custom_tool_outcome = agent::register_plugin_custom_tools(
        &plugin_plan.custom_tool_activations,
        custom_tool_executor,
        &tools,
    )?;
    if !plugin_custom_tool_outcome.loaded_tools.is_empty() {
        info!(
            tools = ?plugin_custom_tool_outcome.loaded_tools,
            "registered plugin custom tools"
        );
    }
    startup_warnings.extend(plugin_custom_tool_outcome.warnings.clone());
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
    let mcp_servers = filter_boot_mcp_servers(
        mcp_servers,
        host_process_surfaces_allowed,
        &mut startup_warnings,
    );
    let mut connected_mcp_servers = Vec::new();
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
        for resource_tool in catalog_resource_tools_as_registry_entries(connected.clone()) {
            tools.register(resource_tool);
        }
        connected_mcp_servers = connected;
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
    runtime_hooks.extend(skill_hooks);
    runtime_hooks = filter_runtime_hooks(
        runtime_hooks,
        host_process_surfaces_allowed,
        &mut startup_warnings,
    );
    let instructions = build_system_preamble(
        workspace_root,
        &options.primary_profile,
        &skill_catalog,
        &plugin_instructions,
    );
    let subagent_profile_resolver = Arc::new(CodeAgentSubagentProfileResolver {
        core: options.core.clone(),
        env_map: options.env_map.clone(),
        base_tool_context: session_tool_context,
        skill_catalog: skill_catalog.clone(),
        plugin_instructions: plugin_instructions.clone(),
    });
    let subagent_executor: Arc<dyn SubagentExecutor> = Arc::new(RuntimeSubagentExecutor::new(
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
    ));
    register_subagent_tools(&mut tools, subagent_executor.clone());
    let provider_name = provider_name(&options.primary_profile.model.provider);
    let tool_specs = tools
        .specs()
        .into_iter()
        .filter(|spec| spec.is_model_visible_for_provider(provider_name))
        .collect::<Vec<_>>();
    let startup_diagnostics = build_startup_diagnostics_snapshot(
        workspace_root,
        &tool_specs,
        &connected_mcp_servers,
        &plugin_plan,
        &startup_warnings,
        &driver_outcome,
    );
    let memory_backend = driver_outcome.primary_memory_backend.clone();
    let runtime_builder = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(tool_context)
        .tool_approval_handler(approval_handler)
        .permission_grants(permission_grants)
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
        .skill_catalog(skill_catalog.clone());
    let runtime = if let Some(memory_backend) = memory_backend.clone() {
        runtime_builder
            .user_message_augmentor(Arc::new(WorkspaceMemoryRecallAugmentor::new(
                memory_backend,
            )))
            .build()
    } else {
        runtime_builder.build()
    };

    Ok(RuntimeBuildResult {
        runtime,
        model_backend,
        memory_backend,
        subagent_executor,
        store,
        skills,
        skill_catalog,
        plugin_instructions,
        mcp_servers: connected_mcp_servers,
        host_process_surfaces_allowed,
        store_label: store_handle.label,
        store_warning: store_handle.warning,
        stored_session_count,
        startup_diagnostics,
    })
}

fn filter_runtime_hooks(
    hooks: Vec<HookRegistration>,
    host_process_surfaces_allowed: bool,
    startup_warnings: &mut Vec<String>,
) -> Vec<HookRegistration> {
    if host_process_surfaces_allowed {
        return hooks;
    }

    let (retained, blocked): (Vec<_>, Vec<_>) = hooks
        .into_iter()
        .partition(|hook| !matches!(hook.handler, HookHandler::Command(_)));
    if !blocked.is_empty() {
        let warning = format!(
            "sandbox backend unavailable; disabled command hooks to avoid host subprocess execution: {}",
            blocked
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        warn!("{warning}");
        startup_warnings.push(warning);
    }
    retained
}

fn filter_boot_mcp_servers(
    servers: Vec<McpServerConfig>,
    host_process_surfaces_allowed: bool,
    startup_warnings: &mut Vec<String>,
) -> Vec<McpServerConfig> {
    if host_process_surfaces_allowed {
        return servers;
    }

    let (retained, blocked): (Vec<_>, Vec<_>) = servers
        .into_iter()
        .partition(|server| !matches!(server.transport, McpTransportConfig::Stdio { .. }));
    if !blocked.is_empty() {
        let warning = format!(
            "sandbox backend unavailable; skipped stdio MCP servers to avoid host subprocess execution: {}",
            blocked
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        warn!("{warning}");
        startup_warnings.push(warning);
    }
    retained
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

#[cfg(test)]
mod tests {
    use super::{filter_boot_mcp_servers, filter_runtime_hooks};
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent::types::{
        CommandHookHandler, HookEvent, HookHandler, HookRegistration, HttpHookHandler,
    };
    use std::collections::BTreeMap;

    #[test]
    fn filtering_runtime_hooks_drops_command_hooks_without_host_process_surfaces() {
        let retained = filter_runtime_hooks(
            vec![
                HookRegistration {
                    name: "command-hook".into(),
                    event: HookEvent::SessionStart,
                    matcher: None,
                    handler: HookHandler::Command(CommandHookHandler {
                        command: "/bin/true".to_string(),
                        asynchronous: false,
                    }),
                    timeout_ms: None,
                    execution: None,
                },
                HookRegistration {
                    name: "http-hook".into(),
                    event: HookEvent::SessionStart,
                    matcher: None,
                    handler: HookHandler::Http(HttpHookHandler {
                        url: "https://example.test/hook".to_string(),
                        method: "POST".to_string(),
                        headers: BTreeMap::new(),
                    }),
                    timeout_ms: None,
                    execution: None,
                },
            ],
            false,
            &mut Vec::new(),
        );

        assert_eq!(retained.len(), 1);
        assert!(matches!(retained[0].handler, HookHandler::Http(_)));
    }

    #[test]
    fn filtering_boot_mcp_servers_keeps_http_transports_when_host_processes_are_disabled() {
        let retained = filter_boot_mcp_servers(
            vec![
                McpServerConfig {
                    name: "stdio".into(),
                    transport: McpTransportConfig::Stdio {
                        command: "stdio-server".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: None,
                    },
                },
                McpServerConfig {
                    name: "http".into(),
                    transport: McpTransportConfig::StreamableHttp {
                        url: "https://example.test/mcp".to_string(),
                        headers: BTreeMap::new(),
                    },
                },
            ],
            false,
            &mut Vec::new(),
        );

        assert_eq!(retained.len(), 1);
        assert!(matches!(
            retained[0].transport,
            McpTransportConfig::StreamableHttp { .. }
        ));
    }
}
