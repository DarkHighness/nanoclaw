use super::support::{RecordingBackend, RecordingObserver, StaticCompactor};
use crate::{AgentRuntimeBuilder, CompactionConfig, HookRunner, RuntimeProgressEvent};
use std::sync::Arc;
use store::{InMemorySessionStore, SessionStore};
use tools::ToolExecutionContext;
use types::SessionEventKind;

#[tokio::test]
async fn runtime_auto_compacts_visible_history_before_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
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
    let initial_agent_session_id = runtime.agent_session_id();
    runtime.run_user_prompt("second turn").await.unwrap();

    let rotated_agent_session_id = runtime.agent_session_id();
    assert_ne!(rotated_agent_session_id, initial_agent_session_id);

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

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.event, SessionEventKind::CompactionCompleted { .. }))
    );
    assert!(events.iter().any(|event| {
        matches!(
            event.event,
            SessionEventKind::CompactionCompleted {
                source_message_count: 2,
                retained_message_count: 1,
                ..
            }
        )
    }));
    assert!(events.iter().any(|event| {
        event.agent_session_id == initial_agent_session_id
            && matches!(
                &event.event,
                SessionEventKind::SessionEnd { reason }
                    if reason.as_deref() == Some("compaction")
            )
    }));
    assert!(events.iter().any(|event| {
        event.agent_session_id == rotated_agent_session_id
            && matches!(
                &event.event,
                SessionEventKind::SessionStart { reason }
                    if reason.as_deref() == Some("compaction")
            )
    }));
}

#[tokio::test]
async fn manual_compaction_notifies_observer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(RecordingBackend::default());
    let mut runtime = AgentRuntimeBuilder::new(backend, store)
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
            preserve_recent_messages: 1,
        })
        .build();
    let mut observer = RecordingObserver::default();

    runtime.run_user_prompt("first turn").await.unwrap();
    runtime
        .steer("retain the latest steering note", Some("test".to_string()))
        .await
        .unwrap();
    runtime
        .compact_now_with_observer(None, &mut observer)
        .await
        .unwrap();

    assert!(observer.events().iter().any(|event| matches!(
        event,
        RuntimeProgressEvent::CompactionCompleted {
            source_message_count,
            retained_message_count,
            ..
        } if *source_message_count >= 2 && *retained_message_count >= 1
    )));
}

#[tokio::test]
async fn manual_compaction_rotates_root_agent_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(RecordingBackend::default());
    let mut runtime = AgentRuntimeBuilder::new(backend, store.clone())
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
            preserve_recent_messages: 1,
        })
        .build();

    runtime.run_user_prompt("first turn").await.unwrap();
    runtime
        .steer("retain the latest steering note", Some("test".to_string()))
        .await
        .unwrap();
    let initial_agent_session_id = runtime.agent_session_id();

    assert!(runtime.compact_now(None).await.unwrap());

    let rotated_agent_session_id = runtime.agent_session_id();
    assert_ne!(rotated_agent_session_id, initial_agent_session_id);

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        event.agent_session_id == initial_agent_session_id
            && matches!(
                &event.event,
                SessionEventKind::SessionEnd { reason }
                    if reason.as_deref() == Some("compaction")
            )
    }));
    assert!(events.iter().any(|event| {
        event.agent_session_id == rotated_agent_session_id
            && matches!(
                &event.event,
                SessionEventKind::SessionStart { reason }
                    if reason.as_deref() == Some("compaction")
            )
    }));
}
