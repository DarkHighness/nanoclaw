use super::{
    CodeAgentSession, CompactionWorkingSnapshot, LiveTaskAttentionAction, LiveTaskSummary,
    LiveTaskWaitOutcome, PERMISSION_MODE_SWITCH_BLOCKED_WHILE_TURN_RUNNING,
    STDIO_MCP_DISABLED_WARNING_PREFIX, SessionMemoryRefreshContext, SessionOperation,
    SessionOperationAction, SessionStartupSnapshot, SideQuestionContextSnapshot,
};
use crate::backend::boot_runtime::{
    COMMAND_HOOK_DISABLED_WARNING_PREFIX, MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX,
    SwitchableCodeIntelBackend, SwitchableCommandHookExecutor, SwitchableHostProcessExecutor,
};
use crate::backend::{
    ApprovalCoordinator, McpServerSummary, PermissionRequestCoordinator, SessionEventStream,
    SessionMonitorManager, StartupDiagnosticsSnapshot, UserInputCoordinator, list_mcp_servers,
};
use crate::display::TuiDisplayConfig;
use crate::interaction::{PendingControlKind, PendingControlReason, SessionPermissionMode};
use crate::motion::TuiMotionConfig;
use crate::statusline::StatusLineConfig;
use agent::mcp::{
    ConnectedMcpServer, McpCatalog, McpResource, McpResourceTemplate, McpServerConfig,
    McpTransportConfig, MockMcpClient, catalog_resource_tools_as_registry_entries,
    catalog_tools_as_registry_entries,
};
use agent::memory::{MemoryBackend, MemoryCoreBackend};
use agent::runtime::{
    CompactionConfig, CompactionRequest, CompactionResult, ConversationCompactor, HookRunner,
    ModelBackend, PermissionGrantStore, Result as RuntimeResult,
};
use agent::tools::{
    CheckpointFileMutation, CheckpointHandler, CheckpointMutationRequest, ExecCommandTool,
    GrantedPermissionResponse, HOST_FEATURE_HOST_PROCESS_SURFACES,
    HOST_FEATURE_REQUEST_PERMISSIONS, PermissionGrantScope, PermissionRequest,
    PermissionRequestHandler, Result as ToolResult, SubagentExecutor, SubagentInputDelivery,
    SubagentLaunchSpec, SubagentParentContext, ToolError, ToolExecutionContext, ToolRegistry,
    WriteStdinTool,
};
use agent::types::{
    AgentHandle, AgentId, AgentResultEnvelope, AgentSessionId, AgentStatus, AgentTaskSpec,
    AgentWaitRequest, AgentWaitResponse, CheckpointRestoreMode, CommandHookHandler,
    DynamicToolSpec, HookEvent, HookHandler, HookRegistration, McpToolBoundary, McpTransportKind,
    Message, MessageId, MessagePart, MessageRole, ModelEvent, ModelRequest, SessionEventEnvelope,
    SessionEventKind, SessionId, SubmittedPromptSnapshot, TaskId, TaskOrigin, TaskStatus,
    ToolAvailability, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec, ToolVisibilityContext,
};
use agent::{AgentRuntimeBuilder, RequestPermissionsTool, RuntimeCommand, SkillCatalog};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};
use store::{EventSink, InMemorySessionStore, SessionStore};
use tokio::sync::Semaphore;
use tokio::time::{Duration, timeout};

struct NeverBackend;

#[async_trait]
impl ModelBackend for NeverBackend {
    async fn stream_turn(
        &self,
        _request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        unreachable!("session-start tests never execute model turns")
    }
}

struct StreamingTextBackend;

struct StaticCompactor;

#[derive(Clone, Default)]
struct PermissionSurfaceBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl PermissionSurfaceBackend {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelBackend for StreamingTextBackend {
    async fn stream_turn(
        &self,
        _request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        Ok(stream::iter(vec![Ok(ModelEvent::ResponseComplete {
            stop_reason: Some("stop".to_string()),
            message_id: None,
            continuation: None,
            usage: None,
            reasoning: Vec::new(),
        })])
        .boxed())
    }
}

#[async_trait]
impl ModelBackend for PermissionSurfaceBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        self.requests.lock().unwrap().push(request.clone());
        let completed_tools = request
            .messages
            .iter()
            .flat_map(|message| message.parts.iter())
            .filter_map(|part| match part {
                MessagePart::ToolResult { result } => Some(result.tool_name.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();

        if !completed_tools
            .iter()
            .any(|name| name == "request_permissions")
        {
            let call = agent::types::ToolCall {
                id: agent::types::ToolCallId::new(),
                call_id: "call-request-permissions".into(),
                tool_name: "request_permissions".into(),
                arguments: json!({
                    "reason": "need write access for the next tool call",
                    "permissions": {
                        "file_system": {
                            "write": ["granted"]
                        }
                    }
                }),
                origin: ToolOrigin::Local,
            };
            return Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed());
        }

        if !completed_tools.iter().any(|name| name == "inspect_policy") {
            let call = agent::types::ToolCall {
                id: agent::types::ToolCallId::new(),
                call_id: "call-inspect-policy".into(),
                tool_name: "inspect_policy".into(),
                arguments: json!({}),
                origin: ToolOrigin::Local,
            };
            return Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed());
        }

        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: "done".to_string(),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                usage: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[async_trait]
impl ConversationCompactor for StaticCompactor {
    async fn compact(&self, request: CompactionRequest) -> RuntimeResult<CompactionResult> {
        Ok(CompactionResult {
            summary: format!("summary for {} messages", request.messages.len()),
        })
    }
}

#[derive(Clone, Default)]
struct RecordingPromptBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingPromptBackend {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelBackend for RecordingPromptBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        Ok(stream::iter(vec![Ok(ModelEvent::ResponseComplete {
            stop_reason: Some("stop".to_string()),
            message_id: None,
            continuation: None,
            usage: None,
            reasoning: Vec::new(),
        })])
        .boxed())
    }
}

#[derive(Clone)]
struct ScriptedTextBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    responses: Arc<Mutex<Vec<String>>>,
}

impl ScriptedTextBackend {
    fn new(responses: Vec<String>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses)),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelBackend for ScriptedTextBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        let response = self
            .responses
            .lock()
            .unwrap()
            .drain(..1)
            .next()
            .expect("scripted text backend response");
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta { delta: response }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                usage: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[derive(Clone)]
struct GatedTextBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    gate: Arc<Semaphore>,
    response: Arc<Mutex<Option<String>>>,
}

impl GatedTextBackend {
    fn new(response: &str) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            gate: Arc::new(Semaphore::new(0)),
            response: Arc::new(Mutex::new(Some(response.to_string()))),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn release(&self) {
        self.gate.add_permits(1);
    }
}

#[async_trait]
impl ModelBackend for GatedTextBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        let _permit = self.gate.acquire().await.unwrap();
        let response = self
            .response
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| "# Current State\n\nreleased".to_string());
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta { delta: response }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                usage: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

struct GrantRequestedPermissionsHandler;

#[async_trait]
impl PermissionRequestHandler for GrantRequestedPermissionsHandler {
    async fn request_permissions(
        &self,
        request: PermissionRequest,
    ) -> ToolResult<GrantedPermissionResponse> {
        Ok(GrantedPermissionResponse {
            permissions: request.permissions,
            scope: PermissionGrantScope::Turn,
        })
    }
}

struct NoopSubagentExecutor;

#[async_trait]
impl SubagentExecutor for NoopSubagentExecutor {
    async fn spawn(
        &self,
        _parent: SubagentParentContext,
        _tasks: Vec<SubagentLaunchSpec>,
    ) -> ToolResult<Vec<AgentHandle>> {
        Err(ToolError::invalid_state(
            "test executor does not support spawn",
        ))
    }

    async fn send(
        &self,
        _parent: SubagentParentContext,
        _agent_id: AgentId,
        _message: agent::types::Message,
        _delivery: SubagentInputDelivery,
    ) -> ToolResult<AgentHandle> {
        Err(ToolError::invalid_state(
            "test executor does not support send",
        ))
    }

    async fn wait(
        &self,
        _parent: SubagentParentContext,
        _request: AgentWaitRequest,
    ) -> ToolResult<AgentWaitResponse> {
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
    ) -> ToolResult<AgentHandle> {
        Err(ToolError::invalid_state(
            "test executor does not support resume",
        ))
    }

    async fn list(&self, _parent: SubagentParentContext) -> ToolResult<Vec<AgentHandle>> {
        Ok(Vec::new())
    }

    async fn cancel(
        &self,
        _parent: SubagentParentContext,
        _agent_id: AgentId,
        _reason: Option<String>,
    ) -> ToolResult<AgentHandle> {
        Err(ToolError::invalid_state(
            "test executor does not support cancel",
        ))
    }
}

struct RecordingSubagentExecutor {
    handles: Mutex<Vec<AgentHandle>>,
    spawned_tasks: Mutex<Vec<AgentTaskSpec>>,
    spawn_parents: Mutex<Vec<SubagentParentContext>>,
    sent_messages: Mutex<Vec<(AgentId, SubagentInputDelivery, agent::types::Message)>>,
    wait_response: Mutex<Option<AgentWaitResponse>>,
}

impl RecordingSubagentExecutor {
    fn new(handles: Vec<AgentHandle>) -> Self {
        Self {
            handles: Mutex::new(handles),
            spawned_tasks: Mutex::new(Vec::new()),
            spawn_parents: Mutex::new(Vec::new()),
            sent_messages: Mutex::new(Vec::new()),
            wait_response: Mutex::new(None),
        }
    }

    fn with_wait_response(handles: Vec<AgentHandle>, wait_response: AgentWaitResponse) -> Self {
        Self {
            handles: Mutex::new(handles),
            spawned_tasks: Mutex::new(Vec::new()),
            spawn_parents: Mutex::new(Vec::new()),
            sent_messages: Mutex::new(Vec::new()),
            wait_response: Mutex::new(Some(wait_response)),
        }
    }
}

#[async_trait]
impl SubagentExecutor for RecordingSubagentExecutor {
    async fn spawn(
        &self,
        parent: SubagentParentContext,
        tasks: Vec<SubagentLaunchSpec>,
    ) -> ToolResult<Vec<AgentHandle>> {
        self.spawn_parents.lock().unwrap().push(parent);
        self.spawned_tasks
            .lock()
            .unwrap()
            .extend(tasks.iter().map(|launch| launch.task.clone()));
        let mut handles = self.handles.lock().unwrap();
        let mut spawned = Vec::with_capacity(tasks.len());
        for launch in tasks {
            let task = launch.task;
            let handle = AgentHandle {
                agent_id: AgentId::from(format!("agent-{}", task.task_id)),
                parent_agent_id: None,
                session_id: SessionId::from(format!("session-{}", task.task_id)),
                agent_session_id: agent::types::AgentSessionId::from(format!(
                    "agent-session-{}",
                    task.task_id
                )),
                task_id: task.task_id.clone(),
                role: task.role.clone(),
                status: AgentStatus::Queued,
                worktree_id: None,
                worktree_root: None,
            };
            handles.push(handle.clone());
            spawned.push(handle);
        }
        Ok(spawned)
    }

    async fn send(
        &self,
        _parent: SubagentParentContext,
        agent_id: AgentId,
        message: agent::types::Message,
        delivery: SubagentInputDelivery,
    ) -> ToolResult<AgentHandle> {
        let handle = self
            .handles
            .lock()
            .unwrap()
            .iter()
            .find(|handle| handle.agent_id == agent_id)
            .cloned()
            .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
        self.sent_messages
            .lock()
            .unwrap()
            .push((agent_id, delivery, message));
        Ok(handle)
    }

    async fn wait(
        &self,
        _parent: SubagentParentContext,
        _request: AgentWaitRequest,
    ) -> ToolResult<AgentWaitResponse> {
        Ok(self
            .wait_response
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(AgentWaitResponse {
                completed: Vec::new(),
                pending: Vec::new(),
                results: Vec::new(),
            }))
    }

    async fn resume(
        &self,
        _parent: SubagentParentContext,
        agent_id: AgentId,
    ) -> ToolResult<AgentHandle> {
        let mut handles = self.handles.lock().unwrap();
        let handle = handles
            .iter_mut()
            .find(|handle| handle.agent_id == agent_id)
            .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
        handle.status = AgentStatus::Queued;
        Ok(handle.clone())
    }

    async fn list(&self, _parent: SubagentParentContext) -> ToolResult<Vec<AgentHandle>> {
        Ok(self.handles.lock().unwrap().clone())
    }

    async fn cancel(
        &self,
        _parent: SubagentParentContext,
        agent_id: AgentId,
        _reason: Option<String>,
    ) -> ToolResult<AgentHandle> {
        let mut handles = self.handles.lock().unwrap();
        let handle = handles
            .iter_mut()
            .find(|handle| handle.agent_id == agent_id)
            .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
        if !handle.status.is_terminal() {
            handle.status = agent::types::AgentStatus::Cancelled;
        }
        Ok(handle.clone())
    }
}

fn startup_snapshot(workspace_root: &std::path::Path) -> SessionStartupSnapshot {
    SessionStartupSnapshot {
        workspace_name: "workspace".to_string(),
        workspace_root: workspace_root.to_path_buf(),
        active_session_ref: "session-active".to_string(),
        root_agent_session_id: "agent-session-active".to_string(),
        provider_label: "provider".to_string(),
        model: "model".to_string(),
        model_reasoning_effort: Some("medium".to_string()),
        supported_model_reasoning_efforts: vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ],
        supports_image_input: false,
        tool_names: Vec::new(),
        store_label: "memory".to_string(),
        store_warning: None,
        stored_session_count: 0,
        default_sandbox_summary: "workspace-write".to_string(),
        sandbox_summary: "workspace-write".to_string(),
        permission_mode: SessionPermissionMode::Default,
        host_process_surfaces_allowed: true,
        startup_diagnostics: StartupDiagnosticsSnapshot::default(),
        display: TuiDisplayConfig::default(),
        statusline: StatusLineConfig::default(),
        motion: TuiMotionConfig::default(),
    }
}

fn build_session(
    runtime: agent::AgentRuntime,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    startup: SessionStartupSnapshot,
) -> CodeAgentSession {
    build_session_with_backends(
        runtime,
        subagent_executor,
        store,
        startup,
        Vec::new(),
        Vec::new(),
        None,
        None,
    )
}

fn build_session_with_memory(
    runtime: agent::AgentRuntime,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    startup: SessionStartupSnapshot,
    memory_backend: Option<Arc<dyn MemoryBackend>>,
) -> CodeAgentSession {
    build_session_with_backends(
        runtime,
        subagent_executor,
        store,
        startup,
        Vec::new(),
        Vec::new(),
        memory_backend,
        None,
    )
}

fn build_session_with_mcp(
    runtime: agent::AgentRuntime,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    startup: SessionStartupSnapshot,
    mcp_servers: Vec<ConnectedMcpServer>,
    configured_mcp_servers: Vec<McpServerConfig>,
) -> CodeAgentSession {
    build_session_with_backends(
        runtime,
        subagent_executor,
        store,
        startup,
        mcp_servers,
        configured_mcp_servers,
        None,
        None,
    )
}

fn build_session_with_backends(
    runtime: agent::AgentRuntime,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    startup: SessionStartupSnapshot,
    mcp_servers: Vec<ConnectedMcpServer>,
    configured_mcp_servers: Vec<McpServerConfig>,
    memory_backend: Option<Arc<dyn MemoryBackend>>,
    session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
) -> CodeAgentSession {
    build_session_with_runtime_state(
        runtime,
        subagent_executor,
        store,
        startup,
        mcp_servers,
        configured_mcp_servers,
        Vec::new(),
        Arc::new(SwitchableCodeIntelBackend::lexical_only()),
        memory_backend,
        session_memory_model_backend,
    )
}

fn build_session_with_runtime_state(
    runtime: agent::AgentRuntime,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    startup: SessionStartupSnapshot,
    mcp_servers: Vec<ConnectedMcpServer>,
    configured_mcp_servers: Vec<McpServerConfig>,
    configured_runtime_hooks: Vec<HookRegistration>,
    code_intel_backend: Arc<SwitchableCodeIntelBackend>,
    memory_backend: Option<Arc<dyn MemoryBackend>>,
    session_memory_model_backend: Option<Arc<dyn ModelBackend>>,
) -> CodeAgentSession {
    let default_sandbox_policy = runtime.base_sandbox_policy();
    let process_executor = Arc::new(SwitchableHostProcessExecutor::new(
        Arc::new(agent::ManagedPolicyProcessExecutor::new()),
        startup.host_process_surfaces_allowed,
    ));
    let runtime_hooks = Arc::new(RwLock::new(if startup.host_process_surfaces_allowed {
        configured_runtime_hooks.clone()
    } else {
        configured_runtime_hooks
            .iter()
            .filter(|hook| !matches!(hook.handler, HookHandler::Command(_)))
            .cloned()
            .collect()
    }));
    let session_tool_context = Arc::new(RwLock::new(ToolExecutionContext {
        workspace_root: startup.workspace_root.clone(),
        worktree_root: Some(startup.workspace_root.clone()),
        effective_sandbox_policy: Some(default_sandbox_policy.clone()),
        workspace_only: true,
        ..Default::default()
    }));
    let events = SessionEventStream::default();
    let monitor_manager: Arc<dyn agent::tools::MonitorManager> =
        Arc::new(SessionMonitorManager::new(
            store.clone(),
            events.clone(),
            process_executor.clone() as Arc<dyn agent::tools::ProcessExecutor>,
        ));
    let worktree_manager = Arc::new(crate::backend::SessionWorktreeManager::new(
        store.clone(),
        events.clone(),
        process_executor.clone() as Arc<dyn agent::tools::ProcessExecutor>,
        startup.workspace_root.clone(),
        session_tool_context.clone(),
    ));
    let command_hook_executor = Arc::new(SwitchableCommandHookExecutor::new(
        process_executor.clone(),
        default_sandbox_policy.clone(),
        startup.host_process_surfaces_allowed,
    ));
    CodeAgentSession::new(
        runtime,
        None,
        session_memory_model_backend,
        subagent_executor,
        monitor_manager,
        worktree_manager,
        store,
        mcp_servers,
        configured_mcp_servers,
        runtime_hooks,
        configured_runtime_hooks,
        process_executor.clone() as Arc<dyn agent::tools::ProcessExecutor>,
        process_executor.clone(),
        command_hook_executor,
        code_intel_backend,
        ApprovalCoordinator::default(),
        UserInputCoordinator::default(),
        PermissionRequestCoordinator::default(),
        events,
        PermissionGrantStore::default(),
        session_tool_context,
        default_sandbox_policy,
        startup,
        SkillCatalog::default(),
        memory_backend,
        Arc::new(std::sync::Mutex::new(
            crate::backend::session_memory_compaction::SessionMemoryRefreshState::default(),
        )),
    )
}

fn local_stdio_mcp_config(server_name: &str) -> McpServerConfig {
    McpServerConfig {
        name: server_name.into(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "fixture-mcp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        },
    }
}

fn local_stdio_mcp_server(server_name: &str) -> ConnectedMcpServer {
    let boundary = McpToolBoundary::local_process(McpTransportKind::Stdio);
    let tool_spec = ToolSpec::function(
        "fixture_lookup",
        "Fixture MCP tool",
        json!({"type": "object", "properties": {}}),
        ToolOutputMode::Text,
        ToolOrigin::Mcp {
            server_name: server_name.into(),
        },
        ToolSource::McpTool {
            server_name: server_name.into(),
        },
    )
    .with_mcp_boundary(boundary.clone())
    .with_availability(ToolAvailability {
        feature_flags: vec![HOST_FEATURE_HOST_PROCESS_SURFACES.to_string()],
        ..ToolAvailability::default()
    });
    let catalog = McpCatalog {
        server_name: server_name.into(),
        tools: vec![tool_spec],
        prompts: Vec::new(),
        resources: vec![McpResource {
            uri: format!("{server_name}://guide"),
            name: "guide".to_string(),
            title: Some("Guide".to_string()),
            description: "fixture guide".to_string(),
            mime_type: Some("text/plain".to_string()),
            parts: vec![MessagePart::Text {
                text: "fixture resource".to_string(),
            }],
        }],
        resource_templates: vec![McpResourceTemplate {
            uri_template: format!("{server_name}://guide/{{section}}"),
            name: "guide-template".to_string(),
            title: Some("Guide Template".to_string()),
            description: "templated fixture guide".to_string(),
            mime_type: Some("text/plain".to_string()),
        }],
    };
    ConnectedMcpServer {
        server_name: server_name.into(),
        boundary,
        client: Arc::new(MockMcpClient::new(
            catalog.clone(),
            Arc::new(|tool_name, _arguments| {
                Ok(agent::types::ToolResult::text(
                    agent::types::ToolCallId::new(),
                    tool_name,
                    "fixture ok",
                ))
            }),
        )),
        catalog,
    }
}

fn command_hook(name: &str) -> HookRegistration {
    HookRegistration {
        name: name.into(),
        event: HookEvent::SessionStart,
        matcher: None,
        handler: HookHandler::Command(CommandHookHandler {
            command: "/bin/true".to_string(),
            asynchronous: false,
        }),
        timeout_ms: None,
        execution: None,
    }
}

async fn register_mcp_server_tools(registry: &mut ToolRegistry, server: ConnectedMcpServer) {
    for adapter in catalog_tools_as_registry_entries(server.client.clone())
        .await
        .unwrap()
    {
        registry.register(adapter);
    }
    for resource_tool in catalog_resource_tools_as_registry_entries(vec![server]) {
        registry.register(resource_tool);
    }
}

fn write_session_note_title(workspace_root: &std::path::Path, session_id: &SessionId, title: &str) {
    let path = workspace_root.join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let note = super::render_session_memory_note(&format!(
        "# Session Title\n\n{title}\n\n# Current State\n\nContinue from the saved plan."
    ));
    std::fs::write(path, note).unwrap();
}

fn sample_handle(task_id: &str, agent_id: &str, status: AgentStatus) -> AgentHandle {
    AgentHandle {
        agent_id: AgentId::from(agent_id),
        parent_agent_id: None,
        session_id: SessionId::from("session-1"),
        agent_session_id: agent::types::AgentSessionId::from(format!("agent-session-{task_id}")),
        task_id: TaskId::from(task_id),
        role: "worker".to_string(),
        status,
        worktree_id: None,
        worktree_root: None,
    }
}

#[tokio::test]
async fn start_new_session_refreshes_backend_snapshot_refs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let initial_session_ref = runtime.session_id().to_string();
    let initial_agent_session_ref = runtime.agent_session_id().to_string();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = initial_session_ref.clone();
    startup.root_agent_session_id = initial_agent_session_ref.clone();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup,
    );

    let outcome = session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();

    assert_eq!(outcome.action, SessionOperationAction::StartedFresh);
    assert_ne!(outcome.startup.active_session_ref, initial_session_ref);
    assert_ne!(
        outcome.startup.root_agent_session_id,
        initial_agent_session_ref
    );
    assert_eq!(outcome.startup.stored_session_count, 1);
    assert!(outcome.transcript.is_empty());

    let new_events = store
        .events(&SessionId::from(outcome.startup.active_session_ref.clone()))
        .await
        .unwrap();
    assert!(new_events.iter().any(|event| matches!(
        &event.event,
        SessionEventKind::SessionStart { reason }
            if reason.as_deref() == Some("operator_new_session")
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn build_session_during_async_startup_does_not_block_the_runtime_thread() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();

    let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

    let snapshot = session.startup_snapshot();
    assert!(!snapshot.active_session_ref.is_empty());
}

#[tokio::test]
async fn start_new_session_keeps_base_instructions_stable() {
    let dir = tempfile::tempdir().unwrap();
    let backend = RecordingPromptBackend::default();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("before refresh"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "# Rules\nrefresh on new session",
    )
    .unwrap();

    session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();
    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("after refresh"),
            submitted_prompt: None,
        })
        .await
        .unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        !requests[0]
            .instructions
            .join("\n\n")
            .contains("# Workspace Memory Primer")
    );
    let refreshed = requests[1].instructions.join("\n\n");
    assert!(!refreshed.contains("# Workspace Memory Primer"));
    assert!(!refreshed.contains("refresh on new session"));
    assert_eq!(refreshed, requests[0].instructions.join("\n\n"));
}

#[tokio::test]
async fn queued_prompts_are_drained_by_runtime_owned_queue() {
    let dir = tempfile::tempdir().unwrap();
    let backend = RecordingPromptBackend::default();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

    let queued_id = session
        .queue_prompt_command(Message::user("second"), None)
        .await
        .unwrap();
    assert!(!queued_id.is_empty());
    assert_eq!(session.queued_command_count(), 1);

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("first"),
            submitted_prompt: None,
        })
        .await
        .unwrap();

    assert_eq!(session.queued_command_count(), 0);
    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].messages.last().unwrap().text_content(), "first");
    assert_eq!(
        requests[1].messages.last().unwrap().text_content(),
        "second"
    );
}

#[tokio::test]
async fn permission_mode_switch_updates_frontend_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut registry = ToolRegistry::new();
    registry.register(ExecCommandTool::new());
    registry.register(WriteStdinTool::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
    );

    let outcome = session
        .set_permission_mode(SessionPermissionMode::DangerFullAccess)
        .await
        .unwrap();
    let snapshot = session.startup_snapshot();

    assert_eq!(outcome.current, SessionPermissionMode::DangerFullAccess);
    assert_eq!(
        snapshot.permission_mode,
        SessionPermissionMode::DangerFullAccess
    );
    assert!(snapshot.host_process_surfaces_allowed);
    assert!(snapshot.sandbox_summary.contains("danger-full-access"));
    assert_eq!(
        snapshot.tool_names,
        vec!["exec_command".to_string(), "write_stdin".to_string()]
    );
}

#[tokio::test]
async fn request_permissions_widens_same_turn_execution_without_mutating_tool_surface() {
    let dir = tempfile::tempdir().unwrap();
    let inspect_path = dir.path().join("granted").join("output.txt");
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(PermissionSurfaceBackend::default());
    let mut registry = ToolRegistry::new();
    registry.register(RequestPermissionsTool::new());
    registry
        .register_dynamic(
            DynamicToolSpec::function(
                "inspect_policy",
                "Checks whether a granted write root is active",
                json!({"type":"object","properties":{}}),
            ),
            Arc::new(move |call_id, _arguments, ctx| {
                let inspect_path = inspect_path.clone();
                Box::pin(async move {
                    ctx.assert_path_write_allowed(&inspect_path)?;
                    Ok(agent::types::ToolResult::text(
                        call_id,
                        "inspect_policy",
                        "write allowed",
                    ))
                })
            }),
        )
        .unwrap();
    let mut runtime = AgentRuntimeBuilder::new(backend.clone(), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            model_visibility: ToolVisibilityContext::default()
                .with_feature(HOST_FEATURE_REQUEST_PERMISSIONS),
            permission_request_handler: Some(Arc::new(GrantRequestedPermissionsHandler)),
            ..Default::default()
        })
        .build();

    let initial_tool_names = runtime
        .tool_specs()
        .into_iter()
        .map(|spec| spec.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        initial_tool_names,
        vec![
            "inspect_policy".to_string(),
            "request_permissions".to_string()
        ]
    );

    let outcome = runtime
        .run_user_prompt("request write access and then inspect it")
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "done");

    let requests = backend.requests();
    assert_eq!(requests.len(), 3);
    let first_request_tools = requests[0]
        .tools
        .iter()
        .map(|spec| spec.name.to_string())
        .collect::<Vec<_>>();
    let second_request_tools = requests[1]
        .tools
        .iter()
        .map(|spec| spec.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(first_request_tools, initial_tool_names);
    assert_eq!(second_request_tools, initial_tool_names);
    assert!(
        requests[1]
            .messages
            .iter()
            .flat_map(|message| message.parts.iter())
            .any(|part| matches!(
                part,
                MessagePart::ToolResult { result }
                    if result.tool_name.as_str() == "request_permissions"
            ))
    );
    assert_eq!(
        runtime
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name.to_string())
            .collect::<Vec<_>>(),
        initial_tool_names
    );
}

#[tokio::test]
async fn permission_mode_switch_fails_fast_while_turn_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(GatedTextBackend::new("still running"));
    let runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
    );

    let running = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .apply_control(RuntimeCommand::Prompt {
                    message: Message::user("hold the turn open"),
                    submitted_prompt: None,
                })
                .await
        })
    };

    timeout(Duration::from_secs(1), async {
        loop {
            if !backend.requests().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("turn should start");

    let error = timeout(
        Duration::from_millis(100),
        session.set_permission_mode(SessionPermissionMode::DangerFullAccess),
    )
    .await
    .expect("permission switch should fail fast")
    .expect_err("permission switch should be rejected while turn is running");
    assert!(
        error
            .to_string()
            .contains(PERMISSION_MODE_SWITCH_BLOCKED_WHILE_TURN_RUNNING)
    );

    backend.release();
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn permission_mode_switch_restores_command_hooks_into_runtime_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.host_process_surfaces_allowed = false;
    startup.startup_diagnostics.warnings =
        vec![format!("{COMMAND_HOOK_DISABLED_WARNING_PREFIX} guard")];
    let session = build_session_with_runtime_state(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        vec![command_hook("guard")],
        Arc::new(SwitchableCodeIntelBackend::lexical_only()),
        None,
        None,
    );

    assert!(session.runtime_hooks.read().unwrap().is_empty());

    session
        .set_permission_mode(SessionPermissionMode::DangerFullAccess)
        .await
        .unwrap();

    let hook_names = session
        .runtime_hooks
        .read()
        .unwrap()
        .iter()
        .map(|hook| hook.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(hook_names, vec!["guard".to_string()]);
    assert!(
        session
            .startup_snapshot()
            .startup_diagnostics
            .warnings
            .iter()
            .all(|warning| !warning.starts_with(COMMAND_HOOK_DISABLED_WARNING_PREFIX))
    );
}

#[tokio::test]
async fn refresh_startup_diagnostics_reports_deferred_command_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session_with_runtime_state(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
        Vec::new(),
        Vec::new(),
        vec![command_hook("guard")],
        Arc::new(SwitchableCodeIntelBackend::lexical_only()),
        None,
        None,
    );

    let runtime = session.runtime.lock().await;
    let diagnostics =
        session.refresh_startup_diagnostics_snapshot(&runtime, false, Some("bwrap missing"));

    assert!(diagnostics.warnings.iter().any(|warning| {
        warning.starts_with(COMMAND_HOOK_DISABLED_WARNING_PREFIX) && warning.contains("guard")
    }));
}

#[tokio::test]
async fn permission_mode_switch_enables_managed_code_intel_and_clears_warning() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let code_intel_backend = Arc::new(SwitchableCodeIntelBackend::managed_for_workspace(
        dir.path(),
        Arc::new(agent::ManagedPolicyProcessExecutor::new()),
        false,
    ));
    let mut startup = startup_snapshot(dir.path());
    startup.host_process_surfaces_allowed = false;
    startup.startup_diagnostics.warnings = vec![format!(
        "{MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX} bwrap missing"
    )];
    let session = build_session_with_runtime_state(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        code_intel_backend,
        None,
        None,
    );

    assert!(!session.code_intel_backend.managed_helpers_enabled());

    session
        .set_permission_mode(SessionPermissionMode::DangerFullAccess)
        .await
        .unwrap();

    assert!(session.code_intel_backend.managed_helpers_enabled());
    assert!(
        session
            .startup_snapshot()
            .startup_diagnostics
            .warnings
            .iter()
            .all(|warning| !warning.starts_with(MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX))
    );
}

#[tokio::test]
async fn attaching_stdio_mcp_servers_registers_tools_and_resources() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_visibility: ToolVisibilityContext::default()
                .with_feature(HOST_FEATURE_HOST_PROCESS_SURFACES),
            ..Default::default()
        })
        .build();
    let session = build_session_with_mcp(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
        Vec::new(),
        vec![local_stdio_mcp_config("fixture")],
    );
    let connected_server = local_stdio_mcp_server("fixture");
    let adapters = catalog_tools_as_registry_entries(connected_server.client.clone())
        .await
        .unwrap();

    let mut runtime = session.runtime.lock().await;
    session.attach_connected_stdio_mcp_servers(
        &mut runtime,
        vec![(connected_server.clone(), adapters)],
    );

    let tool_names = runtime
        .tool_specs()
        .into_iter()
        .map(|spec| spec.name.to_string())
        .collect::<Vec<_>>();
    assert!(tool_names.iter().any(|name| name == "fixture_lookup"));
    assert!(tool_names.iter().any(|name| name == "list_mcp_resources"));
    assert!(tool_names.iter().any(|name| name == "read_mcp_resource"));
    assert_eq!(session.connected_mcp_servers_snapshot().len(), 1);
    assert_eq!(
        session.connected_mcp_servers_snapshot()[0]
            .server_name
            .as_str(),
        "fixture"
    );
}

#[tokio::test]
async fn detaching_stdio_mcp_servers_removes_tools_and_restores_warning() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let connected_server = local_stdio_mcp_server("fixture");
    let mut registry = ToolRegistry::new();
    register_mcp_server_tools(&mut registry, connected_server.clone()).await;
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_visibility: ToolVisibilityContext::default()
                .with_feature(HOST_FEATURE_HOST_PROCESS_SURFACES),
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.host_process_surfaces_allowed = true;
    startup.tool_names = runtime.tool_registry_names();
    startup.startup_diagnostics.mcp_servers =
        list_mcp_servers(std::slice::from_ref(&connected_server));
    let session = build_session_with_mcp(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        vec![connected_server.clone()],
        vec![local_stdio_mcp_config("fixture")],
    );

    let mut runtime = session.runtime.lock().await;
    session.detach_local_stdio_mcp_servers(&mut runtime);
    let diagnostics =
        session.refresh_startup_diagnostics_snapshot(&runtime, false, Some("bwrap missing"));

    assert!(session.connected_mcp_servers_snapshot().is_empty());
    assert!(
        runtime
            .tool_registry_handle()
            .get("fixture_lookup")
            .is_none()
    );
    assert!(
        runtime
            .tool_registry_handle()
            .get("list_mcp_resources")
            .is_none()
    );
    assert!(
        runtime
            .tool_registry_handle()
            .get("read_mcp_resource")
            .is_none()
    );
    assert_eq!(diagnostics.mcp_servers, Vec::<McpServerSummary>::new());
    assert!(diagnostics.warnings.iter().any(|warning| {
        warning.starts_with(STDIO_MCP_DISABLED_WARNING_PREFIX) && warning.contains("fixture")
    }));
}

#[tokio::test]
async fn pending_controls_can_be_updated_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

    let prompt_id = session
        .queue_prompt_command(Message::user("draft"), None)
        .await
        .unwrap();
    let steer_id = session
        .schedule_runtime_steer("focus on tests", Some("manual".to_string()))
        .unwrap();

    let updated_prompt = session
        .update_pending_control(&prompt_id, "edited draft")
        .unwrap();
    assert_eq!(updated_prompt.kind, PendingControlKind::Prompt);
    assert_eq!(updated_prompt.preview, "edited draft");

    let removed_steer = session.remove_pending_control(&steer_id).unwrap();
    assert_eq!(removed_steer.kind, PendingControlKind::Steer);
    assert_eq!(removed_steer.preview, "focus on tests");
    assert_eq!(session.pending_controls().len(), 1);
}

#[tokio::test]
async fn take_pending_steers_drains_all_steers_in_fifo_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let session = build_session(runtime, Arc::new(NoopSubagentExecutor), store, startup);

    let prompt_id = session
        .queue_prompt_command(Message::user("follow-up prompt"), None)
        .await
        .unwrap();
    let steer_one = session
        .schedule_runtime_steer(
            "first steer",
            Some(PendingControlReason::ManualCommand.runtime_value()),
        )
        .unwrap();
    let steer_two = session
        .schedule_runtime_steer(
            "latest steer",
            Some(PendingControlReason::InlineEnter.runtime_value()),
        )
        .unwrap();

    let promoted = session.take_pending_steers().unwrap();

    assert_eq!(promoted.len(), 2);
    assert_eq!(promoted[0].id, steer_one);
    assert_eq!(promoted[0].kind, PendingControlKind::Steer);
    assert_eq!(promoted[0].preview, "first steer");
    assert_eq!(
        promoted[0].reason,
        Some(PendingControlReason::ManualCommand)
    );
    assert_eq!(promoted[1].id, steer_two);
    assert_eq!(promoted[1].kind, PendingControlKind::Steer);
    assert_eq!(promoted[1].preview, "latest steer");
    assert_eq!(promoted[1].reason, Some(PendingControlReason::InlineEnter));

    let remaining = session.pending_controls();
    assert_eq!(remaining.len(), 1);
    assert!(remaining.iter().any(|control| control.id == prompt_id));
    assert!(!remaining.iter().any(|control| control.id == steer_one));
    assert!(!remaining.iter().any(|control| control.id == steer_two));
}

#[tokio::test]
async fn search_sessions_includes_title_only_session_note_matches() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup_snapshot(dir.path()),
    );
    let archived_session_id = SessionId::from("session-archived");

    store
        .append(SessionEventEnvelope::new(
            archived_session_id.clone(),
            AgentSessionId::from("agent-archived"),
            None,
            None,
            SessionEventKind::UserPromptSubmit {
                prompt: SubmittedPromptSnapshot::from_text("status update"),
            },
        ))
        .await
        .unwrap();
    write_session_note_title(
        dir.path(),
        &archived_session_id,
        "Deploy rollback follow-up",
    );

    let matches = session.search_sessions("rollback").await.unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].summary.session_ref,
        archived_session_id.to_string()
    );
    assert_eq!(
        matches[0].summary.session_title.as_deref(),
        Some("Deploy rollback follow-up")
    );
    assert_eq!(matches[0].matched_event_count, 0);
    assert_eq!(
        matches[0].preview_matches,
        vec!["session title: Deploy rollback follow-up".to_string()]
    );
}

#[tokio::test]
async fn load_session_resolves_unique_session_note_title_reference() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup_snapshot(dir.path()),
    );
    let archived_session_id = SessionId::from("session-archived");

    store
        .append(SessionEventEnvelope::new(
            archived_session_id.clone(),
            AgentSessionId::from("agent-archived"),
            None,
            None,
            SessionEventKind::UserPromptSubmit {
                prompt: SubmittedPromptSnapshot::from_text("status update"),
            },
        ))
        .await
        .unwrap();
    write_session_note_title(
        dir.path(),
        &archived_session_id,
        "Deploy rollback follow-up",
    );

    let loaded = session.load_session("rollback").await.unwrap();

    assert_eq!(loaded.summary.session_id, archived_session_id);
    assert_eq!(loaded.events.len(), 1);
    assert!(matches!(
        &loaded.events[0].event,
        SessionEventKind::UserPromptSubmit { prompt } if prompt.text == "status update"
    ));
}

#[tokio::test]
async fn load_session_rejects_ambiguous_session_note_title_reference() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup_snapshot(dir.path()),
    );

    for (session_id, agent_session_id, prompt, title) in [
        (
            SessionId::from("session-archived-a"),
            AgentSessionId::from("agent-archived-a"),
            "status update",
            "Deploy rollback follow-up",
        ),
        (
            SessionId::from("session-archived-b"),
            AgentSessionId::from("agent-archived-b"),
            "rollback checklist",
            "Rollback verification",
        ),
    ] {
        store
            .append(SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id,
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text(prompt),
                },
            ))
            .await
            .unwrap();
        write_session_note_title(dir.path(), &session_id, title);
    }

    let error = session
        .load_session("rollback")
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("ambiguous session title rollback"));
    assert!(error.contains("session-"));
}

#[tokio::test]
async fn list_agent_sessions_carries_parent_session_note_title() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup_snapshot(dir.path()),
    );
    let archived_session_id = SessionId::from("session-archived");

    store
        .append_batch(vec![
            SessionEventEnvelope::new(
                archived_session_id.clone(),
                AgentSessionId::from("agent-root"),
                None,
                None,
                SessionEventKind::SessionStart {
                    reason: Some("resume".to_string()),
                },
            ),
            SessionEventEnvelope::new(
                archived_session_id.clone(),
                AgentSessionId::from("agent-root"),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("inspect"),
                },
            ),
        ])
        .await
        .unwrap();
    write_session_note_title(
        dir.path(),
        &archived_session_id,
        "Deploy rollback follow-up",
    );

    let agent_sessions = session.list_agent_sessions(None).await.unwrap();

    assert_eq!(agent_sessions.len(), 1);
    assert_eq!(
        agent_sessions[0].session_title.as_deref(),
        Some("Deploy rollback follow-up")
    );
}

#[tokio::test]
async fn resume_agent_session_reattaches_archived_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let original_session_ref = runtime.session_id().to_string();
    let original_agent_session_ref = runtime.agent_session_id().to_string();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = original_session_ref.clone();
    startup.root_agent_session_id = original_agent_session_ref.clone();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup,
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("resume me"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();

    let outcome = session
        .apply_session_operation(SessionOperation::ResumeAgentSession {
            agent_session_ref: original_agent_session_ref.clone(),
        })
        .await
        .unwrap();

    assert_eq!(outcome.action, SessionOperationAction::Reattached);
    assert_eq!(
        outcome.requested_agent_session_ref.as_deref(),
        Some(original_agent_session_ref.as_str())
    );
    assert_eq!(outcome.session_ref, original_session_ref);
    assert_ne!(outcome.active_agent_session_ref, original_agent_session_ref);
    assert_eq!(outcome.startup.active_session_ref, outcome.session_ref);
    assert_eq!(
        outcome.startup.root_agent_session_id,
        outcome.active_agent_session_ref
    );
    assert_eq!(outcome.transcript.len(), 1);
    assert_eq!(outcome.transcript[0].text_content(), "resume me");
}

#[tokio::test]
async fn resume_persisted_session_reattaches_root_history_by_session_ref() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let original_session_ref = runtime.session_id().to_string();
    let original_agent_session_ref = runtime.agent_session_id().to_string();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = original_session_ref.clone();
    startup.root_agent_session_id = original_agent_session_ref.clone();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup,
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("resume me"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();

    session
        .resume_persisted_session(&original_session_ref)
        .await
        .unwrap();

    let startup = session.startup_snapshot();
    assert_eq!(startup.active_session_ref, original_session_ref);
    assert_ne!(startup.root_agent_session_id, original_agent_session_ref);

    let loaded = session
        .load_session(&startup.active_session_ref)
        .await
        .unwrap();
    assert_eq!(loaded.transcript.len(), 1);
    assert_eq!(loaded.transcript[0].text_content(), "resume me");
}

#[tokio::test]
async fn fork_persisted_session_seeds_new_session_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let original_session_ref = runtime.session_id().to_string();
    let original_agent_session_ref = runtime.agent_session_id().to_string();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = original_session_ref.clone();
    startup.root_agent_session_id = original_agent_session_ref.clone();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup,
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("fork me"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();

    session
        .fork_persisted_session(&original_session_ref)
        .await
        .unwrap();

    let startup = session.startup_snapshot();
    assert_ne!(startup.active_session_ref, original_session_ref);
    assert_ne!(startup.root_agent_session_id, original_agent_session_ref);

    let forked = session
        .load_session(&startup.active_session_ref)
        .await
        .unwrap();
    assert_eq!(
        forked.summary.session_id.as_str(),
        startup.active_session_ref.as_str()
    );
    assert_eq!(forked.transcript.len(), 1);
    assert_eq!(forked.transcript[0].text_content(), "fork me");
}

#[tokio::test]
async fn resume_agent_session_resolves_session_note_title_to_root_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let original_session_ref = runtime.session_id().to_string();
    let original_agent_session_ref = runtime.agent_session_id().to_string();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = original_session_ref.clone();
    startup.root_agent_session_id = original_agent_session_ref.clone();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup,
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("resume me"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    write_session_note_title(
        dir.path(),
        &SessionId::from(original_session_ref.clone()),
        "Deploy rollback follow-up",
    );
    session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();

    let outcome = session
        .apply_session_operation(SessionOperation::ResumeAgentSession {
            agent_session_ref: "rollback".to_string(),
        })
        .await
        .unwrap();

    assert_eq!(outcome.action, SessionOperationAction::Reattached);
    assert_eq!(
        outcome.requested_agent_session_ref.as_deref(),
        Some(original_agent_session_ref.as_str())
    );
    assert_eq!(outcome.session_ref, original_session_ref);
    assert_eq!(outcome.transcript.len(), 1);
    assert_eq!(outcome.transcript[0].text_content(), "resume me");
}

#[tokio::test]
async fn resume_agent_session_keeps_base_instructions_stable() {
    let dir = tempfile::tempdir().unwrap();
    let backend = RecordingPromptBackend::default();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let original_session_ref = runtime.session_id().to_string();
    let original_agent_session_ref = runtime.agent_session_id().to_string();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = original_session_ref;
    startup.root_agent_session_id = original_agent_session_ref.clone();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store.clone(),
        startup,
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("before resume"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    session
        .apply_session_operation(SessionOperation::StartFresh)
        .await
        .unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Rules\nrefresh on resume").unwrap();

    session
        .apply_session_operation(SessionOperation::ResumeAgentSession {
            agent_session_ref: original_agent_session_ref,
        })
        .await
        .unwrap();
    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("after resume"),
            submitted_prompt: None,
        })
        .await
        .unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        !requests[0]
            .instructions
            .join("\n\n")
            .contains("# Workspace Memory Primer")
    );
    let refreshed = requests[1].instructions.join("\n\n");
    assert!(!refreshed.contains("# Workspace Memory Primer"));
    assert!(!refreshed.contains("refresh on resume"));
    assert_eq!(refreshed, requests[0].instructions.join("\n\n"));
}

#[tokio::test]
async fn manual_compaction_persists_working_memory_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .conversation_compactor(Arc::new(StaticCompactor))
        .compaction_config(CompactionConfig {
            enabled: true,
            context_window_tokens: 64,
            trigger_tokens: 1,
            preserve_recent_messages: 0,
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
        dir.path().to_path_buf(),
        Default::default(),
    ));
    let session = build_session_with_memory(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Some(memory_backend),
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("first turn"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    session
        .apply_control(RuntimeCommand::Steer {
            message: "retain latest steer".to_string(),
            reason: Some("test".to_string()),
        })
        .await
        .unwrap();

    assert!(session.compact_now(None).await.unwrap());

    let working_path = dir.path().join(format!(
        ".nanoclaw/memory/working/sessions/{}.md",
        session.startup_snapshot().active_session_ref
    ));
    let snapshot = std::fs::read_to_string(working_path).unwrap();
    assert!(snapshot.contains("Session continuation snapshot"));
    assert!(snapshot.contains("# Session Title"));
    assert!(snapshot.contains("# Current State"));
    assert!(snapshot.contains("summary for 2 messages"));
    assert!(snapshot.contains("session_id:"));
    assert!(snapshot.contains("last_summarized_message_id:"));
}

#[tokio::test]
async fn compaction_working_snapshot_replaces_previous_body() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
        dir.path().to_path_buf(),
        Default::default(),
    ));
    let session = build_session_with_memory(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Some(memory_backend),
    );

    session
        .persist_compaction_working_snapshot(Some(CompactionWorkingSnapshot {
            session_id: SessionId::from("session-1"),
            agent_session_id: AgentSessionId::from("agent-session-1"),
            summary: "first snapshot".to_string(),
            summary_message_id: MessageId::from("summary-first"),
        }))
        .await;
    session
        .persist_compaction_working_snapshot(Some(CompactionWorkingSnapshot {
            session_id: SessionId::from("session-1"),
            agent_session_id: AgentSessionId::from("agent-session-1"),
            summary: "second snapshot".to_string(),
            summary_message_id: MessageId::from("summary-second"),
        }))
        .await;

    let snapshot = std::fs::read_to_string(
        dir.path()
            .join(".nanoclaw/memory/working/sessions/session-1.md"),
    )
    .unwrap();
    assert!(snapshot.contains("# Session Title"));
    assert!(snapshot.contains("# Current State"));
    assert!(snapshot.contains("second snapshot"));
    assert!(snapshot.contains("last_summarized_message_id: summary-second"));
    assert!(!snapshot.contains("first snapshot"));
}

#[tokio::test]
async fn forced_session_note_refresh_uses_summary_message_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session_id = runtime.session_id();
    let agent_session_id = runtime.agent_session_id();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = session_id.to_string();
    startup.root_agent_session_id = agent_session_id.to_string();
    let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
        dir.path().to_path_buf(),
        Default::default(),
    ));
    let note_backend = ScriptedTextBackend::new(vec![
        concat!(
            "# Current State\n",
            "Tracked tail update after compaction.\n\n",
            "# Worklog\n",
            "- Refreshed from transcript delta only."
        )
        .to_string(),
    ]);
    let session = build_session_with_backends(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        Some(memory_backend),
        Some(Arc::new(note_backend.clone())),
    );

    let summary_message = Message::system("summary before new work");
    let tail_message = Message::assistant("tail update after compaction");
    let context = SessionMemoryRefreshContext {
        session_id: session_id.clone(),
        agent_session_id: agent_session_id.clone(),
        visible_transcript: vec![summary_message.clone(), tail_message.clone()],
        context_tokens: 0,
        completed_turn_count: 1,
        tool_call_count: 0,
        compaction_summary_message_id: Some(summary_message.message_id.clone()),
    };

    session.mark_session_memory_refreshed(&context, Some(summary_message.message_id.clone()));
    session.maybe_refresh_session_memory_note(context, true);
    timeout(Duration::from_secs(1), async {
        loop {
            if note_backend.requests().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let requests = note_backend.requests();
    assert_eq!(requests.len(), 1);
    let update_prompt = requests[0].messages[0].text_content();
    assert!(update_prompt.contains("tail update after compaction"));
    assert!(!update_prompt.contains("summary before new work"));

    timeout(Duration::from_secs(1), async {
        loop {
            let state = session.session_memory_refresh.lock().unwrap().clone();
            if !state.refresh_in_flight
                && dir
                    .path()
                    .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
                    .exists()
            {
                break;
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let note = std::fs::read_to_string(
        dir.path()
            .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md")),
    )
    .unwrap();
    assert!(note.contains("# Session Title"));
    assert!(note.contains("Tracked tail update after compaction."));
    assert!(!note.contains("summary before new work"));

    let state = session.session_memory_refresh.lock().unwrap().clone();
    assert!(state.initialized);
    assert_eq!(
        state.last_summarized_message_id,
        Some(tail_message.message_id.clone())
    );
}

#[tokio::test]
async fn session_note_refresh_runs_in_background_without_blocking_caller() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session_id = runtime.session_id();
    let agent_session_id = runtime.agent_session_id();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = session_id.to_string();
    startup.root_agent_session_id = agent_session_id.to_string();
    let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
        dir.path().to_path_buf(),
        Default::default(),
    ));
    let note_backend =
        GatedTextBackend::new("# Current State\n\nAsync refresh completed successfully.");
    let session = build_session_with_backends(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        Some(memory_backend),
        Some(Arc::new(note_backend.clone())),
    );

    let context = SessionMemoryRefreshContext {
        session_id: session_id.clone(),
        agent_session_id: agent_session_id.clone(),
        visible_transcript: vec![Message::assistant("fresh transcript delta")],
        context_tokens: 12_000,
        completed_turn_count: 1,
        tool_call_count: 0,
        compaction_summary_message_id: None,
    };

    session.maybe_refresh_session_memory_note(context, true);
    timeout(Duration::from_secs(1), async {
        loop {
            if note_backend.requests().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let state = session.session_memory_refresh.lock().unwrap().clone();
    assert!(state.refresh_in_flight);
    assert!(
        !dir.path()
            .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
            .exists()
    );

    note_backend.release();
    timeout(Duration::from_secs(1), async {
        loop {
            let state = session.session_memory_refresh.lock().unwrap().clone();
            if !state.refresh_in_flight {
                break;
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let note = std::fs::read_to_string(
        dir.path()
            .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md")),
    )
    .unwrap();
    assert!(note.contains("Async refresh completed successfully."));
    let state = session.session_memory_refresh.lock().unwrap().clone();
    assert!(!state.refresh_in_flight);
    assert_eq!(state.active_session_id, Some(session_id));
}

#[tokio::test]
async fn session_switch_invalidates_in_flight_refresh_state_updates() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session_id = runtime.session_id();
    let agent_session_id = runtime.agent_session_id();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = session_id.to_string();
    startup.root_agent_session_id = agent_session_id.to_string();
    let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
        dir.path().to_path_buf(),
        Default::default(),
    ));
    let note_backend = GatedTextBackend::new("# Current State\n\nOld session background note.");
    let session = build_session_with_backends(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        Some(memory_backend),
        Some(Arc::new(note_backend.clone())),
    );

    let old_context = SessionMemoryRefreshContext {
        session_id: session_id.clone(),
        agent_session_id: agent_session_id.clone(),
        visible_transcript: vec![Message::assistant("old session delta")],
        context_tokens: 12_000,
        completed_turn_count: 1,
        tool_call_count: 0,
        compaction_summary_message_id: None,
    };
    session.maybe_refresh_session_memory_note(old_context, true);
    timeout(Duration::from_secs(1), async {
        loop {
            if note_backend.requests().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    session
        .reset_session_memory_refresh_state(&SideQuestionContextSnapshot {
            session_id: SessionId::from("session-new"),
            agent_session_id: AgentSessionId::from("agent-session-new"),
            instructions: Vec::new(),
            transcript: Vec::new(),
            tools: Vec::new(),
        })
        .await;

    note_backend.release();
    timeout(Duration::from_secs(1), async {
        loop {
            let state = session.session_memory_refresh.lock().unwrap().clone();
            if !state.refresh_in_flight
                && dir
                    .path()
                    .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
                    .exists()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let state = session.session_memory_refresh.lock().unwrap().clone();
    assert_eq!(
        state.active_session_id,
        Some(SessionId::from("session-new"))
    );
    assert_eq!(state.last_summarized_message_id, None);
    assert!(!state.initialized);
    assert!(
        dir.path()
            .join(format!(".nanoclaw/memory/working/sessions/{session_id}.md"))
            .exists()
    );
}

#[tokio::test]
async fn episodic_capture_appends_daily_log_entries_in_background() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session_id = runtime.session_id();
    let agent_session_id = runtime.agent_session_id();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = session_id.to_string();
    startup.root_agent_session_id = agent_session_id.to_string();
    let memory_backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
        dir.path().to_path_buf(),
        Default::default(),
    ));
    let capture_backend = ScriptedTextBackend::new(vec![
        "- User prefers canary deploys\n- Incident coordination moved to pager".to_string(),
    ]);
    let session = build_session_with_backends(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        Some(memory_backend),
        Some(Arc::new(capture_backend.clone())),
    );
    let context = SessionMemoryRefreshContext {
        session_id: session_id.clone(),
        agent_session_id: agent_session_id.clone(),
        visible_transcript: vec![
            Message::user("remember that canary deploys are preferred"),
            Message::assistant("I'll keep that in mind and note the incident channel."),
        ],
        context_tokens: 1_000,
        completed_turn_count: 1,
        tool_call_count: 0,
        compaction_summary_message_id: None,
    };
    session.maybe_capture_session_episodic_memory(context);
    let logs_root = dir.path().join(".nanoclaw/memory/episodic/logs");
    timeout(Duration::from_secs(1), async {
        loop {
            let has_log_file = std::fs::read_dir(&logs_root)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .flat_map(|year_dir| {
                    std::fs::read_dir(year_dir)
                        .ok()
                        .into_iter()
                        .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
                        .map(|entry| entry.path())
                        .filter(|path| path.is_dir())
                        .collect::<Vec<_>>()
                })
                .any(|month_dir| {
                    std::fs::read_dir(month_dir)
                        .ok()
                        .into_iter()
                        .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
                        .any(|entry| entry.path().is_file())
                });
            if capture_backend.requests().len() == 1 && has_log_file {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let requests = capture_backend.requests();
    assert_eq!(requests.len(), 1);
    let prompt = requests[0].messages[0].text_content();
    assert!(prompt.contains("append-only episodic daily log"));
    assert!(prompt.contains("canary deploys are preferred"));

    timeout(Duration::from_secs(1), async {
        loop {
            let state = session.session_episodic_capture.lock().unwrap().clone();
            if !state.capture_in_flight {
                break;
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let year_dir = std::fs::read_dir(&logs_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let month_dir = std::fs::read_dir(&year_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let log_path = std::fs::read_dir(&month_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let recorded = std::fs::read_to_string(log_path).unwrap();
    assert!(recorded.contains("scope: episodic"));
    assert!(recorded.contains("layer: daily-log"));
    assert!(recorded.contains("User prefers canary deploys"));
    assert!(recorded.contains("Incident coordination moved to pager"));
    let state = session.session_episodic_capture.lock().unwrap().clone();
    assert!(!state.capture_in_flight);
    assert_eq!(state.active_session_id, Some(session_id));
}

#[tokio::test]
async fn reset_session_memory_refresh_state_rebases_episodic_capture_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
    );
    let resumed_tail = Message::assistant("resume tail");

    session
        .reset_session_memory_refresh_state(&SideQuestionContextSnapshot {
            session_id: SessionId::from("session-new"),
            agent_session_id: AgentSessionId::from("agent-session-new"),
            instructions: Vec::new(),
            transcript: vec![resumed_tail.clone()],
            tools: Vec::new(),
        })
        .await;

    let state = session.session_episodic_capture.lock().unwrap().clone();
    assert_eq!(
        state.active_session_id,
        Some(SessionId::from("session-new"))
    );
    assert_eq!(
        state.last_captured_message_id,
        Some(resumed_tail.message_id.clone())
    );
    assert!(!state.capture_in_flight);
}

#[tokio::test]
async fn answer_side_question_uses_snapshot_context_and_wrapper_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime_backend = RecordingPromptBackend::default();
    let runtime = AgentRuntimeBuilder::new(Arc::new(runtime_backend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .instructions(vec!["stable base instruction".to_string()])
        .build();
    let mut startup = startup_snapshot(dir.path());
    startup.active_session_ref = runtime.session_id().to_string();
    startup.root_agent_session_id = runtime.agent_session_id().to_string();
    let side_backend = ScriptedTextBackend::new(vec!["Short answer.".to_string()]);
    let session = build_session_with_backends(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup,
        Vec::new(),
        Vec::new(),
        None,
        Some(Arc::new(side_backend.clone())),
    );

    session
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("main thread question"),
            submitted_prompt: None,
        })
        .await
        .unwrap();

    let outcome = session
        .answer_side_question("  what changed?  ")
        .await
        .unwrap();

    assert_eq!(outcome.question, "what changed?");
    assert_eq!(outcome.response, "Short answer.");

    let requests = side_backend.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.instructions, vec!["stable base instruction"]);
    assert!(request.tools.is_empty());
    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].text_content(), "main thread question");
    let side_prompt = request.messages[1].text_content();
    assert!(side_prompt.contains("This is a side question from the user"));
    assert!(side_prompt.contains("Do not call tools."));
    assert!(side_prompt.ends_with("what changed?"));
    assert_eq!(request.metadata["code_agent"]["purpose"], "side_question");
}

#[tokio::test]
async fn live_task_listing_projects_sorted_child_handles() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let executor = Arc::new(RecordingSubagentExecutor::new(vec![
        AgentHandle {
            role: "reviewer".to_string(),
            ..sample_handle("task-b", "agent-b", AgentStatus::Running)
        },
        AgentHandle {
            role: "researcher".to_string(),
            ..sample_handle("task-a", "agent-a", AgentStatus::Queued)
        },
    ]));
    let session = build_session(runtime, executor, store, startup_snapshot(dir.path()));

    let live_tasks = session.list_live_tasks().await.unwrap();

    assert_eq!(live_tasks.len(), 2);
    assert_eq!(live_tasks[0].task_id, TaskId::from("task-a"));
    assert_eq!(live_tasks[1].task_id, TaskId::from("task-b"));
    assert_eq!(live_tasks[0].role, "researcher");
}

#[tokio::test]
async fn spawn_live_task_returns_handle_and_tracks_active_parent_context() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let startup = startup_snapshot(dir.path());
    let active_session_ref = startup.active_session_ref.clone();
    let active_agent_session_ref = startup.root_agent_session_id.clone();
    let executor = Arc::new(RecordingSubagentExecutor::new(Vec::new()));
    let session = build_session(runtime, executor.clone(), store, startup);

    let outcome = session
        .spawn_live_task("reviewer", "inspect the failing tests")
        .await
        .unwrap();

    assert_eq!(outcome.task.role, "reviewer");
    assert_eq!(outcome.task.origin, TaskOrigin::ChildAgentBacked);
    assert_eq!(outcome.task.status, TaskStatus::Queued);
    assert_eq!(outcome.prompt, "inspect the failing tests");
    assert!(outcome.task.task_id.as_str().starts_with("task_"));
    let spawned_tasks = executor.spawned_tasks.lock().unwrap();
    assert_eq!(spawned_tasks.len(), 1);
    assert_eq!(spawned_tasks[0].role, "reviewer");
    assert_eq!(spawned_tasks[0].prompt, "inspect the failing tests");
    let spawn_parents = executor.spawn_parents.lock().unwrap();
    assert_eq!(spawn_parents.len(), 1);
    assert_eq!(
        spawn_parents[0]
            .session_id
            .as_ref()
            .map(|value| value.as_str()),
        Some(active_session_ref.as_str())
    );
    assert_eq!(
        spawn_parents[0]
            .agent_session_id
            .as_ref()
            .map(|value| value.as_str()),
        Some(active_agent_session_ref.as_str())
    );
    assert!(spawn_parents[0].parent_agent_id.is_none());
}

#[tokio::test]
async fn cancel_live_task_updates_backend_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(RecordingSubagentExecutor::new(vec![AgentHandle {
            role: "editor".to_string(),
            ..sample_handle("task-cancel", "agent-cancel", AgentStatus::Running)
        }])),
        store,
        startup_snapshot(dir.path()),
    );

    let outcome = session
        .cancel_live_task("task-cancel", Some("operator_cancel".to_string()))
        .await
        .unwrap();

    assert_eq!(outcome.action, super::LiveTaskControlAction::Cancelled);
    assert_eq!(outcome.task_id, TaskId::from("task-cancel"));
    assert_eq!(outcome.status, TaskStatus::Cancelled);
}

#[tokio::test]
async fn send_live_task_routes_steer_message_to_child_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let executor = Arc::new(RecordingSubagentExecutor::new(vec![sample_handle(
        "task-send",
        "agent-send",
        AgentStatus::Running,
    )]));
    let session = build_session(
        runtime,
        executor.clone(),
        store,
        startup_snapshot(dir.path()),
    );

    let outcome = session
        .send_live_task("task-send", "focus on tests")
        .await
        .unwrap();

    assert_eq!(outcome.action, super::LiveTaskMessageAction::Sent);
    assert_eq!(outcome.task_id, TaskId::from("task-send"));
    let sent_messages = executor.sent_messages.lock().unwrap();
    assert_eq!(sent_messages.len(), 1);
    assert_eq!(sent_messages[0].0, AgentId::from("agent-send"));
    assert_eq!(sent_messages[0].1, SubagentInputDelivery::Queue);
    assert_eq!(sent_messages[0].2.text_content(), "focus on tests");
}

#[tokio::test]
async fn wait_live_task_returns_terminal_result_summary() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let completed_handle = sample_handle("task-wait", "agent-wait", AgentStatus::Completed);
    let wait_response = AgentWaitResponse {
        completed: vec![completed_handle.clone()],
        pending: Vec::new(),
        results: vec![AgentResultEnvelope {
            agent_id: AgentId::from("agent-wait"),
            task_id: TaskId::from("task-wait"),
            status: AgentStatus::Completed,
            summary: "finished child task".to_string(),
            text: "done".to_string(),
            artifacts: Vec::new(),
            claimed_files: vec!["src/lib.rs".to_string()],
            structured_payload: None,
        }],
    };
    let session = build_session(
        runtime,
        Arc::new(RecordingSubagentExecutor::with_wait_response(
            vec![
                sample_handle("task-wait", "agent-wait", AgentStatus::Running),
                sample_handle("task-followup", "agent-followup", AgentStatus::Running),
                sample_handle("task-done", "agent-done", AgentStatus::Completed),
            ],
            wait_response,
        )),
        store,
        startup_snapshot(dir.path()),
    );

    let outcome = session.wait_live_task("task-wait").await.unwrap();

    assert_eq!(outcome.task_id, TaskId::from("task-wait"));
    assert_eq!(outcome.status, TaskStatus::Completed);
    assert_eq!(outcome.summary, "finished child task");
    assert_eq!(outcome.claimed_files, vec!["src/lib.rs".to_string()]);
    assert_eq!(
        outcome.remaining_live_tasks,
        vec![LiveTaskSummary {
            agent_id: "agent-followup".to_string(),
            task_id: TaskId::from("task-followup"),
            role: "worker".to_string(),
            origin: TaskOrigin::ChildAgentBacked,
            status: TaskStatus::Running,
            session_ref: "session-1".to_string(),
            agent_session_ref: "agent-session-task-followup".to_string(),
            worktree_id: None,
            worktree_root: None,
        }]
    );
}

#[test]
fn schedule_live_task_attention_queues_prompt_when_idle() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
    );
    let outcome = LiveTaskWaitOutcome {
        requested_ref: "task-wait".to_string(),
        agent_id: "agent-wait".to_string(),
        task_id: TaskId::from("task-wait"),
        status: TaskStatus::Completed,
        summary: "finished child task".to_string(),
        claimed_files: vec!["src/lib.rs".to_string()],
        remaining_live_tasks: vec![LiveTaskSummary {
            agent_id: "agent-followup".to_string(),
            task_id: TaskId::from("task-followup"),
            role: "reviewer".to_string(),
            origin: TaskOrigin::ChildAgentBacked,
            status: TaskStatus::Running,
            session_ref: "session-1".to_string(),
            agent_session_ref: "agent-session-task-followup".to_string(),
            worktree_id: None,
            worktree_root: None,
        }],
    };

    let scheduled = session
        .schedule_live_task_attention(&outcome, false)
        .unwrap();

    assert_eq!(scheduled.action, LiveTaskAttentionAction::QueuedPrompt);
    assert!(!scheduled.control_id.is_empty());
    assert!(
        scheduled
            .preview
            .contains("Background task task-wait finished with status completed.")
    );
    assert!(
        scheduled
            .preview
            .contains("Task summary: finished child task")
    );
    assert!(scheduled.preview.contains("Claimed files: src/lib.rs."));
    assert!(
        scheduled
            .preview
            .contains("Still running background tasks: task-followup (reviewer, running).")
    );
    assert!(
        scheduled
            .preview
            .contains("Review the completed background task and integrate any useful findings.")
    );

    let pending = session.pending_controls();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, PendingControlKind::Prompt);
    assert_eq!(pending[0].preview, scheduled.preview);
}

#[test]
fn schedule_live_task_attention_schedules_steer_when_turn_running() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .build();
    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
    );
    let outcome = LiveTaskWaitOutcome {
        requested_ref: "task-wait".to_string(),
        agent_id: "agent-wait".to_string(),
        task_id: TaskId::from("task-wait"),
        status: TaskStatus::Failed,
        summary: "child task failed".to_string(),
        claimed_files: Vec::new(),
        remaining_live_tasks: Vec::new(),
    };

    let scheduled = session
        .schedule_live_task_attention(&outcome, true)
        .unwrap();

    assert_eq!(scheduled.action, LiveTaskAttentionAction::ScheduledSteer);
    assert!(!scheduled.control_id.is_empty());
    assert!(
        scheduled
            .preview
            .contains("Background task task-wait finished with status failed.")
    );
    assert!(
        scheduled
            .preview
            .contains("Inspect the failed background task and decide whether to retry it.")
    );

    let pending = session.pending_controls();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, PendingControlKind::Steer);
    assert_eq!(
        pending[0].reason,
        Some(PendingControlReason::Other(
            "live_task_wait_complete:task-wait".to_string()
        ))
    );
    assert_eq!(pending[0].preview, scheduled.preview);
}

#[test]
fn restore_checkpoint_both_recovers_code_and_rewinds_visible_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(ScriptedTextBackend::new(vec![
        "answer one".to_string(),
        "answer two".to_string(),
    ]));
    let runtime_handle = tokio::runtime::Runtime::new().unwrap();
    let mut runtime = AgentRuntimeBuilder::new(backend, store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();

    let second_turn = runtime_handle.block_on(async {
        runtime.run_user_prompt("first task").await.unwrap();
        runtime.run_user_prompt("second task").await.unwrap()
    });

    let session = build_session(
        runtime,
        Arc::new(NoopSubagentExecutor),
        store,
        startup_snapshot(dir.path()),
    );
    let file_path = dir.path().join("sample.txt");
    runtime_handle.block_on(async {
        tokio::fs::write(&file_path, "before\n").await.unwrap();

        let (session_id, agent_session_id) = {
            let runtime = session.runtime.lock().await;
            (runtime.session_id(), runtime.agent_session_id())
        };
        let checkpoint_ctx = session
            .session_tool_context
            .read()
            .unwrap()
            .clone()
            .with_runtime_scope(
                session_id,
                agent_session_id,
                second_turn.turn_id.clone(),
                "write",
                "call-checkpoint-restore-test",
            );
        let checkpoint = session
            .checkpoint_manager
            .record_mutation(
                &checkpoint_ctx,
                CheckpointMutationRequest {
                    summary: "Updated sample.txt".to_string(),
                    changed_files: vec![CheckpointFileMutation {
                        requested_path: "sample.txt".to_string(),
                        resolved_path: file_path.clone(),
                        before_text: Some("before\n".to_string()),
                        after_text: Some("after\n".to_string()),
                    }],
                },
            )
            .await
            .unwrap();
        tokio::fs::write(&file_path, "after\n").await.unwrap();

        let rounds = session.history_rollback_rounds().await;
        assert_eq!(rounds.len(), 2);
        let latest = rounds.last().unwrap();
        assert_eq!(
            latest
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            Some(checkpoint.checkpoint_id.clone())
        );

        let outcome = session
            .restore_checkpoint(
                checkpoint.checkpoint_id.as_str(),
                CheckpointRestoreMode::Both,
            )
            .await
            .unwrap();

        assert_eq!(outcome.restore.restored_file_count, 1);
        assert_eq!(outcome.removed_message_count, 2);
        assert_eq!(
            tokio::fs::read_to_string(&file_path).await.unwrap(),
            "before\n"
        );
        assert_eq!(outcome.transcript.len(), 2);
        assert_eq!(outcome.transcript[0].role, MessageRole::User);
        assert_eq!(outcome.transcript[0].text_content(), "first task");
        assert_eq!(outcome.transcript[1].role, MessageRole::Assistant);
        assert_eq!(outcome.transcript[1].text_content(), "answer one");
    });
}
