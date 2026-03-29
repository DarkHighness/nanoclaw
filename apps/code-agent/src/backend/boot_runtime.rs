use crate::options::AppOptions;
use agent::runtime::{DefaultCommandHookExecutor, HookRunner, LoopDetectionConfig};
use agent::tools::SubagentExecutor;
use agent::{
    BashTool, CodeDefinitionsTool, CodeDocumentSymbolsTool, CodeIntelBackend, CodeReferencesTool,
    CodeSymbolSearchTool, EditTool, GlobTool, GrepTool, ListTool, ManagedCodeIntelBackend,
    ManagedCodeIntelOptions, ManagedPolicyProcessExecutor, PatchTool, ReadTool, SandboxPolicy,
    TaskTool, TodoListState, TodoReadTool, TodoWriteTool, ToolRegistry,
    WorkspaceTextCodeIntelBackend, WriteTool,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

pub(crate) struct RuntimeTooling {
    pub(crate) hook_runner: Arc<HookRunner>,
    pub(crate) loop_detection_config: LoopDetectionConfig,
    pub(crate) process_executor: Arc<ManagedPolicyProcessExecutor>,
    pub(crate) tools: ToolRegistry,
}

pub(crate) fn build_runtime_tooling(
    options: &AppOptions,
    workspace_root: &Path,
    sandbox_policy: &SandboxPolicy,
) -> RuntimeTooling {
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
    let tools = build_builtin_tools(options, workspace_root, sandbox_policy, &process_executor);

    RuntimeTooling {
        hook_runner,
        loop_detection_config: LoopDetectionConfig {
            enabled: true,
            ..LoopDetectionConfig::default()
        },
        process_executor,
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
    process_executor: &Arc<ManagedPolicyProcessExecutor>,
) -> ToolRegistry {
    let managed_code_intel = build_managed_code_intel(options, workspace_root, process_executor);
    let code_intel_backend: Arc<dyn CodeIntelBackend> = managed_code_intel
        .clone()
        .map(|backend| backend as Arc<dyn CodeIntelBackend>)
        .unwrap_or_else(|| Arc::new(WorkspaceTextCodeIntelBackend::new()));
    let todo_state = TodoListState::default();
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
    tools
}

fn build_managed_code_intel(
    options: &AppOptions,
    workspace_root: &Path,
    process_executor: &Arc<ManagedPolicyProcessExecutor>,
) -> Option<Arc<ManagedCodeIntelBackend>> {
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
