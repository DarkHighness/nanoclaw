#[cfg(feature = "browser-tools")]
use crate::backend::SessionBrowserManager;
#[cfg(feature = "automation-tools")]
use crate::backend::SessionCronManager;
use crate::backend::boot_inputs::DriverHostInputs;
use crate::backend::boot_mcp::{
    build_startup_diagnostics_snapshot, connect_and_prepare_mcp_servers,
    mcp_connection_sandbox_policy, resolve_mcp_tool_conflicts, summarize_mcp_servers,
};
#[cfg(feature = "automation-tools")]
use crate::backend::boot_runtime::register_automation_tools;
#[cfg(feature = "browser-tools")]
use crate::backend::boot_runtime::register_browser_tools;
use crate::backend::boot_runtime::{
    COMMAND_HOOK_DISABLED_WARNING_PREFIX, SwitchableCodeIntelBackend,
    SwitchableCommandHookExecutor, SwitchableHostProcessExecutor, build_runtime_tooling,
    host_process_surfaces_allowed, register_monitor_tools, register_subagent_tools,
    register_worktree_tools,
};
use crate::backend::memory_recall::WorkspaceMemoryRecallAugmentor;
use crate::backend::session_memory_compaction::{
    SessionMemoryConversationCompactor, SessionMemoryRefreshState, SharedSessionMemoryRefreshState,
};
use crate::backend::store::build_store;
use crate::backend::{
    ApprovalCoordinator, NonInteractivePermissionRequestHandler, NonInteractiveToolApprovalHandler,
    NonInteractiveUserInputHandler, PermissionRequestCoordinator, SessionEventPublisher,
    SessionEventStream, SessionMonitorManager, SessionPermissionRequestHandler, SessionTaskManager,
    SessionToolApprovalHandler, SessionUserInputHandler, SessionWorktreeManager,
    UserInputCoordinator, build_code_agent_tool_approval_policy, build_plugin_activation_plan,
    build_sandbox_policy, build_system_preamble, build_tool_context, dedup_mcp_servers,
    log_sandbox_status, merge_driver_host_inputs, resolve_mcp_servers, resolve_skill_roots,
    tool_context_for_profile,
};
use crate::options::AppOptions;
use crate::provider::{
    MutableAgentBackend, agent_backend_capabilities, build_agent_backend, build_internal_backend,
    build_mutable_agent_backend, provider_label,
};
use agent::mcp::{
    ConnectedMcpServer, McpConnectOptions, McpServerConfig, McpTransportConfig,
    catalog_resource_tools_as_registry_entries,
};
use agent::runtime::{
    CompactionConfig, ConversationCompactor, ModelBackend, ModelConversationCompactor,
    PermissionGrantStore, RuntimeSubagentExecutor, SubagentProfileResolver, SubagentRuntimeProfile,
    ToolApprovalHandler, ToolApprovalPolicy,
};
use agent::tools::{
    HOST_FEATURE_HOST_PROCESS_SURFACES, HOST_FEATURE_REQUEST_PERMISSIONS,
    HOST_FEATURE_REQUEST_USER_INPUT, SubagentExecutor, SubagentLaunchSpec, describe_sandbox_policy,
    ensure_sandbox_policy_supported,
};
use agent::types::{HookHandler, HookRegistration, ToolSpec};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, SandboxPolicy, SkillCatalog, ToolExecutionContext,
    ToolRegistry,
};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use code_agent_config::{builtin_skill_root, filter_unavailable_builtin_mcp_servers};
use nanoclaw_config::{CoreConfig, ResolvedAgentProfile};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{info, warn};

struct RuntimeBuildResult {
    runtime: AgentRuntime,
    model_backend: MutableAgentBackend,
    memory_backend: Option<Arc<dyn agent::memory::MemoryBackend>>,
    session_memory_refresh_state: SharedSessionMemoryRefreshState,
    subagent_executor: Arc<dyn SubagentExecutor>,
    monitor_manager: Arc<dyn agent::tools::MonitorManager>,
    worktree_manager: Arc<SessionWorktreeManager>,
    store: Arc<dyn store::SessionStore>,
    skill_catalog: SkillCatalog,
    plugin_instructions: Arc<RwLock<Vec<String>>>,
    mcp_servers: Vec<ConnectedMcpServer>,
    mcp_server_configs: Vec<McpServerConfig>,
    mcp_connection_details: BTreeMap<String, String>,
    runtime_hook_state: Arc<RwLock<Vec<HookRegistration>>>,
    configured_runtime_hooks: Arc<RwLock<Vec<HookRegistration>>>,
    driver_tool_names: Vec<String>,
    mcp_process_executor: Arc<dyn agent::tools::ProcessExecutor>,
    host_process_executor: Arc<SwitchableHostProcessExecutor>,
    command_hook_executor: Arc<SwitchableCommandHookExecutor>,
    code_intel_backend: Arc<SwitchableCodeIntelBackend>,
    host_process_surfaces_allowed: bool,
    tool_catalog_specs: Vec<ToolSpec>,
    disabled_tool_names: BTreeSet<String>,
    store_label: String,
    store_warning: Option<String>,
    stored_session_count: usize,
    startup_diagnostics: crate::backend::StartupDiagnosticsSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootProgressStage {
    Store,
    Plugins,
    Skills,
    Tooling,
    Mcp,
    Finalize,
}

impl BootProgressStage {
    pub const ALL: [Self; 6] = [
        Self::Store,
        Self::Plugins,
        Self::Skills,
        Self::Tooling,
        Self::Mcp,
        Self::Finalize,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Store => "Store",
            Self::Plugins => "Plugins",
            Self::Skills => "Skills",
            Self::Tooling => "Tooling",
            Self::Mcp => "MCP",
            Self::Finalize => "Finalize",
        }
    }

    pub fn position(self) -> usize {
        Self::ALL
            .iter()
            .position(|stage| *stage == self)
            .expect("boot progress stage must stay registered")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootProgressStatus {
    Started,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootProgressItemKind {
    Store,
    Plugin,
    SkillRoot,
    Skill,
    McpServer,
    ToolSurface,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootProgressItem {
    pub kind: BootProgressItemKind,
    pub label: String,
}

impl BootProgressItem {
    fn new(kind: BootProgressItemKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootProgressUpdate {
    pub stage: BootProgressStage,
    pub status: BootProgressStatus,
    pub items: Vec<BootProgressItem>,
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionApprovalMode {
    Interactive,
    NonInteractive,
}

#[derive(Clone)]
pub struct CodeAgentSubagentProfileResolver {
    pub core: CoreConfig,
    pub env_map: EnvMap,
    pub base_tool_context: Arc<RwLock<ToolExecutionContext>>,
    pub skill_catalog: SkillCatalog,
    pub plugin_instructions: Arc<RwLock<Vec<String>>>,
}

impl CodeAgentSubagentProfileResolver {
    pub fn resolve_agent_profile(
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
                &self.plugin_instructions.read().unwrap(),
                &base_tool_context.model_visibility,
            ),
            supports_tool_calls: profile.model.capabilities.tool_calls,
        })
    }
}

fn configure_host_prompt_tool_visibility(
    tool_context: &mut ToolExecutionContext,
    approval_mode: SessionApprovalMode,
) {
    if !matches!(approval_mode, SessionApprovalMode::Interactive) {
        return;
    }

    // Host-mediated prompt tools should only be advertised when the active
    // session can actually surface and resolve those prompts.
    tool_context.model_visibility = tool_context
        .model_visibility
        .clone()
        .with_feature(HOST_FEATURE_REQUEST_USER_INPUT)
        .with_feature(HOST_FEATURE_REQUEST_PERMISSIONS);
}

fn configure_host_process_tool_visibility(tool_context: &mut ToolExecutionContext, enabled: bool) {
    // Host-process tools may stay registered so permission-mode switches can
    // reveal or hide them without rebuilding the runtime. The active session
    // mode still controls whether the model can see those surfaces.
    tool_context
        .model_visibility
        .set_feature_enabled(HOST_FEATURE_HOST_PROCESS_SURFACES, enabled);
}

pub async fn build_session(
    options: &AppOptions,
    workspace_root: &Path,
) -> Result<super::CodeAgentSession> {
    build_session_with_approval_mode_and_progress(
        options,
        workspace_root,
        SessionApprovalMode::Interactive,
        |_| {},
    )
    .await
}

pub async fn build_session_with_approval_mode(
    options: &AppOptions,
    workspace_root: &Path,
    approval_mode: SessionApprovalMode,
) -> Result<super::CodeAgentSession> {
    build_session_with_approval_mode_and_progress(options, workspace_root, approval_mode, |_| {})
        .await
}

pub async fn build_session_with_approval_mode_and_progress<F>(
    options: &AppOptions,
    workspace_root: &Path,
    approval_mode: SessionApprovalMode,
    mut progress: F,
) -> Result<super::CodeAgentSession>
where
    F: FnMut(BootProgressUpdate),
{
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
    configure_host_prompt_tool_visibility(&mut base_tool_context, approval_mode);
    let sandbox_policy = build_sandbox_policy(options, &base_tool_context);
    let sandbox_status = ensure_sandbox_policy_supported(&sandbox_policy)
        .context("sandbox policy cannot be enforced on this host")?;
    let mut tool_context = base_tool_context.with_sandbox_policy(sandbox_policy.clone());
    configure_host_process_tool_visibility(
        &mut tool_context,
        host_process_surfaces_allowed(&sandbox_policy, &sandbox_status),
    );
    let session_tool_context = Arc::new(RwLock::new(tool_context.clone()));
    log_sandbox_status(&sandbox_status);
    let sandbox_summary = describe_sandbox_policy(&sandbox_policy, &sandbox_status);

    let RuntimeBuildResult {
        runtime,
        model_backend,
        memory_backend,
        session_memory_refresh_state,
        subagent_executor,
        monitor_manager,
        worktree_manager,
        store,
        skill_catalog,
        plugin_instructions,
        mcp_servers,
        mcp_server_configs,
        mcp_connection_details,
        runtime_hook_state,
        configured_runtime_hooks,
        driver_tool_names,
        mcp_process_executor,
        host_process_executor,
        command_hook_executor,
        code_intel_backend,
        host_process_surfaces_allowed,
        tool_catalog_specs,
        disabled_tool_names,
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
        events.clone(),
        permission_grants.clone(),
        &mut progress,
    )
    .await?;
    progress(BootProgressUpdate {
        stage: BootProgressStage::Finalize,
        status: BootProgressStatus::Started,
        items: vec![
            BootProgressItem::new(
                BootProgressItemKind::ToolSurface,
                provider_label(&options.primary_profile),
            ),
            BootProgressItem::new(
                BootProgressItemKind::ToolSurface,
                options.primary_profile.model.model.clone(),
            ),
        ],
        note: Some("Publishing session surfaces".to_string()),
    });
    let tool_names = runtime.tool_registry_names();
    let supported_model_reasoning_efforts = model_backend.supported_reasoning_efforts();
    let backend_capabilities = agent_backend_capabilities(&options.primary_profile);
    // Persisted history and CLI resume/fork both key off the substrate
    // `session_id`, so the operator-facing session reference should stay equal
    // to the durable store id instead of introducing a second host alias.
    let active_session_ref = runtime.session_id().to_string();
    let root_agent_session_id = runtime.agent_session_id().to_string();
    let session_memory_model_backend: Arc<dyn ModelBackend> = Arc::new(model_backend.clone());

    let session = super::CodeAgentSession::new(
        runtime,
        Some(model_backend),
        Some(session_memory_model_backend),
        subagent_executor,
        monitor_manager,
        worktree_manager,
        store,
        mcp_servers,
        mcp_server_configs,
        mcp_connection_details,
        runtime_hook_state,
        configured_runtime_hooks,
        mcp_process_executor,
        host_process_executor,
        command_hook_executor,
        code_intel_backend,
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
            tool_specs: tool_catalog_specs,
            disabled_tool_names: disabled_tool_names.iter().cloned().collect(),
            store_label,
            store_warning,
            stored_session_count: stored_session_count,
            default_sandbox_summary: sandbox_summary.clone(),
            sandbox_summary,
            permission_mode: crate::SessionPermissionMode::Default,
            host_process_surfaces_allowed,
            startup_diagnostics,
            display: options.display.clone(),
            statusline: options.statusline.clone(),
            motion: options.motion.clone(),
        },
        skill_catalog,
        plugin_instructions,
        memory_backend,
        session_memory_refresh_state,
        super::session::ManagedSurfaceReloadConfig {
            env_map: options.env_map.clone(),
            primary_profile: options.primary_profile.clone(),
            skill_roots: options.skill_roots.clone(),
            disabled_builtin_skills: Arc::new(RwLock::new(options.disabled_builtin_skills.clone())),
            plugins: Arc::new(RwLock::new(options.plugins.clone())),
        },
        driver_tool_names,
    );
    progress(BootProgressUpdate {
        stage: BootProgressStage::Finalize,
        status: BootProgressStatus::Completed,
        items: vec![BootProgressItem::new(
            BootProgressItemKind::ToolSurface,
            session.startup_snapshot().active_session_ref,
        )],
        note: Some("Session ready".to_string()),
    });
    Ok(session)
}

async fn build_runtime<F>(
    options: &AppOptions,
    workspace_root: &Path,
    approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_context: ToolExecutionContext,
    session_tool_context: Arc<RwLock<ToolExecutionContext>>,
    sandbox_policy: SandboxPolicy,
    sandbox_status: agent::tools::SandboxBackendStatus,
    events: SessionEventStream,
    permission_grants: PermissionGrantStore,
    progress: &mut F,
) -> Result<RuntimeBuildResult>
where
    F: FnMut(BootProgressUpdate),
{
    progress(BootProgressUpdate {
        stage: BootProgressStage::Store,
        status: BootProgressStatus::Started,
        items: vec![BootProgressItem::new(
            BootProgressItemKind::Store,
            "session store",
        )],
        note: Some("Opening persisted state".to_string()),
    });
    let session_memory_refresh_state = Arc::new(Mutex::new(SessionMemoryRefreshState::default()));
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
    progress(BootProgressUpdate {
        stage: BootProgressStage::Store,
        status: BootProgressStatus::Completed,
        items: vec![BootProgressItem::new(
            BootProgressItemKind::Store,
            store_handle.label.clone(),
        )],
        note: Some(format!("{stored_session_count} stored session(s)")),
    });
    progress(BootProgressUpdate {
        stage: BootProgressStage::Plugins,
        status: BootProgressStatus::Started,
        items: Vec::new(),
        note: Some("Resolving plugin activation plan".to_string()),
    });
    let plugin_plan = build_plugin_activation_plan(workspace_root, &options.plugins)
        .context("failed to build plugin activation plan")?;
    let enabled_plugins = plugin_plan
        .plugin_states
        .iter()
        .filter(|state| state.enabled)
        .map(|state| {
            BootProgressItem::new(BootProgressItemKind::Plugin, state.plugin_id.to_string())
        })
        .collect::<Vec<_>>();
    progress(BootProgressUpdate {
        stage: BootProgressStage::Plugins,
        status: BootProgressStatus::Completed,
        items: enabled_plugins,
        note: Some(format!(
            "{} plugin(s) enabled",
            plugin_plan
                .plugin_states
                .iter()
                .filter(|state| state.enabled)
                .count()
        )),
    });
    let skill_roots = resolve_skill_roots(&options.skill_roots, workspace_root, &plugin_plan);
    progress(BootProgressUpdate {
        stage: BootProgressStage::Skills,
        status: BootProgressStatus::Started,
        items: skill_roots
            .iter()
            .map(|root| {
                let label = root
                    .path
                    .strip_prefix(workspace_root)
                    .unwrap_or(root.path.as_path())
                    .display()
                    .to_string();
                BootProgressItem::new(BootProgressItemKind::SkillRoot, label)
            })
            .collect(),
        note: Some("Loading skill roots".to_string()),
    });
    let skill_catalog = filter_disabled_builtin_skills(
        workspace_root,
        &options.disabled_builtin_skills,
        agent::skills::load_skill_roots(&skill_roots)
            .await
            .context("failed to load skill roots")?,
    );
    let skills = skill_catalog.all();
    progress(BootProgressUpdate {
        stage: BootProgressStage::Skills,
        status: BootProgressStatus::Completed,
        items: skills
            .iter()
            .map(|skill| BootProgressItem::new(BootProgressItemKind::Skill, skill.name.clone()))
            .collect(),
        note: Some(format!("{} skill(s) loaded", skills.len())),
    });
    let runtime_hooks = plugin_plan.hooks.clone();
    let plugin_mcp_servers = plugin_plan.mcp_servers.clone();
    // Managed MCP UI should see the same core-plus-plugin catalog that later
    // surface refreshes operate on. Driver-provided MCP servers remain runtime
    // host inputs rather than managed entries, so they get merged just before
    // connection below.
    let configured_mcp_server_configs = dedup_mcp_servers(resolve_mcp_servers(
        &merge_boot_mcp_servers(options.core.mcp_servers.clone(), plugin_mcp_servers.clone()),
        workspace_root,
    ));
    let plugin_instructions = plugin_plan.instructions.clone();
    let model_compactor: Arc<dyn ConversationCompactor> =
        Arc::new(ModelConversationCompactor::new(summary_backend));
    let compactor: Arc<dyn ConversationCompactor> =
        Arc::new(SessionMemoryConversationCompactor::new(
            workspace_root.to_path_buf(),
            session_memory_refresh_state.clone(),
            model_compactor,
        ));
    progress(BootProgressUpdate {
        stage: BootProgressStage::Tooling,
        status: BootProgressStatus::Started,
        items: vec![
            BootProgressItem::new(BootProgressItemKind::ToolSurface, "local tools"),
            BootProgressItem::new(BootProgressItemKind::ToolSurface, "command hooks"),
            BootProgressItem::new(BootProgressItemKind::ToolSurface, "code intelligence"),
        ],
        note: Some("Building runtime surfaces".to_string()),
    });
    // Runtime tooling assembly is still host boot work, but it lives behind a
    // dedicated helper so later frontends inherit the same process-local tool,
    // hook, and LSP wiring without reopening this orchestration block.
    let runtime_tooling = build_runtime_tooling(
        options,
        workspace_root,
        &sandbox_policy,
        &sandbox_status,
        skill_catalog.clone(),
    );
    let loop_detection_config = runtime_tooling.loop_detection_config;
    let process_executor = runtime_tooling.process_executor.clone();
    let command_hook_executor = runtime_tooling.command_hook_executor.clone();
    let code_intel_backend = runtime_tooling.code_intel_backend.clone();
    let host_process_surfaces_allowed = runtime_tooling.host_process_surfaces_allowed;
    let mut startup_warnings = runtime_tooling.startup_warnings.clone();
    let hook_runner = runtime_tooling.hook_runner.clone();
    let mut tools = runtime_tooling.tools;
    progress(BootProgressUpdate {
        stage: BootProgressStage::Tooling,
        status: BootProgressStatus::Completed,
        items: vec![
            BootProgressItem::new(
                BootProgressItemKind::ToolSurface,
                if host_process_surfaces_allowed {
                    "host surfaces enabled"
                } else {
                    "host surfaces degraded"
                },
            ),
            BootProgressItem::new(
                BootProgressItemKind::ToolSurface,
                if runtime_tooling.startup_warnings.is_empty() {
                    "startup checks clean"
                } else {
                    "startup warnings present"
                },
            ),
        ],
        note: Some("Runtime surfaces ready".to_string()),
    });
    // Custom tools can stay registered even when the current session mode
    // hides host-process surfaces. Execution still goes through the same
    // process executor and active sandbox policy once the operator enables it.
    let custom_tool_executor: Option<Arc<dyn agent::tools::ProcessExecutor>> =
        Some(process_executor.clone() as Arc<_>);
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
    // Boot should connect the same effective MCP set that session reloads use:
    // managed/core entries first, then plugin/driver additions. Dedup keeps
    // the first config per name so explicit workspace overrides still win over
    // plugin or driver defaults.
    let resolved_mcp_servers = dedup_mcp_servers(resolve_mcp_servers(
        &merge_boot_mcp_servers(options.core.mcp_servers.clone(), mcp_servers),
        workspace_root,
    ));
    #[cfg(feature = "browser-tools")]
    let browser_manager: Arc<dyn agent::tools::BrowserManager> =
        Arc::new(SessionBrowserManager::new(store.clone(), events.clone()));
    #[cfg(feature = "browser-tools")]
    register_browser_tools(&mut tools, browser_manager);
    progress(BootProgressUpdate {
        stage: BootProgressStage::Mcp,
        status: BootProgressStatus::Started,
        items: resolved_mcp_servers
            .iter()
            .map(|server| {
                BootProgressItem::new(BootProgressItemKind::McpServer, server.name.to_string())
            })
            .collect(),
        note: Some("Connecting MCP servers".to_string()),
    });
    let available_mcp_servers = filter_unavailable_builtin_mcp_servers(
        &options.env_map,
        resolved_mcp_servers.clone(),
        &mut startup_warnings,
    );
    let boot_mcp_servers = filter_boot_mcp_servers(
        available_mcp_servers,
        host_process_surfaces_allowed,
        &mut startup_warnings,
    );
    let mut connected_mcp_servers = Vec::new();
    let mut mcp_connection_details = BTreeMap::new();
    if !boot_mcp_servers.is_empty() {
        let mut prepared = connect_and_prepare_mcp_servers(
            boot_mcp_servers
                .iter()
                .cloned()
                .map(|server| {
                    let sandbox_policy = mcp_connection_sandbox_policy(&sandbox_policy, &server);
                    (
                        server,
                        McpConnectOptions {
                            process_executor: process_executor.clone(),
                            sandbox_policy,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        )
        .await;
        let conflicts = resolve_mcp_tool_conflicts(&tools, prepared.connected);
        prepared.details.extend(conflicts.details);
        prepared.connected = conflicts.connected;
        mcp_connection_details = prepared.details;
        for (_, adapters) in &prepared.connected {
            for adapter in adapters.clone() {
                tools.register(adapter);
            }
        }
        connected_mcp_servers = prepared
            .connected
            .into_iter()
            .map(|(server, _)| server)
            .collect();
        for resource_tool in
            catalog_resource_tools_as_registry_entries(connected_mcp_servers.clone())
        {
            tools.register(resource_tool);
        }
    }
    let disabled_tool_names = disabled_tool_names(options, &options.env_map);
    let mut disabled_tool_hits = BTreeSet::new();
    let mut tool_catalog_specs = tools.specs();
    apply_disabled_tools(&tools, &disabled_tool_names, &mut disabled_tool_hits);
    progress(BootProgressUpdate {
        stage: BootProgressStage::Mcp,
        status: BootProgressStatus::Completed,
        items: connected_mcp_servers
            .iter()
            .map(|server| {
                BootProgressItem::new(
                    BootProgressItemKind::McpServer,
                    server.server_name.to_string(),
                )
            })
            .collect(),
        note: Some(format!(
            "{} MCP server(s) connected",
            connected_mcp_servers.len()
        )),
    });
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
    let configured_runtime_hooks = Arc::new(RwLock::new(runtime_hooks));
    let runtime_hooks = filter_runtime_hooks(
        configured_runtime_hooks.read().unwrap().clone(),
        host_process_surfaces_allowed,
        &mut startup_warnings,
    );
    let runtime_hook_state = Arc::new(RwLock::new(runtime_hooks.clone()));
    let plugin_instructions = Arc::new(RwLock::new(plugin_instructions));
    let instructions = build_system_preamble(
        workspace_root,
        &options.primary_profile,
        &plugin_instructions.read().unwrap(),
        &tool_context.model_visibility,
    );
    let subagent_profile_resolver = Arc::new(CodeAgentSubagentProfileResolver {
        core: options.core.clone(),
        env_map: options.env_map.clone(),
        base_tool_context: session_tool_context.clone(),
        skill_catalog: skill_catalog.clone(),
        plugin_instructions: plugin_instructions.clone(),
    });
    let subagent_progress_sink = Arc::new(SessionEventPublisher::new(events.clone()));
    let approval_policy: Arc<dyn ToolApprovalPolicy> = Arc::new(
        build_code_agent_tool_approval_policy(&options.approval_policy),
    );
    let worktree_manager = Arc::new(SessionWorktreeManager::new(
        store.clone(),
        events.clone(),
        runtime_tooling.process_executor.clone(),
        workspace_root.to_path_buf(),
        session_tool_context.clone(),
    ));
    let subagent_executor: Arc<dyn SubagentExecutor> = Arc::new(RuntimeSubagentExecutor::new(
        hook_runner.clone(),
        store.clone(),
        tools.clone(),
        session_tool_context.clone(),
        approval_handler.clone(),
        approval_policy.clone(),
        loop_detection_config.clone(),
        runtime_hook_state.clone(),
        skill_catalog.clone(),
        subagent_profile_resolver,
        Some(worktree_manager.clone()),
        Some(subagent_progress_sink),
    ));
    let task_manager: Arc<dyn agent::tools::TaskManager> = Arc::new(SessionTaskManager::new(
        store.clone(),
        subagent_executor.clone(),
        events.clone(),
    ));
    #[cfg(feature = "automation-tools")]
    let cron_manager = SessionCronManager::new(store.clone(), events.clone(), task_manager.clone());
    #[cfg(feature = "automation-tools")]
    if let Err(error) = cron_manager.restore_all_sessions().await {
        let warning = format!("failed to restore persisted automations: {error}");
        warn!("{warning}");
        startup_warnings.push(warning);
    }
    #[cfg(feature = "automation-tools")]
    let cron_manager: Arc<dyn agent::CronManager> = Arc::new(cron_manager);
    let monitor_manager: Arc<dyn agent::tools::MonitorManager> =
        Arc::new(SessionMonitorManager::new(
            store.clone(),
            events.clone(),
            runtime_tooling.process_executor.clone(),
        ));
    #[cfg(feature = "automation-tools")]
    register_automation_tools(&mut tools, cron_manager);
    register_monitor_tools(&mut tools, monitor_manager.clone());
    register_worktree_tools(&mut tools, worktree_manager.clone());
    register_subagent_tools(&mut tools, subagent_executor.clone(), task_manager);
    tool_catalog_specs.extend(tools.specs());
    apply_disabled_tools(&tools, &disabled_tool_names, &mut disabled_tool_hits);
    append_disabled_tool_warnings(
        &disabled_tool_names,
        &disabled_tool_hits,
        &mut startup_warnings,
    );
    let tool_specs = tools
        .specs()
        .into_iter()
        .filter(|spec| spec.is_model_visible(&tool_context.model_visibility))
        .collect::<Vec<_>>();
    let tool_catalog_specs = merge_tool_catalog_specs(tool_catalog_specs)
        .into_iter()
        .filter(|spec| spec.is_model_visible(&tool_context.model_visibility))
        .collect::<Vec<_>>();
    let mut startup_diagnostics = build_startup_diagnostics_snapshot(
        workspace_root,
        &tool_specs,
        &connected_mcp_servers,
        &plugin_plan,
        &startup_warnings,
        &driver_outcome,
    );
    startup_diagnostics.mcp_servers = summarize_mcp_servers(
        &configured_mcp_server_configs,
        &connected_mcp_servers,
        &mcp_connection_details,
    );
    let memory_backend = driver_outcome.primary_memory_backend.clone();
    let runtime_builder = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .shared_tool_context(session_tool_context.clone())
        .tool_approval_handler(approval_handler)
        .tool_approval_policy(approval_policy)
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
        session_memory_refresh_state,
        subagent_executor,
        monitor_manager,
        worktree_manager,
        store,
        skill_catalog,
        plugin_instructions,
        mcp_servers: connected_mcp_servers,
        mcp_server_configs: configured_mcp_server_configs,
        mcp_connection_details,
        runtime_hook_state,
        configured_runtime_hooks,
        driver_tool_names: driver_outcome.tool_names.clone(),
        mcp_process_executor: process_executor.clone() as Arc<dyn agent::tools::ProcessExecutor>,
        host_process_executor: process_executor,
        command_hook_executor,
        code_intel_backend,
        host_process_surfaces_allowed,
        tool_catalog_specs,
        disabled_tool_names,
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
            "{COMMAND_HOOK_DISABLED_WARNING_PREFIX} {}",
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

fn merge_boot_mcp_servers(
    configured_mcp_servers: Vec<McpServerConfig>,
    plugin_mcp_servers: Vec<McpServerConfig>,
) -> Vec<McpServerConfig> {
    // Boot should use the same precedence as post-startup managed-surface
    // refreshes: operator-managed/core MCP entries first, then declarative
    // plugin additions. Later dedup keeps the first server with a given name so
    // explicit workspace overrides win over plugin defaults.
    [configured_mcp_servers, plugin_mcp_servers].concat()
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

fn filter_disabled_builtin_skills(
    workspace_root: &Path,
    disabled_builtin_skills: &BTreeSet<String>,
    skill_catalog: SkillCatalog,
) -> SkillCatalog {
    if disabled_builtin_skills.is_empty() {
        return skill_catalog;
    }
    let builtin_root = builtin_skill_root(workspace_root);
    let filtered = skill_catalog
        .all()
        .into_iter()
        .filter(|skill| {
            !(skill.provenance.root.path == builtin_root
                && disabled_builtin_skills.contains(&skill.name))
        })
        .collect();
    SkillCatalog::from_parts(skill_catalog.roots(), filtered)
}

fn disabled_tool_names_from_env(env_map: &EnvMap) -> BTreeSet<String> {
    env_map
        .get_non_empty_var(agent_env::vars::NANOCLAW_CORE_DISABLED_TOOLS)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn disabled_tool_names(options: &AppOptions, env_map: &EnvMap) -> BTreeSet<String> {
    let mut names = options.disabled_tools.clone();
    names.extend(disabled_tool_names_from_env(env_map));
    names
}

fn apply_disabled_tools(
    tools: &ToolRegistry,
    requested_names: &BTreeSet<String>,
    removed_names: &mut BTreeSet<String>,
) {
    for name in requested_names {
        if tools.remove(name) {
            removed_names.insert(name.clone());
        }
    }
}

fn merge_tool_catalog_specs(specs: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let mut merged = BTreeMap::new();
    for spec in specs {
        merged.entry(spec.name.clone()).or_insert(spec);
    }
    merged.into_values().collect()
}

fn append_disabled_tool_warnings(
    requested_names: &BTreeSet<String>,
    removed_names: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) {
    if !removed_names.is_empty() {
        warnings.push(format!(
            "disabled tool(s) via {}: {}",
            agent_env::vars::NANOCLAW_CORE_DISABLED_TOOLS.key,
            removed_names.iter().cloned().collect::<Vec<_>>().join(", "),
        ));
    }
    let unknown_names = requested_names
        .difference(removed_names)
        .cloned()
        .collect::<Vec<_>>();
    if !unknown_names.is_empty() {
        warnings.push(format!(
            "ignored unknown tool name(s) in {}: {}",
            agent_env::vars::NANOCLAW_CORE_DISABLED_TOOLS.key,
            unknown_names.join(", "),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SessionApprovalMode, append_disabled_tool_warnings, apply_disabled_tools,
        configure_host_prompt_tool_visibility, disabled_tool_names_from_env,
        filter_boot_mcp_servers, filter_disabled_builtin_skills, filter_runtime_hooks,
        merge_boot_mcp_servers,
    };
    use crate::backend::{dedup_mcp_servers, merge_driver_host_inputs};
    use agent::DriverActivationOutcome;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent::skills::load_skill_roots;
    use agent::types::{
        CommandHookHandler, HookEvent, HookHandler, HookRegistration, HttpHookHandler,
    };
    use agent::{
        RequestPermissionsTool, RequestUserInputTool, SkillRoot, ToolExecutionContext, ToolRegistry,
    };
    use agent_env::EnvMap;
    use code_agent_config::{builtin_skill_root, materialize_builtin_skills};
    use std::collections::{BTreeMap, BTreeSet};

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
                    enabled: true,
                    bootstrap_network: None,
                    runtime_network: None,
                    transport: McpTransportConfig::Stdio {
                        command: "stdio-server".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: None,
                    },
                },
                McpServerConfig {
                    name: "http".into(),
                    enabled: true,
                    bootstrap_network: None,
                    runtime_network: None,
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

    #[test]
    fn boot_mcp_merge_keeps_core_entries_ahead_of_plugin_defaults() {
        let merged = merge_boot_mcp_servers(
            vec![McpServerConfig {
                name: "context7".into(),
                enabled: true,
                bootstrap_network: None,
                runtime_network: None,
                transport: McpTransportConfig::Stdio {
                    command: "core-context7".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            vec![
                McpServerConfig {
                    name: "context7".into(),
                    enabled: true,
                    bootstrap_network: None,
                    runtime_network: None,
                    transport: McpTransportConfig::Stdio {
                        command: "plugin-context7".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: None,
                    },
                },
                McpServerConfig {
                    name: "plugin-docs".into(),
                    enabled: true,
                    bootstrap_network: None,
                    runtime_network: None,
                    transport: McpTransportConfig::StreamableHttp {
                        url: "https://example.test/mcp".to_string(),
                        headers: BTreeMap::new(),
                    },
                },
            ],
        );
        let deduped = dedup_mcp_servers(merged);

        assert_eq!(deduped.len(), 2);
        let context7 = deduped
            .iter()
            .find(|server| server.name.as_str() == "context7")
            .expect("context7 should be retained");
        assert!(matches!(
            context7.transport,
            McpTransportConfig::Stdio { ref command, .. } if command == "core-context7"
        ));
        assert!(
            deduped
                .iter()
                .any(|server| server.name.as_str() == "plugin-docs")
        );
    }

    #[test]
    fn boot_mcp_merge_keeps_core_entries_ahead_of_plugin_and_driver_defaults() {
        let merged_driver = merge_driver_host_inputs(
            Vec::new(),
            vec![McpServerConfig {
                name: "plugin-docs".into(),
                enabled: true,
                bootstrap_network: None,
                runtime_network: None,
                transport: McpTransportConfig::StreamableHttp {
                    url: "https://example.test/plugin".to_string(),
                    headers: BTreeMap::new(),
                },
            }],
            Vec::new(),
            &DriverActivationOutcome {
                mcp_servers: vec![
                    McpServerConfig {
                        name: "context7".into(),
                        enabled: true,
                        bootstrap_network: None,
                        runtime_network: None,
                        transport: McpTransportConfig::Stdio {
                            command: "driver-context7".to_string(),
                            args: Vec::new(),
                            env: BTreeMap::new(),
                            cwd: None,
                        },
                    },
                    McpServerConfig {
                        name: "driver-logs".into(),
                        enabled: true,
                        bootstrap_network: None,
                        runtime_network: None,
                        transport: McpTransportConfig::StreamableHttp {
                            url: "https://example.test/driver".to_string(),
                            headers: BTreeMap::new(),
                        },
                    },
                ],
                ..Default::default()
            },
        );
        let deduped = dedup_mcp_servers(merge_boot_mcp_servers(
            vec![McpServerConfig {
                name: "context7".into(),
                enabled: true,
                bootstrap_network: None,
                runtime_network: None,
                transport: McpTransportConfig::Stdio {
                    command: "core-context7".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            merged_driver.mcp_servers,
        ));

        assert_eq!(deduped.len(), 3);
        let context7 = deduped
            .iter()
            .find(|server| server.name.as_str() == "context7")
            .expect("context7 should be retained");
        assert!(matches!(
            context7.transport,
            McpTransportConfig::Stdio { ref command, .. } if command == "core-context7"
        ));
        assert!(
            deduped
                .iter()
                .any(|server| server.name.as_str() == "plugin-docs")
        );
        assert!(
            deduped
                .iter()
                .any(|server| server.name.as_str() == "driver-logs")
        );
    }

    #[test]
    fn disabled_tool_names_are_parsed_from_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "NANOCLAW_CORE_DISABLED_TOOLS=web_search,exec_command,,request_user_input\n",
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();

        assert_eq!(
            disabled_tool_names_from_env(&env_map),
            BTreeSet::from([
                "exec_command".to_string(),
                "request_user_input".to_string(),
                "web_search".to_string(),
            ])
        );
    }

    #[test]
    fn applying_disabled_tools_removes_requested_entries_and_reports_unknown_names() {
        let mut registry = ToolRegistry::new();
        registry.register(RequestUserInputTool::new());
        registry.register(RequestPermissionsTool::new());

        let requested =
            BTreeSet::from(["request_user_input".to_string(), "missing_tool".to_string()]);
        let mut removed = BTreeSet::new();
        apply_disabled_tools(&registry, &requested, &mut removed);

        assert!(registry.get("request_user_input").is_none());
        assert!(registry.get("request_permissions").is_some());
        assert_eq!(removed, BTreeSet::from(["request_user_input".to_string()]));

        let mut warnings = Vec::new();
        append_disabled_tool_warnings(&requested, &removed, &mut warnings);
        assert_eq!(
            warnings,
            vec![
                "disabled tool(s) via NANOCLAW_CORE_DISABLED_TOOLS: request_user_input".to_string(),
                "ignored unknown tool name(s) in NANOCLAW_CORE_DISABLED_TOOLS: missing_tool"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn host_prompt_tools_are_only_visible_in_interactive_sessions() {
        let mut registry = ToolRegistry::new();
        registry.register(RequestUserInputTool::new());
        registry.register(RequestPermissionsTool::new());

        let mut interactive = ToolExecutionContext::default();
        configure_host_prompt_tool_visibility(&mut interactive, SessionApprovalMode::Interactive);
        let interactive_names = registry
            .specs()
            .into_iter()
            .filter(|spec| spec.is_model_visible(&interactive.model_visibility))
            .map(|spec| spec.name.to_string())
            .collect::<Vec<_>>();
        assert!(interactive_names.contains(&"request_user_input".to_string()));
        assert!(interactive_names.contains(&"request_permissions".to_string()));

        let mut non_interactive = ToolExecutionContext::default();
        configure_host_prompt_tool_visibility(
            &mut non_interactive,
            SessionApprovalMode::NonInteractive,
        );
        let non_interactive_names = registry
            .specs()
            .into_iter()
            .filter(|spec| spec.is_model_visible(&non_interactive.model_visibility))
            .map(|spec| spec.name.to_string())
            .collect::<Vec<_>>();
        assert!(!non_interactive_names.contains(&"request_user_input".to_string()));
        assert!(!non_interactive_names.contains(&"request_permissions".to_string()));
    }

    #[tokio::test]
    async fn filtering_disabled_builtin_skills_drops_only_builtin_entries() {
        let dir = tempfile::tempdir().unwrap();
        materialize_builtin_skills(dir.path()).unwrap();
        let managed_root = dir.path().join(".nanoclaw/skills");
        std::fs::create_dir_all(managed_root.join("local-review")).unwrap();
        std::fs::write(
            managed_root.join("local-review/SKILL.md"),
            r#"---
name: local-review
description: local review flow
---

Use the local review skill.
"#,
        )
        .unwrap();

        let catalog = load_skill_roots(&[
            SkillRoot::managed(managed_root),
            SkillRoot::external(builtin_skill_root(dir.path())),
        ])
        .await
        .unwrap();

        let filtered = filter_disabled_builtin_skills(
            dir.path(),
            &BTreeSet::from(["frontend-skill".to_string()]),
            catalog,
        );
        let names = filtered
            .all()
            .into_iter()
            .map(|skill| skill.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"local-review".to_string()));
        assert!(names.contains(&"pdf".to_string()));
        assert!(!names.contains(&"frontend-skill".to_string()));
    }
}
