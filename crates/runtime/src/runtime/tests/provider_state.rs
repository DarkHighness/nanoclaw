use super::super::AgentRuntime;
use super::support::{ContinuingBackend, StaticCompactor};
use crate::{AgentRuntimeBuilder, CompactionConfig, HookRunner};
use skills::SkillCatalog;
use std::sync::Arc;
use store::{InMemoryRunStore, RunStore};
use tools::ToolExecutionContext;
use types::{ProviderContinuation, RunEventKind};

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
