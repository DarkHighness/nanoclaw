use super::AgentRuntime;
use crate::{
    AgentRuntimeBuilder, CompactionConfig, CompactionRequest, CompactionResult,
    ConversationCompactor, DefaultCommandHookExecutor, HookRunner, ModelBackend,
    ModelBackendCapabilities, NoopAgentHookEvaluator, ReqwestHttpHookExecutor, Result,
    RuntimeCommand, RuntimeObserver, RuntimeProgressEvent, StringMatcher, ToolApprovalHandler,
    ToolApprovalMatcher, ToolApprovalOutcome, ToolApprovalRequest, ToolApprovalRule,
    ToolApprovalRuleSet, ToolArgumentMatcher,
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use serde_json::Value;
use skills::{Skill, SkillCatalog};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use store::{InMemoryRunStore, RunStore};
use tools::{ReadTool, Tool, ToolError, ToolExecutionContext, ToolRegistry, mcp_tool_annotations};
use types::{
    AgentCoreError, HookContext, HookEvent, HookHandler, HookOutput, HookRegistration, Message,
    ModelEvent, ModelRequest, PromptHookHandler, ProviderContinuation, RunEventKind, ToolCall,
    ToolCallId, ToolLifecycleEventEnvelope, ToolLifecycleEventKind, ToolOrigin, ToolOutputMode,
    ToolResult, ToolSpec,
};

struct MockBackend;

#[derive(Clone, Default)]
struct RecordingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingBackend {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[derive(Clone, Default)]
struct ContinuingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    fail_first_continuation: Arc<Mutex<bool>>,
}

impl ContinuingBackend {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn with_failed_continuation() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            fail_first_continuation: Arc::new(Mutex::new(true)),
        }
    }
}

struct StaticPromptEvaluator;

struct StaticCompactor;

#[async_trait]
impl crate::PromptHookEvaluator for StaticPromptEvaluator {
    async fn evaluate(&self, _prompt: &str, _context: HookContext) -> Result<HookOutput> {
        Ok(HookOutput {
            system_message: Some("hook system message".to_string()),
            additional_context: vec!["hook additional context".to_string()],
            ..HookOutput::default()
        })
    }
}

#[async_trait]
impl ConversationCompactor for StaticCompactor {
    async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult> {
        Ok(CompactionResult {
            summary: format!("summary for {} messages", request.messages.len()),
        })
    }
}

#[derive(Clone, Debug, Default)]
struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fail".into(),
            description: "Always fails".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            output_mode: ToolOutputMode::Text,
            output_schema: None,
            origin: ToolOrigin::Local,
            annotations: Default::default(),
        }
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        _arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        Err(ToolError::invalid_state("boom"))
    }
}

#[derive(Clone, Debug, Default)]
struct DangerousTool;

#[async_trait]
impl Tool for DangerousTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "danger".into(),
            description: "Mutates files".to_string(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            output_mode: ToolOutputMode::Text,
            output_schema: None,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Dangerous Tool", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        _arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        Ok(ToolResult::text(call_id, "danger", "mutated"))
    }
}

#[derive(Default)]
struct MockApprovalHandler {
    requests: Mutex<Vec<ToolApprovalRequest>>,
    outcomes: Mutex<VecDeque<ToolApprovalOutcome>>,
}

#[derive(Default)]
struct RecordingObserver {
    events: Vec<RuntimeProgressEvent>,
}

impl RuntimeObserver for RecordingObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()> {
        self.events.push(event);
        Ok(())
    }
}

impl MockApprovalHandler {
    fn with_outcomes(outcomes: impl IntoIterator<Item = ToolApprovalOutcome>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes.into_iter().collect()),
        }
    }

    fn requests(&self) -> Vec<ToolApprovalRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolApprovalHandler for MockApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> Result<ToolApprovalOutcome> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ToolApprovalOutcome::Approve))
    }
}

#[async_trait]
impl ModelBackend for MockBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        let user_text = request
            .messages
            .last()
            .map(Message::text_content)
            .unwrap_or_default();
        if user_text.contains("tool")
            && !request.messages.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
            })
        {
            let call = ToolCall {
                id: ToolCallId::new(),
                call_id: "call-read-1".into(),
                tool_name: "read".into(),
                arguments: serde_json::json!({"path":"sample.txt","line_count":1}),
                origin: ToolOrigin::Local,
            };
            Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        } else {
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }
}

#[async_trait]
impl ModelBackend for RecordingBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: "ok".to_string(),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[async_trait]
impl ModelBackend for ContinuingBackend {
    fn capabilities(&self) -> ModelBackendCapabilities {
        ModelBackendCapabilities {
            provider_managed_history: true,
            provider_native_compaction: true,
        }
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request.clone());
        if request.continuation.is_some() {
            let mut fail_first = self.fail_first_continuation.lock().unwrap();
            if *fail_first {
                *fail_first = false;
                return Err(AgentCoreError::ProviderContinuationLost(
                    "provider lost previous_response_id".to_string(),
                )
                .into());
            }
        }

        let response_index = self.requests.lock().unwrap().len();
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: format!("response {response_index}"),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: Some(format!("msg_{response_index}").into()),
                continuation: Some(ProviderContinuation::OpenAiResponses {
                    response_id: format!("resp_{response_index}").into(),
                }),
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[tokio::test]
async fn runtime_handles_tool_loop() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
        .await
        .unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(Arc::new(MockBackend), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();

    let outcome = runtime.run_user_prompt("please use tool").await.unwrap();
    assert_eq!(outcome.assistant_text, "done");
}

#[tokio::test]
async fn observer_tool_lifecycle_events_share_store_event_ids() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
        .await
        .unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(Arc::new(MockBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();
    let mut observer = RecordingObserver::default();

    let outcome = runtime
        .run_user_prompt_with_observer("please use tool", &mut observer)
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "done");

    let observed_lifecycle = observer
        .events
        .iter()
        .filter_map(|event| match event {
            RuntimeProgressEvent::ToolLifecycle { event } => Some(event.clone()),
            _ => None,
        })
        .collect::<Vec<ToolLifecycleEventEnvelope>>();
    assert_eq!(observed_lifecycle.len(), 2);
    assert!(matches!(
        observed_lifecycle[0].event,
        ToolLifecycleEventKind::Started { .. }
    ));
    assert!(matches!(
        observed_lifecycle[1].event,
        ToolLifecycleEventKind::Completed { .. }
    ));

    let stored_lifecycle = store
        .events(&runtime.run_id())
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| event.tool_lifecycle_event())
        .collect::<Vec<_>>();
    assert_eq!(
        observed_lifecycle
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>(),
        stored_lifecycle
            .iter()
            .map(|event| event.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        observed_lifecycle
            .iter()
            .map(|event| event.tool_call_id.clone())
            .collect::<Vec<_>>(),
        stored_lifecycle
            .iter()
            .map(|event| event.tool_call_id.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn runtime_uses_provider_continuation_for_follow_up_turns() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::default());
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();

    runtime.run_user_prompt("first task").await.unwrap();
    runtime.run_user_prompt("second task").await.unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].continuation.is_none());
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(requests[0].messages[0].text_content(), "first task");
    assert_eq!(
        requests[1].continuation,
        Some(ProviderContinuation::OpenAiResponses {
            response_id: "resp_1".into(),
        })
    );
    assert_eq!(requests[1].messages.len(), 1);
    assert_eq!(requests[1].messages[0].text_content(), "second task");
}

#[tokio::test]
async fn runtime_retries_full_transcript_when_provider_continuation_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::with_failed_continuation());
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();

    runtime.run_user_prompt("first task").await.unwrap();
    runtime.run_user_prompt("second task").await.unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1].continuation,
        Some(ProviderContinuation::OpenAiResponses {
            response_id: "resp_1".into(),
        })
    );
    assert!(requests[2].continuation.is_none());
    assert!(
        requests[2].messages.len() >= 3,
        "fallback request should resend visible transcript"
    );
    let events = store.events(&runtime.run_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::Notification { source, message }
                if source == "provider_state"
                    && message.contains("provider continuation lost")
        )
    }));
}

#[tokio::test]
async fn local_compaction_resets_provider_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::default());
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .conversation_compactor(Arc::new(StaticCompactor))
        .compaction_config(CompactionConfig {
            enabled: true,
            context_window_tokens: 64,
            trigger_tokens: 32,
            preserve_recent_messages: 1,
        })
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();

    runtime.run_user_prompt("first task").await.unwrap();
    runtime
        .steer("keep explanations brief", Some("test".to_string()))
        .await
        .unwrap();
    assert!(runtime.compact_now(None).await.unwrap());
    runtime.run_user_prompt("second task").await.unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].continuation.is_none());
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.text_content().contains("summary for 2 messages"))
    );
}

struct ToolErrorRecoveringBackend;

#[async_trait]
impl ModelBackend for ToolErrorRecoveringBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        let has_tool_result = request.messages.iter().any(|message| {
            message
                .parts
                .iter()
                .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
        });
        if !has_tool_result {
            let call = ToolCall {
                id: ToolCallId::new(),
                call_id: "call-fail-1".into(),
                tool_name: "fail".into(),
                arguments: serde_json::json!({}),
                origin: ToolOrigin::Local,
            };
            Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        } else {
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "recovered".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }
}

#[tokio::test]
async fn runtime_continues_after_tool_error_result() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(FailingTool);
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime =
        AgentRuntimeBuilder::new(Arc::new(ToolErrorRecoveringBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_registry(registry)
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .skill_catalog(SkillCatalog::default())
            .build();

    let outcome = runtime
        .run_user_prompt("please use the failing tool")
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "recovered");

    let events = store.events(&runtime.run_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::ToolCallFailed { error, .. } if error.contains("boom")
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::TranscriptMessage { message }
                if message.parts.iter().any(|part| matches!(
                    part,
                    types::MessagePart::ToolResult { result }
                        if result.is_error && result.text_content().contains("boom")
            ))
        )
    }));
}

struct ApprovalRecoveringBackend;

#[async_trait]
impl ModelBackend for ApprovalRecoveringBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        let has_tool_result = request.messages.iter().any(|message| {
            message
                .parts
                .iter()
                .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
        });
        if !has_tool_result {
            let call = ToolCall {
                id: ToolCallId::new(),
                call_id: "call-danger-1".into(),
                tool_name: "danger".into(),
                arguments: serde_json::json!({"path":"sample.txt"}),
                origin: ToolOrigin::Local,
            };
            Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        } else {
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "approval recovered".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }
}

#[tokio::test]
async fn runtime_continues_after_tool_approval_denied() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(DangerousTool);
    let approval_handler = Arc::new(MockApprovalHandler::with_outcomes([
        ToolApprovalOutcome::Deny {
            reason: Some("user denied dangerous tool".to_string()),
        },
    ]));
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime =
        AgentRuntimeBuilder::new(Arc::new(ApprovalRecoveringBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_registry(registry)
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .tool_approval_handler(approval_handler.clone())
            .skill_catalog(SkillCatalog::default())
            .build();

    let outcome = runtime
        .run_user_prompt("please use the dangerous tool")
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "approval recovered");

    let requests = approval_handler.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].call.tool_name, types::ToolName::from("danger"));
    assert!(
        requests[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("destructive"))
    );

    let events = store.events(&runtime.run_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::ToolApprovalRequested { call, .. }
                if call.tool_name == types::ToolName::from("danger")
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::ToolApprovalResolved { call, approved, .. }
                if call.tool_name == types::ToolName::from("danger") && !approved
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::TranscriptMessage { message }
                if message.parts.iter().any(|part| matches!(
                    part,
                    types::MessagePart::ToolResult { result }
                        if result.is_error
                            && result.text_content() == "user denied dangerous tool"
                ))
        )
    }));
}

#[tokio::test]
async fn approval_policy_can_auto_allow_matching_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(DangerousTool);
    let approval_handler = Arc::new(MockApprovalHandler::with_outcomes([
        ToolApprovalOutcome::Deny {
            reason: Some("fallback should not run".to_string()),
        },
    ]));
    let policy = Arc::new(ToolApprovalRuleSet::new(vec![ToolApprovalRule::allow(
        ToolApprovalMatcher {
            tool_names: [types::ToolName::from("danger")].into_iter().collect(),
            origins: vec![crate::ToolOriginMatcher::Local],
            argument_matchers: vec![ToolArgumentMatcher::String {
                pointer: "/path".to_string(),
                matcher: StringMatcher::Prefix("sample".to_string()),
            }],
        },
        "allow the sample fixture destructive tool",
    )]));
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime =
        AgentRuntimeBuilder::new(Arc::new(ApprovalRecoveringBackend), store)
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_registry(registry)
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                model_context_window_tokens: Some(128_000),
                ..Default::default()
            })
            .tool_approval_handler(approval_handler.clone())
            .tool_approval_policy(policy)
            .skill_catalog(SkillCatalog::default())
            .build();

    let outcome = runtime
        .run_user_prompt("please use the dangerous tool")
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "approval recovered");
    assert!(approval_handler.requests().is_empty());
}

#[tokio::test]
async fn approval_policy_can_require_review_for_otherwise_safe_tools() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
        .await
        .unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    let approval_handler = Arc::new(MockApprovalHandler::with_outcomes([
        ToolApprovalOutcome::Deny {
            reason: Some("review required for sensitive file".to_string()),
        },
    ]));
    let policy = Arc::new(ToolApprovalRuleSet::new(vec![ToolApprovalRule::ask(
        ToolApprovalMatcher {
            tool_names: [types::ToolName::from("read")].into_iter().collect(),
            origins: vec![crate::ToolOriginMatcher::Local],
            argument_matchers: vec![ToolArgumentMatcher::String {
                pointer: "/path".to_string(),
                matcher: StringMatcher::Exact("sample.txt".to_string()),
            }],
        },
        "sensitive file read requires review",
    )]));
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(Arc::new(MockBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .tool_approval_handler(approval_handler.clone())
        .tool_approval_policy(policy)
        .skill_catalog(SkillCatalog::default())
        .build();

    let outcome = runtime.run_user_prompt("please use tool").await.unwrap();
    assert_eq!(outcome.assistant_text, "done");

    let requests = approval_handler.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("sensitive file read requires review"))
    );
    let events = store.events(&runtime.run_id()).await.unwrap();
    assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                RunEventKind::ToolApprovalRequested { reasons, .. }
                    if reasons.iter().any(|reason| reason.contains("sensitive file read requires review"))
            )
        }));
}

struct StreamingTextBackend;

#[async_trait]
impl ModelBackend for StreamingTextBackend {
    async fn stream_turn(
        &self,
        _request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: "hel".to_string(),
            }),
            Ok(ModelEvent::TextDelta {
                delta: "lo".to_string(),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[tokio::test]
async fn runtime_notifies_observer_of_streaming_text_progress() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();
    let mut observer = RecordingObserver::default();

    let outcome = runtime
        .run_user_prompt_with_observer("hello there", &mut observer)
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "hello");
    assert!(observer.events.iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::UserPromptAdded { prompt } if prompt == "hello there"
    )));
    assert!(observer.events.iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::AssistantTextDelta { delta } if delta == "hel"
    )));
    assert!(observer.events.iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::AssistantTextDelta { delta } if delta == "lo"
    )));
    assert!(observer.events.iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::TurnCompleted { assistant_text, .. } if assistant_text == "hello"
    )));
}

#[tokio::test]
async fn runtime_steer_appends_system_message_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();
    let mut observer = RecordingObserver::default();

    runtime
        .steer_with_observer(
            "stay focused on tests",
            Some("manual".to_string()),
            &mut observer,
        )
        .await
        .unwrap();

    let transcript = store.replay_transcript(&runtime.run_id()).await.unwrap();
    assert_eq!(transcript.len(), 1);
    assert_eq!(transcript[0].role, types::MessageRole::System);
    assert_eq!(transcript[0].text_content(), "stay focused on tests");

    let events = store.events(&runtime.run_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::SteerApplied { message, reason }
                if message == "stay focused on tests"
                    && reason.as_deref() == Some("manual")
        )
    }));
    assert!(observer.events.iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::SteerApplied { message, reason }
            if message == "stay focused on tests" && reason.as_deref() == Some("manual")
    )));
}

#[tokio::test]
async fn runtime_apply_control_runs_prompt_and_steer_commands() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryRunStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .skill_catalog(SkillCatalog::default())
        .build();

    let steer = runtime
        .apply_control(RuntimeCommand::Steer {
            message: "prefer terse answers".to_string(),
            reason: Some("queued".to_string()),
        })
        .await
        .unwrap();
    assert!(steer.is_none());

    let prompt = runtime
        .apply_control(RuntimeCommand::Prompt {
            prompt: "hello".to_string(),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prompt.assistant_text, "hello");

    let transcript = store.replay_transcript(&runtime.run_id()).await.unwrap();
    assert_eq!(transcript[0].text_content(), "prefer terse answers");
    assert_eq!(transcript[1].text_content(), "hello");
    assert_eq!(transcript[2].text_content(), "hello");
}

#[tokio::test]
async fn runtime_keeps_dynamic_hook_context_append_only_and_disables_prompt_skill_matching() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryRunStore::new());
    let backend = Arc::new(RecordingBackend::default());
    let skill_catalog = SkillCatalog::new(vec![Skill {
        name: "pdf".to_string(),
        description: "Use for PDF tasks".to_string(),
        aliases: vec!["acrobat".to_string()],
        body: "Use for PDF work.".to_string(),
        root_dir: PathBuf::from("/tmp/pdf"),
        tags: vec!["document".to_string()],
        hooks: Vec::new(),
        references: Vec::new(),
        scripts: Vec::new(),
        assets: Vec::new(),
        metadata: BTreeMap::new(),
        extension_metadata: BTreeMap::new(),
    }]);
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(DefaultCommandHookExecutor::default()),
        Arc::new(ReqwestHttpHookExecutor::default()),
        Arc::new(StaticPromptEvaluator),
        Arc::new(NoopAgentHookEvaluator),
    ));
    let mut runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(hook_runner)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .instructions(vec!["static base instruction".to_string()])
        .hooks(vec![HookRegistration {
            name: "inject_context".to_string(),
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            handler: HookHandler::Prompt(PromptHookHandler {
                prompt: "ignored".to_string(),
            }),
            timeout_ms: None,
        }])
        .skill_catalog(skill_catalog)
        .build();
    let mut observer = RecordingObserver::default();

    let _outcome = runtime
        .run_user_prompt_with_observer("please use acrobat skill on this file", &mut observer)
        .await
        .unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].instructions, vec!["static base instruction"]);
    assert!(requests[0].additional_context.is_empty());
    assert_eq!(requests[0].messages.len(), 3);
    assert_eq!(requests[0].messages[0].role, types::MessageRole::System);
    assert_eq!(
        requests[0].messages[0].text_content(),
        "hook system message"
    );
    assert_eq!(requests[0].messages[1].role, types::MessageRole::System);
    assert_eq!(
        requests[0].messages[1].text_content(),
        "hook additional context"
    );
    assert_eq!(requests[0].messages[2].role, types::MessageRole::User);
    assert_eq!(
        requests[0].messages[2].text_content(),
        "please use acrobat skill on this file"
    );

    let transcript = store.replay_transcript(&runtime.run_id()).await.unwrap();
    assert_eq!(transcript.len(), 4);
    assert_eq!(transcript[0].text_content(), "hook system message");
    assert_eq!(transcript[1].text_content(), "hook additional context");
    assert_eq!(
        transcript[2].text_content(),
        "please use acrobat skill on this file"
    );
    assert_eq!(transcript[3].text_content(), "ok");
}

#[tokio::test]
async fn runtime_auto_compacts_visible_history_before_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryRunStore::new());
    let backend = Arc::new(RecordingBackend::default());
    let mut runtime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .instructions(vec!["static base instruction".to_string()])
        .conversation_compactor(Arc::new(StaticCompactor))
        .compaction_config(CompactionConfig {
            enabled: true,
            context_window_tokens: 64,
            trigger_tokens: 1,
            preserve_recent_messages: 1,
        })
        .build();

    runtime.run_user_prompt("first turn").await.unwrap();
    runtime.run_user_prompt("second turn").await.unwrap();

    let requests = backend.requests();
    assert!(requests.len() >= 2);
    let last_request = requests.last().unwrap();
    assert_eq!(last_request.instructions, vec!["static base instruction"]);
    assert_eq!(last_request.messages[0].role, types::MessageRole::System);
    assert!(
        last_request.messages[0]
            .text_content()
            .starts_with("summary for ")
    );
    assert_eq!(last_request.messages.len(), 2);
    assert_eq!(last_request.messages[1].text_content(), "second turn");

    let events = store.events(&runtime.run_id()).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.event, RunEventKind::CompactionCompleted { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event.event,
            RunEventKind::CompactionCompleted {
                source_message_count: 2,
                retained_message_count: 1,
                ..
            }
        )
    }));
}
