use super::support::RecordingObserver;
use crate::{
    AgentRuntimeBuilder, HookRunner, ModelBackend, Result, RuntimeCommand, RuntimeProgressEvent,
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use std::sync::Arc;
use store::{InMemoryRunStore, RunStore};
use tools::ToolExecutionContext;
use types::{ModelEvent, ModelRequest, RunEventKind};

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
    assert!(observer.events().iter().any(|event| matches!(
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
