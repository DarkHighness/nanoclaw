use super::support::{FailingTool, RecordingBackend, RecordingObserver};
use crate::{
    AgentRuntimeBuilder, AugmentedUserMessage, HookRunner, ModelBackend, Result, RuntimeCommand,
    RuntimeControlPlane, RuntimeProgressEvent, UserMessageAugmentationContext,
    UserMessageAugmentor,
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use std::sync::{Arc, Mutex};
use store::{InMemorySessionStore, SessionStore, SessionStoreError};
use tools::{ReadTool, ToolExecutionContext, ToolRegistry};
use types::{
    Message, ModelEvent, ModelRequest, SessionEventKind, TokenUsage, TokenUsagePhase, ToolCall,
    ToolCallId, ToolLifecycleEventKind, ToolOrigin,
};

struct StreamingTextBackend;

struct PrefixMessageAugmentor;

#[derive(Clone, Default)]
struct ToolTurnRecordingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[derive(Clone)]
struct RetryThenSuccessBackend {
    attempts: Arc<Mutex<usize>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    retries_before_success: usize,
}

impl ToolTurnRecordingBackend {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl RetryThenSuccessBackend {
    fn with_failures(retries_before_success: usize) -> Self {
        Self {
            attempts: Arc::new(Mutex::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
            retries_before_success,
        }
    }

    fn attempts(&self) -> usize {
        *self.attempts.lock().unwrap()
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

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
                usage: Some(TokenUsage::from_input_output(120, 30, 20)),
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[async_trait]
impl UserMessageAugmentor for PrefixMessageAugmentor {
    async fn augment_user_message(
        &self,
        _context: &UserMessageAugmentationContext,
        message: Message,
    ) -> Result<AugmentedUserMessage> {
        Ok(AugmentedUserMessage {
            prefix_messages: vec![Message::user("recalled memory")],
            message,
        })
    }
}

#[async_trait]
impl ModelBackend for ToolTurnRecordingBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request.clone());
        let has_tool_result = request.messages.iter().any(|message| {
            message
                .parts
                .iter()
                .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
        });
        if !has_tool_result {
            let call = ToolCall {
                id: ToolCallId::new(),
                call_id: "call-read-1".into(),
                tool_name: "read".into(),
                arguments: serde_json::json!({"path":"sample.txt","line_count":1}),
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
impl ModelBackend for RetryThenSuccessBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        let mut attempts = self.attempts.lock().unwrap();
        *attempts = attempts.saturating_add(1);
        if *attempts <= self.retries_before_success {
            return Err(crate::RuntimeError::model_backend_request(
                "rate limit",
                429,
                true,
                None,
            ));
        }

        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: "ok".to_string(),
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

#[derive(Clone, Default)]
struct FailingToolTurnRecordingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl FailingToolTurnRecordingBackend {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelBackend for FailingToolTurnRecordingBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request.clone());
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
                delta: "handled".to_string(),
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

struct SteeringObserver {
    control_plane: RuntimeControlPlane,
    sent: bool,
    events: Vec<RuntimeProgressEvent>,
}

impl SteeringObserver {
    fn new(control_plane: RuntimeControlPlane) -> Self {
        Self {
            control_plane,
            sent: false,
            events: Vec::new(),
        }
    }
}

impl crate::RuntimeObserver for SteeringObserver {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> Result<()> {
        if !self.sent
            && matches!(
                &event,
                RuntimeProgressEvent::ToolLifecycle { event }
                    if matches!(event.event, ToolLifecycleEventKind::Completed { .. })
            )
        {
            self.control_plane
                .push_steer("prefer terse answers", Some("tool_safe_point".to_string()));
            self.sent = true;
        }
        self.events.push(event);
        Ok(())
    }
}

#[tokio::test]
async fn runtime_notifies_observer_of_streaming_text_progress() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();
    let mut observer = RecordingObserver::default();

    let outcome = runtime
        .run_user_prompt_with_observer("hello there", &mut observer)
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "hello");
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::UserPromptAdded { prompt } if prompt == "hello there"
    )));
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::AssistantTextDelta { delta } if delta == "hel"
    )));
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::AssistantTextDelta { delta } if delta == "lo"
    )));
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::TurnCompleted { assistant_text, .. } if assistant_text == "hello"
    )));
}

#[tokio::test]
async fn runtime_retries_retryable_provider_failures_before_streaming() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = RetryThenSuccessBackend::with_failures(2);
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();
    let mut observer = RecordingObserver::default();

    let outcome = runtime
        .run_user_prompt_with_observer("retry me", &mut observer)
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "ok");
    assert_eq!(backend.attempts(), 3);

    let retry_events = observer
        .events()
        .iter()
        .filter_map(|event| match event {
            RuntimeProgressEvent::ProviderRetryScheduled {
                status_code,
                retry_count,
                max_retries,
                remaining_retries,
                ..
            } => Some((*status_code, *retry_count, *max_retries, *remaining_retries)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retry_events, vec![(429, 1, 5, 4), (429, 2, 5, 3)]);
    let requests = backend.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].additional_context.iter().any(|entry| {
        entry.contains("tool_calls_so_far: 0")
            && entry.contains("provider_retries_so_far: 0")
            && entry.contains("error_recovery_signals_so_far: 0")
    }));
    assert!(requests[2].additional_context.iter().any(|entry| {
        entry.contains("tool_calls_so_far: 0")
            && entry.contains("provider_retries_so_far: 2")
            && entry.contains("error_recovery_signals_so_far: 2")
    }));

    let request_start_count = observer
        .events()
        .iter()
        .filter(|event| matches!(event, RuntimeProgressEvent::ModelRequestStarted { .. }))
        .count();
    assert_eq!(request_start_count, 3);
}

#[tokio::test]
async fn runtime_injects_skill_capture_counts_after_successful_tool_use() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
        .await
        .unwrap();
    let backend = ToolTurnRecordingBackend::default();
    let mut registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();

    let outcome = runtime
        .run_user_prompt("please inspect sample.txt")
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "done");
    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].additional_context.iter().any(|entry| {
        entry.contains("tool_calls_so_far: 0")
            && entry.contains("failed_tool_calls_so_far: 0")
            && entry.contains("provider_retries_so_far: 0")
    }));
    assert!(requests[1].additional_context.iter().any(|entry| {
        entry.contains("tool_calls_so_far: 1")
            && entry.contains("failed_tool_calls_so_far: 0")
            && entry.contains("provider_retries_so_far: 0")
            && entry.contains("error_recovery_signals_so_far: 0")
    }));
}

#[tokio::test]
async fn runtime_injects_skill_capture_counts_after_failed_tool_use() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FailingToolTurnRecordingBackend::default();
    let mut registry = ToolRegistry::new();
    registry.register(FailingTool);
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();

    let outcome = runtime
        .run_user_prompt("try the failing tool")
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "handled");
    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].additional_context.iter().any(|entry| {
        entry.contains("tool_calls_so_far: 1")
            && entry.contains("failed_tool_calls_so_far: 1")
            && entry.contains("provider_retries_so_far: 0")
            && entry.contains("error_recovery_signals_so_far: 1")
    }));
}

#[tokio::test]
async fn runtime_tracks_token_usage_and_context_window() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();
    let mut observer = RecordingObserver::default();

    runtime
        .run_user_prompt_with_observer("hello there", &mut observer)
        .await
        .unwrap();

    let token_events = observer
        .events()
        .iter()
        .filter_map(|event| match event {
            RuntimeProgressEvent::TokenUsageUpdated { phase, ledger } => Some((*phase, ledger)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(token_events.len(), 2);
    assert!(matches!(token_events[0].0, TokenUsagePhase::RequestStarted));
    assert!(matches!(
        token_events[1].0,
        TokenUsagePhase::ResponseCompleted
    ));
    assert!(
        token_events[0]
            .1
            .context_window
            .is_some_and(|usage| usage.used_tokens > 0 && usage.max_tokens == 200_000)
    );
    assert_eq!(
        token_events[1].1.last_usage,
        Some(TokenUsage::from_input_output(120, 30, 20))
    );
    assert_eq!(
        token_events[1].1.cumulative_usage,
        TokenUsage::from_input_output(120, 30, 20)
    );
    assert_eq!(
        token_events[1]
            .1
            .cumulative_usage
            .prefix_cache_hit_rate_basis_points(),
        Some(1667)
    );
    assert_eq!(
        runtime.token_ledger().cumulative_usage,
        TokenUsage::from_input_output(120, 30, 20)
    );
    assert_eq!(
        runtime
            .token_ledger()
            .cumulative_usage
            .prefix_cache_hit_rate_basis_points(),
        Some(1667)
    );

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        SessionEventKind::TokenUsageUpdated {
            phase: TokenUsagePhase::ResponseCompleted,
            ledger,
        } if ledger.cumulative_usage == TokenUsage::from_input_output(120, 30, 20)
    )));
}

#[tokio::test]
async fn runtime_inserts_augmentor_prefix_messages_before_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let backend = RecordingBackend::default();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        })
        .user_message_augmentor(Arc::new(PrefixMessageAugmentor))
        .build();
    let mut observer = RecordingObserver::default();

    runtime
        .run_user_prompt_with_observer("inspect the repo", &mut observer)
        .await
        .unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 1);
    let prompts = requests[0]
        .messages
        .iter()
        .map(Message::text_content)
        .collect::<Vec<_>>();
    assert_eq!(prompts, vec!["recalled memory", "inspect the repo"]);
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::UserPromptAdded { prompt } if prompt == "inspect the repo"
    )));
}

#[tokio::test]
async fn runtime_steer_appends_system_message_and_event() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
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

    let transcript = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    assert_eq!(transcript.len(), 1);
    assert_eq!(transcript[0].role, types::MessageRole::System);
    assert_eq!(transcript[0].text_content(), "stay focused on tests");

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            SessionEventKind::SteerApplied { message, reason }
                if message == "stay focused on tests"
                    && reason.as_deref() == Some("manual")
        )
    }));
    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::SteerApplied { message, reason }
            if message == "stay focused on tests" && reason.as_deref() == Some("manual")
    )));
}

#[tokio::test]
async fn runtime_apply_control_runs_prompt_and_steer_commands() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
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
            message: Message::user("hello"),
            submitted_prompt: None,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prompt.assistant_text, "hello");

    let transcript = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    assert_eq!(transcript[0].text_content(), "prefer terse answers");
    assert_eq!(transcript[1].text_content(), "hello");
    assert_eq!(transcript[2].text_content(), "hello");
}

#[tokio::test]
async fn runtime_apply_control_drains_runtime_prompt_queue_before_returning_idle() {
    let dir = tempfile::tempdir().unwrap();
    let backend = RecordingBackend::default();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();

    runtime.control_plane().push_prompt(Message::user("second"));

    let outcome = runtime
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("first"),
            submitted_prompt: None,
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.assistant_text, "ok");
    assert!(runtime.control_plane().is_empty());
    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].messages.last().unwrap().text_content(), "first");
    assert_eq!(
        requests[1].messages.last().unwrap().text_content(),
        "second"
    );
}

#[tokio::test]
async fn runtime_new_session_rotates_top_level_session_and_clears_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();

    runtime
        .apply_control(RuntimeCommand::Steer {
            message: "prefer terse answers".to_string(),
            reason: Some("queued".to_string()),
        })
        .await
        .unwrap();
    runtime
        .apply_control(RuntimeCommand::Prompt {
            message: Message::user("hello"),
            submitted_prompt: None,
        })
        .await
        .unwrap();
    let previous_session_id = runtime.session_id();
    let previous_agent_session_id = runtime.agent_session_id();

    runtime.start_new_session().await.unwrap();

    let next_session_id = runtime.session_id();
    let next_agent_session_id = runtime.agent_session_id();
    assert_ne!(next_session_id, previous_session_id);
    assert_ne!(next_agent_session_id, previous_agent_session_id);
    assert_eq!(
        runtime.token_ledger().cumulative_usage,
        TokenUsage::default()
    );

    let previous_events = store.events(&previous_session_id).await.unwrap();
    assert!(previous_events.iter().any(|event| {
        event.agent_session_id == previous_agent_session_id
            && matches!(
                &event.event,
                SessionEventKind::SessionEnd { reason }
                    if reason.as_deref() == Some("operator_new_session")
            )
    }));

    let next_events = store.events(&next_session_id).await;
    assert!(matches!(
        next_events,
        Err(SessionStoreError::SessionNotFound(session_id)) if session_id == next_session_id
    ));
    assert!(
        store
            .replay_transcript(&next_session_id)
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn ending_pristine_runtime_session_is_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();
    let session_id = runtime.session_id();

    runtime
        .end_session(Some("operator_exit".to_string()))
        .await
        .unwrap();

    let events = store.events(&session_id).await;
    assert!(matches!(
        events,
        Err(SessionStoreError::SessionNotFound(missing)) if missing == session_id
    ));
}

#[tokio::test]
async fn runtime_mailbox_steer_merges_at_safe_point_before_followup_request() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("sample.txt"), "hello\nworld")
        .await
        .unwrap();
    let backend = ToolTurnRecordingBackend::default();
    let store = Arc::new(InMemorySessionStore::new());
    let mut registry = ToolRegistry::new();
    registry.register(ReadTool::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(backend.clone()), store.clone())
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(registry)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .build();
    let mut observer = SteeringObserver::new(runtime.control_plane());

    let outcome = runtime
        .run_user_prompt_with_observer("please use tool", &mut observer)
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "done");
    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.role == types::MessageRole::System
                && message.text_content() == "prefer terse answers")
    );

    let transcript = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    assert!(transcript.iter().any(|message| {
        message.role == types::MessageRole::System
            && message.text_content() == "prefer terse answers"
    }));
    assert!(observer.events.iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::SteerApplied { message, reason }
            if message == "prefer terse answers"
                && reason.as_deref() == Some("tool_safe_point")
    )));

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        SessionEventKind::SteerApplied { message, reason }
            if message == "prefer terse answers"
                && reason.as_deref() == Some("tool_safe_point")
    )));
}
