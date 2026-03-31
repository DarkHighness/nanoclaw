use crate::options::AppOptions;
use agent::runtime::{
    CommandHookExecutor, DefaultCommandHookExecutor, HookRunner, LoopDetectionConfig,
};
use agent::tools::{SandboxBackendStatus, SubagentExecutor};
use agent::{
    ApplyPatchTool, CodeCallHierarchyTool, CodeDefinitionsTool, CodeDocumentSymbolsTool,
    CodeHoverTool, CodeImplementationsTool, CodeIntelBackend, CodeReferencesTool,
    CodeSymbolSearchTool, EditTool, ExecCommandTool, ExecutionState, GlobTool, GrepTool,
    JsReplTool, ListTool, ManagedCodeIntelBackend, ManagedCodeIntelOptions,
    ManagedPolicyProcessExecutor, PatchTool, PlanState, ReadTool, RequestPermissionsTool,
    RequestUserInputTool, SandboxPolicy, SkillCatalog, SkillTool, TaskTool, ToolRegistry,
    ToolSearchTool, ToolSuggestTool, UpdateExecutionTool, UpdatePlanTool, ViewImageTool,
    WebFetchTool, WebSearchBackendsTool, WebSearchTool, WorkspaceTextCodeIntelBackend,
    WriteStdinTool, WriteTool,
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
    skill_catalog: SkillCatalog,
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
        skill_catalog,
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
    tools.register(agent::tools::AgentResumeTool::new(
        subagent_executor.clone(),
    ));
    tools.register(agent::tools::AgentListTool::new(subagent_executor.clone()));
    tools.register(agent::tools::AgentCancelTool::new(subagent_executor));
}

fn build_builtin_tools(
    options: &AppOptions,
    workspace_root: &Path,
    sandbox_policy: &SandboxPolicy,
    sandbox_status: &SandboxBackendStatus,
    process_executor: &Arc<ManagedPolicyProcessExecutor>,
    skill_catalog: SkillCatalog,
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
    let execution_state = ExecutionState::default();
    let mut tools = ToolRegistry::new();
    let discovery_registry = tools.clone();

    if let Some(observer) = managed_code_intel {
        tools.register(ReadTool::with_file_activity_observer(observer.clone()));
        tools.register(ViewImageTool::with_file_activity_observer(observer.clone()));
        tools.register(WriteTool::with_file_activity_observer(observer.clone()));
        tools.register(EditTool::with_file_activity_observer(observer.clone()));
        tools.register(ApplyPatchTool::with_file_activity_observer(
            observer.clone(),
        ));
        tools.register(PatchTool::with_file_activity_observer(observer));
    } else {
        tools.register(ReadTool::new());
        tools.register(ViewImageTool::new());
        tools.register(WriteTool::new());
        tools.register(EditTool::new());
        tools.register(ApplyPatchTool::new());
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
    // `exec_command` and `write_stdin` are the only interactive process
    // surfaces now exposed by the host. Keeping one session model avoids
    // forcing TUI, approval, and provider layers to special-case legacy paths.
    tools.register(ExecCommandTool::with_process_executor_and_policy(
        process_executor.clone(),
        sandbox_policy.clone(),
    ));
    tools.register(WriteStdinTool::new());
    tools.register(CodeSymbolSearchTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeDocumentSymbolsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeDefinitionsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeReferencesTool::with_backend(code_intel_backend.clone()));
    tools.register(CodeHoverTool::with_backend(code_intel_backend.clone()));
    tools.register(CodeImplementationsTool::with_backend(
        code_intel_backend.clone(),
    ));
    tools.register(CodeCallHierarchyTool::with_backend(code_intel_backend));
    tools.register(ToolSearchTool::new(discovery_registry.clone()));
    tools.register(ToolSuggestTool::new(discovery_registry));
    tools.register(UpdatePlanTool::new(plan_state));
    tools.register(UpdateExecutionTool::new(execution_state));
    tools.register(SkillTool::new(skill_catalog));
    tools.register(RequestUserInputTool::new());
    tools.register(RequestPermissionsTool::new());
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
    use super::{build_runtime_tooling, host_process_surfaces_allowed, register_subagent_tools};
    use crate::options::AppOptions;
    use agent::SkillCatalog;
    use agent::tools::{
        NetworkPolicy, SandboxBackendKind, SandboxBackendStatus, SandboxPolicy, SubagentExecutor,
        SubagentInputDelivery, SubagentLaunchSpec, SubagentParentContext,
    };
    use agent::types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentWaitRequest, AgentWaitResponse,
    };
    use agent_env::EnvMap;
    use async_trait::async_trait;
    use serde_json::Value;
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
        assert!(tool_names.iter().any(|name| name.as_str() == "view_image"));
        assert!(tool_names.iter().any(|name| name.as_str() == "tool_search"));
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "tool_suggest")
        );
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "exec_command")
        );
        assert!(tool_names.iter().any(|name| name.as_str() == "write_stdin"));
        assert!(tool_names.iter().any(|name| name.as_str() == "web_fetch"));
        assert!(tool_names.iter().any(|name| name.as_str() == "web_search"));
        assert!(
            tool_names
                .iter()
                .any(|name| name.as_str() == "web_search_backends")
        );
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
        register_subagent_tools(&mut tooling.tools, Arc::new(NoopSubagentExecutor));

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

    struct NoopSubagentExecutor;

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
}
