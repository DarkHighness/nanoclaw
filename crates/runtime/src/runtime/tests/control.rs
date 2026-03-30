use super::support::{RecordingBackend, RecordingObserver};
use crate::{
    AgentRuntimeBuilder, HookRunner, ModelBackend, Result, RuntimeCommand, RuntimeProgressEvent,
    RuntimeSteerMailbox,
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use std::sync::{Arc, Mutex};
use store::{InMemorySessionStore, SessionStore};
use tools::{ReadTool, ToolExecutionContext, ToolRegistry};
use types::{
    ModelEvent, ModelRequest, SessionEventKind, TokenUsage, TokenUsagePhase, ToolCall, ToolCallId,
    ToolLifecycleEventKind, ToolOrigin,
};

struct StreamingTextBackend;

#[derive(Clone, Default)]
struct ToolTurnRecordingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ToolTurnRecordingBackend {
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

struct SteeringObserver {
    mailbox: RuntimeSteerMailbox,
    sent: bool,
    events: Vec<RuntimeProgressEvent>,
}

impl SteeringObserver {
    fn new(mailbox: RuntimeSteerMailbox) -> Self {
        Self {
            mailbox,
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
            self.mailbox
                .send("prefer terse answers", Some("tool_safe_point".to_string()))
                .unwrap();
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
        runtime.token_ledger().cumulative_usage,
        TokenUsage::from_input_output(120, 30, 20)
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
            prompt: "hello".to_string(),
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

    runtime.command_queue().push_prompt("second").await;

    let outcome = runtime
        .apply_control(RuntimeCommand::Prompt {
            prompt: "first".to_string(),
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome.assistant_text, "ok");
    assert!(runtime.command_queue().is_empty());
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
            prompt: "hello".to_string(),
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

    let next_events = store.events(&next_session_id).await.unwrap();
    assert!(next_events.iter().any(|event| {
        event.agent_session_id == next_agent_session_id
            && matches!(
                &event.event,
                SessionEventKind::SessionStart { reason }
                    if reason.as_deref() == Some("operator_new_session")
            )
    }));
    assert!(
        store
            .replay_transcript(&next_session_id)
            .await
            .unwrap()
            .is_empty()
    );
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
    let mut observer = SteeringObserver::new(runtime.steer_mailbox());

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
