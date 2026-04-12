use crate::options::AppOptions;
#[cfg(feature = "notebook-tools")]
use agent::NotebookReadTool;
use agent::runtime::RuntimeError;
use agent::runtime::{
    CommandHookExecutor, DefaultCommandHookExecutor, HookRunner, LoopDetectionConfig,
};
use agent::tools::{
    CodeCallHierarchyDirection, CodeCallHierarchyEntry, CodeDiagnostic, CodeHover,
    CodeNavigationTarget, CodeReference, CodeSearchMatch, CodeSymbol, FileActivityObserver,
    MonitorManager, ProcessExecutor, SandboxBackendStatus, SandboxError, SubagentExecutor,
    TaskManager, WorktreeManager,
};
use agent::{
    CodeDiagnosticsTool, CodeDocumentSymbolsTool, CodeIntelBackend, CodeNavTool, CodeSearchTool,
    CodeSymbolSearchTool, EditTool, ExecCommandTool, GlobTool, GrepTool, JsReplTool, ListTool,
    ManagedCodeIntelBackend, ManagedCodeIntelOptions, ManagedPolicyProcessExecutor,
    MonitorListTool, MonitorStartTool, MonitorStopTool, PatchFilesTool, ReadTool,
    RequestPermissionsTool, RequestUserInputTool, SandboxPolicy, SkillCatalog, SkillManageTool,
    SkillViewTool, SkillsListTool, ToolCallId, ToolDiscoverTool, ToolExecutionContext,
    ToolRegistry, ToolResult, WebFetchTool, WebSearchBackendsTool, WebSearchTool,
    WorkspaceTextCodeIntelBackend, WorktreeEnterTool, WorktreeExitTool, WorktreeListTool,
    WriteStdinTool, WriteTool,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tracing::warn;

pub const COMMAND_HOOK_DISABLED_WARNING_PREFIX: &str =
    "sandbox backend unavailable; disabled command hooks to avoid host subprocess execution:";
pub const MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX: &str = "sandbox backend unavailable; disabled managed code-intel helpers to avoid host subprocess execution:";
const HOST_PROCESS_SURFACES_DISABLED_MESSAGE: &str = "host subprocess surfaces are disabled in the current session. Switch /permissions to danger-full-access or enable a supported sandbox backend.";

#[derive(Clone)]
pub struct SwitchableHostProcessExecutor {
    inner: Arc<dyn ProcessExecutor>,
    enabled: Arc<RwLock<bool>>,
}

#[derive(Clone)]
pub struct SwitchableCommandHookExecutor {
    process_executor: Arc<dyn ProcessExecutor>,
    state: Arc<RwLock<SwitchableCommandHookState>>,
}

#[derive(Clone)]
struct SwitchableCommandHookState {
    enabled: bool,
    sandbox_policy: SandboxPolicy,
}

#[derive(Clone)]
struct ManagedCodeIntelConfig {
    workspace_root: PathBuf,
    options: ManagedCodeIntelOptions,
    process_executor: Arc<dyn ProcessExecutor>,
}

impl ManagedCodeIntelConfig {
    fn build_backend(&self) -> Arc<ManagedCodeIntelBackend> {
        Arc::new(ManagedCodeIntelBackend::new(
            self.workspace_root.clone(),
            self.options.clone(),
            self.process_executor.clone(),
            SandboxPolicy::permissive(),
            SandboxPolicy::permissive(),
        ))
    }
}

#[derive(Clone)]
pub struct SwitchableCodeIntelBackend {
    fallback: WorkspaceTextCodeIntelBackend,
    managed_config: Option<ManagedCodeIntelConfig>,
    managed_backend: Arc<RwLock<Option<Arc<ManagedCodeIntelBackend>>>>,
}

pub struct RuntimeTooling {
    pub hook_runner: Arc<HookRunner>,
    pub loop_detection_config: LoopDetectionConfig,
    pub process_executor: Arc<SwitchableHostProcessExecutor>,
    pub command_hook_executor: Arc<SwitchableCommandHookExecutor>,
    pub code_intel_backend: Arc<SwitchableCodeIntelBackend>,
    pub host_process_surfaces_allowed: bool,
    pub startup_warnings: Vec<String>,
    pub tools: ToolRegistry,
}

impl SwitchableHostProcessExecutor {
    #[must_use]
    pub fn new(inner: Arc<dyn ProcessExecutor>, enabled: bool) -> Self {
        Self {
            inner,
            enabled: Arc::new(RwLock::new(enabled)),
        }
    }

    pub fn set_host_process_surfaces(&self, enabled: bool) {
        *self.enabled.write().unwrap() = enabled;
    }

    pub fn ensure_enabled(&self, tool_name: &str) -> agent::tools::Result<()> {
        if *self.enabled.read().unwrap() {
            return Ok(());
        }
        Err(agent::tools::ToolError::invalid_state(format!(
            "{tool_name} is unavailable because {HOST_PROCESS_SURFACES_DISABLED_MESSAGE}"
        )))
    }
}

impl ProcessExecutor for SwitchableHostProcessExecutor {
    fn prepare(
        &self,
        request: agent::tools::ExecRequest,
    ) -> std::result::Result<tokio::process::Command, SandboxError> {
        if !*self.enabled.read().unwrap() {
            return Err(SandboxError::invalid_state(
                HOST_PROCESS_SURFACES_DISABLED_MESSAGE,
            ));
        }
        self.inner.prepare(request)
    }
}

impl SwitchableCommandHookExecutor {
    #[must_use]
    pub fn new(
        process_executor: Arc<dyn ProcessExecutor>,
        sandbox_policy: SandboxPolicy,
        enabled: bool,
    ) -> Self {
        Self {
            process_executor,
            state: Arc::new(RwLock::new(SwitchableCommandHookState {
                enabled,
                sandbox_policy,
            })),
        }
    }

    pub fn set_host_process_surfaces(&self, enabled: bool, sandbox_policy: SandboxPolicy) {
        let mut state = self.state.write().unwrap();
        state.enabled = enabled;
        state.sandbox_policy = sandbox_policy;
    }
}

#[derive(Clone)]
pub struct GuardedWriteStdinTool {
    gate: Arc<SwitchableHostProcessExecutor>,
    inner: WriteStdinTool,
}

impl GuardedWriteStdinTool {
    #[must_use]
    pub fn new(gate: Arc<SwitchableHostProcessExecutor>) -> Self {
        Self {
            gate,
            inner: WriteStdinTool::new(),
        }
    }
}

#[async_trait]
impl agent::Tool for GuardedWriteStdinTool {
    fn spec(&self) -> agent::types::ToolSpec {
        self.inner.spec()
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<ToolResult> {
        let _ = ctx;
        self.gate.ensure_enabled("write_stdin")?;
        self.inner.execute(call_id, arguments, ctx).await
    }
}

#[async_trait::async_trait]
impl CommandHookExecutor for SwitchableCommandHookExecutor {
    async fn execute(
        &self,
        registration: &agent::types::HookRegistration,
        context: agent::types::HookContext,
    ) -> agent::runtime::Result<agent::types::HookResult> {
        let state = self.state.read().unwrap().clone();
        if !state.enabled {
            return Err(RuntimeError::hook(
                "command hooks are disabled until host-process surfaces are enabled",
            ));
        }

        DefaultCommandHookExecutor::with_process_executor_and_policy(
            BTreeMap::new(),
            self.process_executor.clone(),
            state.sandbox_policy,
        )
        .execute(registration, context)
        .await
    }
}

impl SwitchableCodeIntelBackend {
    #[must_use]
    fn new(
        options: &AppOptions,
        workspace_root: &Path,
        process_executor: Arc<dyn ProcessExecutor>,
        host_process_surfaces_allowed: bool,
        sandbox_status: &SandboxBackendStatus,
        startup_warnings: &mut Vec<String>,
    ) -> Self {
        let managed_config = options.lsp_enabled.then(|| {
            let mut lsp_options = ManagedCodeIntelOptions::for_workspace(workspace_root);
            lsp_options.auto_install = options.lsp_auto_install;
            if let Some(install_root) = &options.lsp_install_root {
                lsp_options.install_root = install_root.clone();
            }
            ManagedCodeIntelConfig {
                workspace_root: workspace_root.to_path_buf(),
                options: lsp_options,
                process_executor,
            }
        });
        let managed_backend = if host_process_surfaces_allowed {
            managed_config
                .as_ref()
                .map(ManagedCodeIntelConfig::build_backend)
        } else {
            if managed_config.is_some()
                && let Some(reason) = sandbox_status.reason()
            {
                warn!(
                    "sandbox enforcement backend unavailable; disabling managed code-intel helpers to avoid host fallback: {reason}"
                );
                startup_warnings.push(format!(
                    "{MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX} {reason}"
                ));
            }
            None
        };

        Self {
            fallback: WorkspaceTextCodeIntelBackend::new(),
            managed_config,
            managed_backend: Arc::new(RwLock::new(managed_backend)),
        }
    }

    #[must_use]
    pub fn lexical_only() -> Self {
        Self {
            fallback: WorkspaceTextCodeIntelBackend::new(),
            managed_config: None,
            managed_backend: Arc::new(RwLock::new(None)),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn managed_for_workspace(
        workspace_root: &Path,
        process_executor: Arc<dyn ProcessExecutor>,
        enabled: bool,
    ) -> Self {
        let config = ManagedCodeIntelConfig {
            workspace_root: workspace_root.to_path_buf(),
            options: ManagedCodeIntelOptions::for_workspace(workspace_root),
            process_executor,
        };
        let managed_backend = enabled.then(|| config.build_backend());
        Self {
            fallback: WorkspaceTextCodeIntelBackend::new(),
            managed_config: Some(config),
            managed_backend: Arc::new(RwLock::new(managed_backend)),
        }
    }

    fn managed_backend_snapshot(&self) -> Option<Arc<ManagedCodeIntelBackend>> {
        self.managed_backend.read().unwrap().clone()
    }

    pub fn managed_helpers_supported(&self) -> bool {
        self.managed_config.is_some()
    }

    pub fn managed_helpers_enabled(&self) -> bool {
        self.managed_backend.read().unwrap().is_some()
    }

    pub fn set_managed_helpers_enabled(&self, enabled: bool) {
        let Some(config) = &self.managed_config else {
            return;
        };

        let mut backend = self.managed_backend.write().unwrap();
        if enabled {
            if backend.is_none() {
                *backend = Some(config.build_backend());
            }
        } else {
            *backend = None;
        }
    }
}

impl FileActivityObserver for SwitchableCodeIntelBackend {
    fn did_open(&self, path: PathBuf) {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.did_open(path);
        }
    }

    fn did_change(&self, path: PathBuf) {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.did_change(path);
        }
    }

    fn did_save(&self, path: PathBuf) {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.did_save(path);
        }
    }

    fn did_remove(&self, path: PathBuf) {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.did_remove(path);
        }
    }
}

#[async_trait::async_trait]
impl CodeIntelBackend for SwitchableCodeIntelBackend {
    fn name(&self) -> &'static str {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.name()
        } else {
            self.fallback.name()
        }
    }

    async fn search(
        &self,
        query: &str,
        path_prefix: Option<&str>,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeSearchMatch>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.search(query, path_prefix, limit, ctx).await
        } else {
            self.fallback.search(query, path_prefix, limit, ctx).await
        }
    }

    async fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeSymbol>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.workspace_symbols(query, limit, ctx).await
        } else {
            self.fallback.workspace_symbols(query, limit, ctx).await
        }
    }

    async fn document_symbols(
        &self,
        path: &Path,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeSymbol>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.document_symbols(path, limit, ctx).await
        } else {
            self.fallback.document_symbols(path, limit, ctx).await
        }
    }

    async fn definitions(
        &self,
        target: &CodeNavigationTarget,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeSymbol>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.definitions(target, limit, ctx).await
        } else {
            self.fallback.definitions(target, limit, ctx).await
        }
    }

    async fn references(
        &self,
        target: &CodeNavigationTarget,
        include_declaration: bool,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeReference>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend
                .references(target, include_declaration, limit, ctx)
                .await
        } else {
            self.fallback
                .references(target, include_declaration, limit, ctx)
                .await
        }
    }

    async fn hover(
        &self,
        target: &CodeNavigationTarget,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Option<CodeHover>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.hover(target, ctx).await
        } else {
            self.fallback.hover(target, ctx).await
        }
    }

    async fn implementations(
        &self,
        target: &CodeNavigationTarget,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeSymbol>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.implementations(target, limit, ctx).await
        } else {
            self.fallback.implementations(target, limit, ctx).await
        }
    }

    async fn call_hierarchy(
        &self,
        target: &CodeNavigationTarget,
        direction: CodeCallHierarchyDirection,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeCallHierarchyEntry>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.call_hierarchy(target, direction, limit, ctx).await
        } else {
            self.fallback
                .call_hierarchy(target, direction, limit, ctx)
                .await
        }
    }

    async fn diagnostics(
        &self,
        path: Option<&Path>,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<Vec<CodeDiagnostic>> {
        if let Some(backend) = self.managed_backend_snapshot() {
            backend.diagnostics(path, limit, ctx).await
        } else {
            self.fallback.diagnostics(path, limit, ctx).await
        }
    }
}

pub fn build_runtime_tooling(
    options: &AppOptions,
    workspace_root: &Path,
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
    skill_catalog: SkillCatalog,
) -> RuntimeTooling {
    let host_process_surfaces_allowed =
        host_process_surfaces_allowed(sandbox_policy, sandbox_status);
    let process_executor = Arc::new(SwitchableHostProcessExecutor::new(
        Arc::new(ManagedPolicyProcessExecutor::new()),
        host_process_surfaces_allowed,
    ));
    let command_hook_executor = Arc::new(SwitchableCommandHookExecutor::new(
        process_executor.clone(),
        sandbox_policy.clone(),
        host_process_surfaces_allowed,
    ));
    let command_executor: Arc<dyn CommandHookExecutor> = command_hook_executor.clone();
    let hook_runner = Arc::new(HookRunner::with_services(
        command_executor,
        Arc::new(agent::runtime::ReqwestHttpHookExecutor::default()),
        Arc::new(agent::runtime::FailClosedPromptHookEvaluator),
        Arc::new(agent::runtime::FailClosedAgentHookEvaluator),
        Arc::new(agent::runtime::DefaultWasmHookExecutor::default()),
    ));
    let mut startup_warnings = Vec::new();
    let code_intel_backend = Arc::new(SwitchableCodeIntelBackend::new(
        options,
        workspace_root,
        process_executor.clone(),
        host_process_surfaces_allowed,
        sandbox_status,
        &mut startup_warnings,
    ));
    let tools = build_builtin_tools(
        sandbox_policy,
        code_intel_backend.clone(),
        process_executor.clone(),
        skill_catalog,
    );

    RuntimeTooling {
        hook_runner,
        loop_detection_config: LoopDetectionConfig {
            enabled: true,
            ..LoopDetectionConfig::default()
        },
        process_executor,
        code_intel_backend,
        command_hook_executor,
        host_process_surfaces_allowed,
        startup_warnings,
        tools,
    }
}

pub fn register_subagent_tools(
    tools: &mut ToolRegistry,
    subagent_executor: Arc<dyn SubagentExecutor>,
    task_manager: Arc<dyn TaskManager>,
) {
    tools.register(agent::tools::TaskCreateTool::new(task_manager.clone()));
    tools.register(agent::tools::TaskGetTool::new(task_manager.clone()));
    tools.register(agent::tools::TaskListTool::new(task_manager.clone()));
    tools.register(agent::tools::TaskUpdateTool::new(task_manager.clone()));
    tools.register(agent::tools::TaskStopTool::new(task_manager));
    tools.register(agent::tools::AgentSpawnTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentSendTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentWaitTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentResumeTool::new(
        subagent_executor.clone(),
    ));
    tools.register(agent::tools::AgentListTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentCancelTool::new(subagent_executor));
}

pub fn register_monitor_tools(tools: &mut ToolRegistry, monitor_manager: Arc<dyn MonitorManager>) {
    tools.register(MonitorStartTool::new(monitor_manager.clone()));
    tools.register(MonitorListTool::new(monitor_manager.clone()));
    tools.register(MonitorStopTool::new(monitor_manager));
}

pub fn register_worktree_tools(
    tools: &mut ToolRegistry,
    worktree_manager: Arc<dyn WorktreeManager>,
) {
    tools.register(WorktreeEnterTool::new(worktree_manager.clone()));
    tools.register(WorktreeListTool::new(worktree_manager.clone()));
    tools.register(WorktreeExitTool::new(worktree_manager));
}

fn build_builtin_tools(
    sandbox_policy: &SandboxPolicy,
    code_intel_backend: Arc<SwitchableCodeIntelBackend>,
    process_executor: Arc<SwitchableHostProcessExecutor>,
    skill_catalog: SkillCatalog,
) -> ToolRegistry {
    let file_activity_backend = code_intel_backend.clone();
    let code_intel_backend: Arc<dyn CodeIntelBackend> = code_intel_backend;
    let mut tools = ToolRegistry::new();
    let discovery_registry = tools.clone();

    let file_activity_observer: Arc<dyn FileActivityObserver> = file_activity_backend;
    #[cfg(feature = "notebook-tools")]
    tools.register(NotebookReadTool::with_file_activity_observer(
        file_activity_observer.clone(),
    ));
    tools.register(ReadTool::with_file_activity_observer(
        file_activity_observer.clone(),
    ));
    tools.register(WriteTool::with_file_activity_observer(
        file_activity_observer.clone(),
    ));
    tools.register(EditTool::with_file_activity_observer(
        file_activity_observer.clone(),
    ));
    tools.register(PatchFilesTool::with_file_activity_observer(
        file_activity_observer.clone(),
    ));
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(JsReplTool::new());
    // Public web search/fetch is a first-class operator surface. Keep it
    // available by default so hosts do not silently diverge from Codex-like
    // workflows that expect live browsing without extra rebuild flags.
    tools.register(WebFetchTool::new());
    tools.register(WebSearchTool::new());
    tools.register(WebSearchBackendsTool::new());
    // `exec_command` and `write_stdin` are the only interactive process
    // surfaces now exposed by the host. Keeping one session model avoids
    // forcing TUI, approval, and provider layers to special-case legacy paths.
    tools.register(ExecCommandTool::with_process_executor_and_policy(
        process_executor.clone(),
        sandbox_policy.clone(),
    ));
    tools.register(GuardedWriteStdinTool::new(process_executor));
    tools.register(CodeSearchTool::with_backend(code_intel_backend.clone()));
    tools.register(CodeSymbolSearchTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeDocumentSymbolsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeNavTool::with_backend(code_intel_backend.clone()));
    tools.register(CodeDiagnosticsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(ToolDiscoverTool::new(discovery_registry));
    tools.register(SkillsListTool::new(skill_catalog.clone()));
    tools.register(SkillViewTool::new(skill_catalog.clone()));
    tools.register(SkillManageTool::new(skill_catalog));
    tools.register(RequestUserInputTool::new());
    tools.register(RequestPermissionsTool::new());
    tools
}

pub fn host_process_surfaces_allowed(
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
) -> bool {
    !sandbox_policy.requires_enforcement() || sandbox_status.is_available()
}

#[cfg(test)]
mod tests {
    use super::{build_runtime_tooling, host_process_surfaces_allowed, register_subagent_tools};
    use crate::options::AppOptions;
    use agent::SkillCatalog;
    use agent::tools::{
        NetworkPolicy, SandboxBackendKind, SandboxBackendStatus, SandboxPolicy, SubagentExecutor,
        SubagentInputDelivery, SubagentLaunchSpec, SubagentParentContext, TaskManager,
        ToolExecutionContext,
    };
    use agent::types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentSessionId, AgentTaskSpec, AgentWaitRequest,
        AgentWaitResponse, SessionId, TaskId, TaskRecord, TaskStatus, TaskSummaryRecord,
        ToolCallId, ToolName,
    };
    use agent_env::EnvMap;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn load_options() -> AppOptions {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        AppOptions::from_env_and_args_iter(dir.path(), &env_map, std::iter::empty::<String>())
            .unwrap()
    }

    #[test]
    fn restrictive_policies_require_a_real_backend_for_host_process_tools() {
        let restrictive = SandboxPolicy {
            network: NetworkPolicy::Off,
            ..SandboxPolicy::recommended_for_scope(&Default::default())
        };

        assert!(!host_process_surfaces_allowed(
            &restrictive,
            &SandboxBackendStatus::Unavailable {
                reason: "bwrap missing".to_string(),
            }
        ));
        assert!(host_process_surfaces_allowed(
            &restrictive,
            &SandboxBackendStatus::Available {
                kind: SandboxBackendKind::LinuxBubblewrap,
            }
        ));
    }

    #[test]
    fn permissive_policy_keeps_host_process_tools_available() {
        assert!(host_process_surfaces_allowed(
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            }
        ));
    }

    #[test]
    fn runtime_tooling_disables_host_process_helpers_when_backend_is_unavailable() {
        let mut options = load_options();
        options.lsp_enabled = true;
        let workspace = tempdir().unwrap();
        let policy = SandboxPolicy {
            network: NetworkPolicy::Off,
            ..SandboxPolicy::recommended_for_scope(&Default::default())
        };
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &policy,
            &SandboxBackendStatus::Unavailable {
                reason: "bwrap missing".to_string(),
            },
            SkillCatalog::default(),
        );

        assert!(
            tooling
                .tools
                .names()
                .into_iter()
                .any(|name| name.as_str() == "exec_command")
        );
        assert!(!tooling.host_process_surfaces_allowed);
        assert!(
            tooling
                .startup_warnings
                .iter()
                .any(|warning| warning.contains("disabled managed code-intel helpers"))
        );
    }

    #[test]
    fn runtime_tooling_keeps_web_tools_available_by_default() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );

        let tool_names = tooling.tools.names();
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "tool_discover")
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "exec_command")
        );
        assert!(tool_names.iter().any(|name| name.as_str() == "write_stdin"));
        assert!(tool_names.iter().any(|name| name.as_str() == "web_fetch"));
        assert!(tool_names.iter().any(|name| name.as_str() == "web_search"));
        assert!(tool_names.iter().any(|name| name.as_str() == "patch_files"));
        #[cfg(feature = "notebook-tools")]
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "notebook_read")
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "web_search_backends")
        );
    }

    #[cfg(feature = "notebook-tools")]
    #[test]
    fn runtime_tooling_registers_notebook_surface_when_feature_enabled() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );

        let tool_names = tooling.tools.names();
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "notebook_read")
        );
    }

    #[test]
    fn register_subagent_tools_exposes_handle_based_child_controls_only() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let mut tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );
        register_subagent_tools(
            &mut tooling.tools,
            Arc::new(NoopSubagentExecutor),
            Arc::new(NoopTaskManager),
        );

        let tool_names = tooling.tools.names();
        assert!(tool_names.iter().any(|name| name.as_str() == "task_create"));
        assert!(tool_names.iter().any(|name| name.as_str() == "task_get"));
        assert!(tool_names.iter().any(|name| name.as_str() == "task_list"));
        assert!(tool_names.iter().any(|name| name.as_str() == "task_update"));
        assert!(tool_names.iter().any(|name| name.as_str() == "task_stop"));
        assert!(tool_names.iter().any(|name| name.as_str() == "spawn_agent"));
        assert!(tool_names.iter().any(|name| name.as_str() == "send_input"));
        assert!(tool_names.iter().any(|name| name.as_str() == "wait_agent"));
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "resume_agent")
        );
        assert!(tool_names.iter().any(|name| name.as_str() == "list_agents"));
        assert!(tool_names.iter().any(|name| name.as_str() == "close_agent"));
        assert!(!tool_names.iter().any(|name| name.as_str() == "task"));
        assert!(!tool_names.iter().any(|name| name.as_str() == "task_batch"));
    }

    #[test]
    fn patch_visibility_uses_patch_files_as_the_only_runtime_patch_surface() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );

        let openai_visible = tooling
            .tools
            .specs()
            .into_iter()
            .filter(|spec| spec.is_model_visible_for_provider("openai"))
            .map(|spec| spec.name.to_string())
            .collect::<Vec<_>>();

        assert!(openai_visible.iter().any(|name| name == "patch_files"));
    }

    #[test]
    fn code_nav_visibility_uses_canonical_surface_only() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );

        let openai_visible = tooling
            .tools
            .specs()
            .into_iter()
            .filter(|spec| spec.is_model_visible_for_provider("openai"))
            .map(|spec| spec.name.to_string())
            .collect::<Vec<_>>();

        assert!(openai_visible.iter().any(|name| name == "code_nav"));
        assert!(openai_visible.iter().any(|name| name == "code_search"));
    }

    #[test]
    fn openai_visible_tool_schemas_compile_to_provider_safe_subset() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let mut tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );
        register_subagent_tools(
            &mut tooling.tools,
            Arc::new(NoopSubagentExecutor),
            Arc::new(NoopTaskManager),
        );

        let openai_specs = tooling
            .tools
            .specs()
            .into_iter()
            .filter(|spec| spec.is_model_visible_for_provider("openai"))
            .collect::<Vec<_>>();

        assert!(!openai_specs.is_empty());

        for spec in openai_specs {
            if !matches!(spec.kind, agent::types::ToolKind::Function) {
                continue;
            }

            let tool_schema = agent::provider::tool_schema(&spec);
            let parameters = &tool_schema["parameters"];
            assert_eq!(
                parameters["type"],
                Value::String("object".to_string()),
                "tool `{}` must expose object parameters to OpenAI",
                spec.name
            );
            assert_schema_is_provider_safe_subset(parameters, &spec.name.to_string());
        }
    }

    fn assert_schema_is_provider_safe_subset(value: &Value, tool_name: &str) {
        match value {
            Value::Object(map) => {
                for forbidden in ["$ref", "$defs", "definitions", "allOf", "anyOf", "oneOf"] {
                    assert!(
                        !map.contains_key(forbidden),
                        "tool `{tool_name}` leaked forbidden schema keyword `{forbidden}`: {value}"
                    );
                }
                for child in map.values() {
                    assert_schema_is_provider_safe_subset(child, tool_name);
                }
            }
            Value::Array(values) => {
                for child in values {
                    assert_schema_is_provider_safe_subset(child, tool_name);
                }
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn write_stdin_is_blocked_when_host_process_surfaces_turn_off_mid_session() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );
        let exec_tool = tooling.tools.get("exec_command").expect("exec tool");
        let ctx = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            effective_sandbox_policy: Some(SandboxPolicy::permissive()),
            ..Default::default()
        };
        let result = exec_tool
            .execute(
                ToolCallId::new(),
                json!({"cmd": "cat", "yield_time_ms": 10}),
                &ctx,
            )
            .await
            .expect("start exec session");
        let session_id = result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("session_id"))
            .and_then(Value::as_str)
            .expect("exec session id")
            .to_string();

        tooling.process_executor.set_host_process_surfaces(false);
        let write_tool = tooling.tools.get("write_stdin").expect("write tool");
        let error = write_tool
            .execute(
                ToolCallId::new(),
                json!({"session_id": session_id, "chars": "hello"}),
                &ctx,
            )
            .await
            .expect_err("write_stdin should fail closed once host process surfaces are disabled");
        assert!(
            error
                .to_string()
                .contains("host subprocess surfaces are disabled")
        );

        tooling.process_executor.set_host_process_surfaces(true);
        write_tool
            .execute(
                ToolCallId::new(),
                json!({"session_id": session_id, "close_stdin": true, "yield_time_ms": 10}),
                &ctx,
            )
            .await
            .expect("cleanup exec session");
    }

    #[tokio::test]
    async fn child_tool_snapshots_share_runtime_host_process_gate() {
        let options = load_options();
        let workspace = tempdir().unwrap();
        let tooling = build_runtime_tooling(
            &options,
            workspace.path(),
            &SandboxPolicy::permissive(),
            &SandboxBackendStatus::Unavailable {
                reason: "not needed".to_string(),
            },
            SkillCatalog::default(),
        );
        let child_tools = tooling.tools.filtered_by_names(&[
            ToolName::from("exec_command"),
            ToolName::from("write_stdin"),
        ]);
        let ctx = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            effective_sandbox_policy: Some(SandboxPolicy::permissive()),
            ..Default::default()
        };
        let exec_tool = child_tools.get("exec_command").expect("exec tool");
        let result = exec_tool
            .execute(
                ToolCallId::new(),
                json!({"cmd": "cat", "yield_time_ms": 10}),
                &ctx,
            )
            .await
            .expect("start child exec session");
        let session_id = result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("session_id"))
            .and_then(Value::as_str)
            .expect("exec session id")
            .to_string();

        tooling.process_executor.set_host_process_surfaces(false);
        let write_tool = child_tools.get("write_stdin").expect("write tool");
        let error = write_tool
            .execute(
                ToolCallId::new(),
                json!({"session_id": session_id, "chars": "hello"}),
                &ctx,
            )
            .await
            .expect_err("child snapshot should observe revoked host process access");
        assert!(
            error
                .to_string()
                .contains("host subprocess surfaces are disabled")
        );

        tooling.process_executor.set_host_process_surfaces(true);
        write_tool
            .execute(
                ToolCallId::new(),
                json!({"session_id": session_id, "close_stdin": true, "yield_time_ms": 10}),
                &ctx,
            )
            .await
            .expect("cleanup child exec session");
    }

    struct NoopSubagentExecutor;

    struct NoopTaskManager;

    #[async_trait]
    impl SubagentExecutor for NoopSubagentExecutor {
        async fn spawn(
            &self,
            _parent: SubagentParentContext,
            _tasks: Vec<SubagentLaunchSpec>,
        ) -> agent::tools::Result<Vec<AgentHandle>> {
            Err(agent::tools::ToolError::invalid_state(
                "test executor does not support spawn",
            ))
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _message: agent::types::Message,
            _delivery: SubagentInputDelivery,
        ) -> agent::tools::Result<AgentHandle> {
            Err(agent::tools::ToolError::invalid_state(
                "test executor does not support send",
            ))
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            _request: AgentWaitRequest,
        ) -> agent::tools::Result<AgentWaitResponse> {
            Ok(AgentWaitResponse {
                completed: Vec::new(),
                pending: Vec::new(),
                results: Vec::<AgentResultEnvelope>::new(),
            })
        }

        async fn resume(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
        ) -> agent::tools::Result<AgentHandle> {
            Err(agent::tools::ToolError::invalid_state(
                "test executor does not support resume",
            ))
        }

        async fn list(
            &self,
            _parent: SubagentParentContext,
        ) -> agent::tools::Result<Vec<AgentHandle>> {
            Ok(Vec::new())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _reason: Option<String>,
        ) -> agent::tools::Result<AgentHandle> {
            Err(agent::tools::ToolError::invalid_state(
                "test executor does not support cancel",
            ))
        }
    }

    #[async_trait]
    impl TaskManager for NoopTaskManager {
        async fn create_task(
            &self,
            _parent: SubagentParentContext,
            task: AgentTaskSpec,
            status: TaskStatus,
        ) -> agent::tools::Result<TaskRecord> {
            Ok(TaskRecord {
                summary: TaskSummaryRecord {
                    task_id: task.task_id.clone(),
                    session_id: SessionId::from("session_root"),
                    agent_session_id: AgentSessionId::from("agent_session_root"),
                    role: task.role.clone(),
                    origin: task.origin,
                    status,
                    parent_agent_id: None,
                    child_agent_id: None,
                    summary: Some(task.prompt.clone()),
                    worktree_id: None,
                    worktree_root: None,
                },
                spec: task,
                claimed_files: Vec::new(),
                result: None,
                error: None,
            })
        }

        async fn get_task(
            &self,
            _parent: SubagentParentContext,
            task_id: &TaskId,
        ) -> agent::tools::Result<TaskRecord> {
            Ok(TaskRecord {
                summary: TaskSummaryRecord {
                    task_id: task_id.clone(),
                    session_id: SessionId::from("session_root"),
                    agent_session_id: AgentSessionId::from("agent_session_root"),
                    role: "reviewer".to_string(),
                    origin: agent::types::TaskOrigin::AgentCreated,
                    status: TaskStatus::Open,
                    parent_agent_id: None,
                    child_agent_id: None,
                    summary: Some("task".to_string()),
                    worktree_id: None,
                    worktree_root: None,
                },
                spec: AgentTaskSpec {
                    task_id: task_id.clone(),
                    role: "reviewer".to_string(),
                    prompt: "task".to_string(),
                    origin: agent::types::TaskOrigin::AgentCreated,
                    steer: None,
                    allowed_tools: Vec::new(),
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                },
                claimed_files: Vec::new(),
                result: None,
                error: None,
            })
        }

        async fn list_tasks(
            &self,
            _parent: SubagentParentContext,
            _include_closed: bool,
        ) -> agent::tools::Result<Vec<TaskSummaryRecord>> {
            Ok(Vec::new())
        }

        async fn update_task(
            &self,
            parent: SubagentParentContext,
            task_id: TaskId,
            status: Option<TaskStatus>,
            summary: Option<String>,
        ) -> agent::tools::Result<TaskRecord> {
            let mut record = self.get_task(parent, &task_id).await?;
            if let Some(status) = status {
                record.summary.status = status;
            }
            if let Some(summary) = summary {
                record.summary.summary = Some(summary);
            }
            Ok(record)
        }

        async fn stop_task(
            &self,
            parent: SubagentParentContext,
            task_id: TaskId,
            reason: Option<String>,
        ) -> agent::tools::Result<TaskRecord> {
            let mut record = self.get_task(parent, &task_id).await?;
            record.summary.status = TaskStatus::Cancelled;
            record.error = reason;
            Ok(record)
        }
    }
}
