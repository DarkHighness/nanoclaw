use super::super::AgentRuntime;
use super::support::{FailingTool, MockBackend, RecordingObserver};
use crate::{AgentRuntimeBuilder, HookRunner, ModelBackend, Result, RuntimeProgressEvent};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use std::sync::Arc;
use store::{InMemoryRunStore, RunStore};
use tools::{ReadTool, ToolExecutionContext, ToolRegistry};
use types::{
    ModelEvent, ModelRequest, RunEventKind, ToolCall, ToolCallId, ToolLifecycleEventEnvelope,
    ToolLifecycleEventKind, ToolOrigin,
};

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
        .build();
    let mut observer = RecordingObserver::default();

    let outcome = runtime
        .run_user_prompt_with_observer("please use tool", &mut observer)
        .await
        .unwrap();
    assert_eq!(outcome.assistant_text, "done");

    let observed_lifecycle = observer
        .events()
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
