use crate::options::AppOptions;
use agent::runtime::{
    CommandHookExecutor, DefaultCommandHookExecutor, HookRunner, LoopDetectionConfig,
};
use agent::tools::{SandboxBackendStatus, SubagentExecutor};
use agent::{
    BashTool, CodeDefinitionsTool, CodeDocumentSymbolsTool, CodeIntelBackend, CodeReferencesTool,
    CodeSymbolSearchTool, EditTool, GlobTool, GrepTool, JsReplTool, ListTool,
    ManagedCodeIntelBackend, ManagedCodeIntelOptions, ManagedPolicyProcessExecutor, PatchTool,
    PlanState, ReadTool, RequestUserInputTool, SandboxPolicy, TaskTool, ToolRegistry,
    UpdatePlanTool, WebFetchTool, WebSearchBackendsTool, WebSearchTool,
    WorkspaceTextCodeIntelBackend, WriteTool,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

pub(crate) struct RuntimeTooling {
    pub(crate) hook_runner: Arc<HookRunner>,
    pub(crate) loop_detection_config: LoopDetectionConfig,
    pub(crate) process_executor: Arc<ManagedPolicyProcessExecutor>,
    pub(crate) host_process_surfaces_allowed: bool,
    pub(crate) startup_warnings: Vec<String>,
    pub(crate) tools: ToolRegistry,
}

pub(crate) fn build_runtime_tooling(
    options: &AppOptions,
    workspace_root: &Path,
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
) -> RuntimeTooling {
    let process_executor = Arc::new(ManagedPolicyProcessExecutor::new());
    let host_process_surfaces_allowed =
        host_process_surfaces_allowed(sandbox_policy, sandbox_status);
    let command_executor: Arc<dyn CommandHookExecutor> = if host_process_surfaces_allowed {
        Arc::new(
            DefaultCommandHookExecutor::with_process_executor_and_policy(
                BTreeMap::new(),
                process_executor.clone(),
                sandbox_policy.clone(),
            ),
        )
    } else {
        // Startup may continue without sandbox enforcement after an explicit
        // operator override, but command hooks still need to fail closed so
        // they never widen into silent host execution.
        Arc::new(DefaultCommandHookExecutor::default())
    };
    let hook_runner = Arc::new(HookRunner::with_services(
        command_executor,
        Arc::new(agent::runtime::ReqwestHttpHookExecutor::default()),
        Arc::new(agent::runtime::FailClosedPromptHookEvaluator),
        Arc::new(agent::runtime::FailClosedAgentHookEvaluator),
        Arc::new(agent::runtime::DefaultWasmHookExecutor::default()),
    ));
    let (tools, startup_warnings) = build_builtin_tools(
        options,
        workspace_root,
        sandbox_policy,
        sandbox_status,
        &process_executor,
    );

    RuntimeTooling {
        hook_runner,
        loop_detection_config: LoopDetectionConfig {
            enabled: true,
            ..LoopDetectionConfig::default()
        },
        process_executor,
        host_process_surfaces_allowed,
        startup_warnings,
        tools,
    }
}

pub(crate) fn register_subagent_tools(
    tools: &mut ToolRegistry,
    subagent_executor: Arc<dyn SubagentExecutor>,
) {
    tools.register(TaskTool::new(subagent_executor.clone()));
    tools.register(agent::tools::TaskBatchTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentSpawnTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentSendTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentWaitTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentListTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentCancelTool::new(subagent_executor));
}

fn build_builtin_tools(
    options: &AppOptions,
    workspace_root: &Path,
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
    process_executor: &Arc<ManagedPolicyProcessExecutor>,
) -> (ToolRegistry, Vec<String>) {
    let host_process_surfaces_allowed =
        host_process_surfaces_allowed(sandbox_policy, sandbox_status);
    let mut startup_warnings = Vec::new();
    let managed_code_intel = build_managed_code_intel(
        options,
        workspace_root,
        process_executor,
        host_process_surfaces_allowed,
        sandbox_status,
        &mut startup_warnings,
    );
    let code_intel_backend: Arc<dyn CodeIntelBackend> = managed_code_intel
        .clone()
        .map(|backend| backend as Arc<dyn CodeIntelBackend>)
        .unwrap_or_else(|| Arc::new(WorkspaceTextCodeIntelBackend::new()));
    let plan_state = PlanState::default();
    let mut tools = ToolRegistry::new();

    if let Some(observer) = managed_code_intel {
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
    tools.register(JsReplTool::new());
    // Public web search/fetch is a first-class operator surface. Keep it
    // available by default so hosts do not silently diverge from Codex-like
    // workflows that expect live browsing without extra rebuild flags.
    tools.register(WebFetchTool::new());
    tools.register(WebSearchTool::new());
    tools.register(WebSearchBackendsTool::new());
    if host_process_surfaces_allowed {
        tools.register(BashTool::with_process_executor_and_policy(
            process_executor.clone(),
            sandbox_policy.clone(),
        ));
    } else if let Some(reason) = sandbox_status.reason() {
        // File tools still enforce workspace/protected-path policy in-process,
        // but exposing a model-driven shell would silently widen to full host
        // execution when the enforcing backend is missing.
        warn!(
            "sandbox enforcement backend unavailable; disabling bash tool to avoid host fallback: {reason}"
        );
        startup_warnings.push(format!(
            "sandbox backend unavailable; disabled bash tool to avoid host subprocess execution: {reason}"
        ));
    }
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
    tools.register(UpdatePlanTool::new(plan_state));
    tools.register(RequestUserInputTool::new());
    (tools, startup_warnings)
}

fn build_managed_code_intel(
    options: &AppOptions,
    workspace_root: &Path,
    process_executor: &Arc<ManagedPolicyProcessExecutor>,
    host_process_surfaces_allowed: bool,
    sandbox_status: &SandboxBackendStatus,
    startup_warnings: &mut Vec<String>,
) -> Option<Arc<ManagedCodeIntelBackend>> {
    if options.lsp_enabled && !host_process_surfaces_allowed {
        if let Some(reason) = sandbox_status.reason() {
            warn!(
                "sandbox enforcement backend unavailable; disabling managed code-intel helpers to avoid host fallback: {reason}"
            );
            startup_warnings.push(format!(
                "sandbox backend unavailable; disabled managed code-intel helpers to avoid host subprocess execution: {reason}"
            ));
        }
        return None;
    }
    // Managed LSP helpers run outside the normal foreground tool approval path.
    // Boot keeps that policy decision local so future frontends inherit the
    // same helper behavior without duplicating host wiring rules.
    options.lsp_enabled.then(|| {
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
    })
}

pub(crate) fn host_process_surfaces_allowed(
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
) -> bool {
    !sandbox_policy.requires_enforcement() || sandbox_status.is_available()
}

#[cfg(test)]
mod tests {
    use super::{build_runtime_tooling, host_process_surfaces_allowed};
    use crate::options::AppOptions;
    use agent::tools::{NetworkPolicy, SandboxBackendKind, SandboxBackendStatus, SandboxPolicy};
    use agent_env::EnvMap;
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
        );

        assert!(
            !tooling
                .tools
                .names()
                .into_iter()
                .any(|name| name.as_str() == "bash")
        );
        assert!(!tooling.host_process_surfaces_allowed);
        assert!(
            tooling
                .startup_warnings
                .iter()
                .any(|warning| warning.contains("disabled bash tool"))
        );
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
        );

        let tool_names = tooling.tools.names();
        assert!(tool_names.iter().any(|name| name.as_str() == "web_fetch"));
        assert!(tool_names.iter().any(|name| name.as_str() == "web_search"));
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "web_search_backends")
        );
    }
}
