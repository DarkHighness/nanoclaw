use super::super::AgentRuntime;
use super::support::{ContinuingBackend, StaticCompactor};
use crate::{
    AgentRuntimeBuilder, CompactionConfig, DefaultCommandHookExecutor, DefaultWasmHookExecutor,
    FailClosedAgentHookEvaluator, HookRunner, PromptHookEvaluator, ReqwestHttpHookExecutor, Result,
};
use async_trait::async_trait;
use skills::SkillCatalog;
use std::sync::{Arc, Mutex};
use store::{InMemorySessionStore, SessionStore};
use tools::ToolExecutionContext;
use types::{
    HookContext, HookEffect, HookRegistration, HookResult, MessageId, MessagePart, MessagePatch,
    MessageRole, MessageSelector, ProviderContinuation, SessionEventKind,
};

#[derive(Clone, Default)]
struct MessageIdPatchPromptEvaluator {
    target_message_id: Arc<Mutex<Option<MessageId>>>,
}

#[async_trait]
impl PromptHookEvaluator for MessageIdPatchPromptEvaluator {
    async fn evaluate(
        &self,
        _registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult> {
        let prompt = context
            .payload
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if prompt != "second task" {
            return Ok(HookResult::default());
        }
        let target_message_id = self
            .target_message_id
            .lock()
            .unwrap()
            .clone()
            .expect("target message id should be primed before the second turn");
        Ok(HookResult {
            effects: vec![HookEffect::PatchMessage {
                selector: MessageSelector::MessageId {
                    message_id: target_message_id,
                },
                patch: MessagePatch {
                    append_parts: vec![MessagePart::text(" patched")],
                    ..Default::default()
                },
            }],
        })
    }
}

#[derive(Clone, Default)]
struct LastAssistantPatchPromptEvaluator;

#[async_trait]
impl PromptHookEvaluator for LastAssistantPatchPromptEvaluator {
    async fn evaluate(
        &self,
        _registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult> {
        let prompt = context
            .payload
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if prompt != "third task" {
            return Ok(HookResult::default());
        }
        Ok(HookResult {
            effects: vec![HookEffect::PatchMessage {
                selector: MessageSelector::LastOfRole {
                    role: MessageRole::Assistant,
                },
                patch: MessagePatch {
                    append_parts: vec![MessagePart::text(" patched")],
                    ..Default::default()
                },
            }],
        })
    }
}

#[tokio::test]
async fn runtime_uses_provider_continuation_for_follow_up_turns() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::default());
    let store = Arc::new(InMemorySessionStore::new());
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
    let store = Arc::new(InMemorySessionStore::new());
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
    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            SessionEventKind::Notification { source, message }
                if source == "provider_state"
                    && message.contains("provider continuation lost")
        )
    }));
}

#[tokio::test]
async fn local_compaction_resets_provider_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::default());
    let store = Arc::new(InMemorySessionStore::new());
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
    assert!(requests[1].messages.iter().any(|message: &types::Message| {
        message.text_content().contains("summary for 2 messages")
    }));
}

#[tokio::test]
async fn message_id_patch_resets_provider_continuation_and_replays_full_visible_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::default());
    let store = Arc::new(InMemorySessionStore::new());
    let prompt_evaluator = Arc::new(MessageIdPatchPromptEvaluator::default());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(Arc::new(HookRunner::with_services(
            Arc::new(DefaultCommandHookExecutor::default()),
            Arc::new(ReqwestHttpHookExecutor::default()),
            prompt_evaluator.clone(),
            Arc::new(FailClosedAgentHookEvaluator),
            Arc::new(DefaultWasmHookExecutor::default()),
        )))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .hooks(vec![HookRegistration {
            name: "message-id-patch".to_string(),
            event: types::HookEvent::UserPromptSubmit,
            matcher: None,
            handler: types::HookHandler::Prompt(types::PromptHookHandler {
                prompt: "ignored".to_string(),
            }),
            timeout_ms: None,
            execution: None,
        }])
        .skill_catalog(SkillCatalog::default())
        .build();

    runtime.run_user_prompt("first task").await.unwrap();
    let first_message_id = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap()
        .first()
        .expect("first prompt should be in the transcript")
        .message_id
        .clone();
    *prompt_evaluator.target_message_id.lock().unwrap() = Some(first_message_id.clone());

    runtime.run_user_prompt("second task").await.unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].continuation.is_none());
    assert!(requests[1].messages.iter().any(|message| {
        message.message_id == first_message_id
            && message.parts
                == vec![
                    types::MessagePart::text("first task"),
                    types::MessagePart::text(" patched"),
                ]
    }));

    let transcript = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    assert_eq!(
        transcript[0].parts,
        vec![
            types::MessagePart::text("first task"),
            types::MessagePart::text(" patched"),
        ]
    );
    assert!(
        store
            .events(&runtime.session_id())
            .await
            .unwrap()
            .iter()
            .any(|event| {
                matches!(
                    &event.event,
                    SessionEventKind::TranscriptMessagePatched { message_id, .. }
                        if message_id == &first_message_id
                )
            })
    );
}

#[tokio::test]
async fn last_of_role_patch_targets_last_visible_assistant_message() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(ContinuingBackend::default());
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(backend.clone(), store.clone())
        .hook_runner(Arc::new(HookRunner::with_services(
            Arc::new(DefaultCommandHookExecutor::default()),
            Arc::new(ReqwestHttpHookExecutor::default()),
            Arc::new(LastAssistantPatchPromptEvaluator),
            Arc::new(FailClosedAgentHookEvaluator),
            Arc::new(DefaultWasmHookExecutor::default()),
        )))
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .hooks(vec![HookRegistration {
            name: "last-of-role-patch".to_string(),
            event: types::HookEvent::UserPromptSubmit,
            matcher: None,
            handler: types::HookHandler::Prompt(types::PromptHookHandler {
                prompt: "ignored".to_string(),
            }),
            timeout_ms: None,
            execution: None,
        }])
        .skill_catalog(SkillCatalog::default())
        .build();

    runtime.run_user_prompt("first task").await.unwrap();
    runtime.run_user_prompt("second task").await.unwrap();

    let transcript_after_second_turn = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    let assistant_messages = transcript_after_second_turn
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages.len(), 2);
    let first_assistant_id = assistant_messages[0].message_id.clone();
    let second_assistant_id = assistant_messages[1].message_id.clone();

    runtime.run_user_prompt("third task").await.unwrap();

    let requests = backend.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].continuation.is_none());
    assert!(requests[2].messages.iter().any(|message| {
        message.message_id == second_assistant_id
            && message.parts
                == vec![
                    types::MessagePart::text("response 2"),
                    types::MessagePart::text(" patched"),
                ]
    }));
    assert!(
        requests[2]
            .messages
            .iter()
            .any(|message| message.message_id == first_assistant_id
                && message.text_content() == "response 1")
    );

    let transcript = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    let assistant_messages = transcript
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(assistant_messages.len(), 3);
    assert_eq!(assistant_messages[0].text_content(), "response 1");
    assert_eq!(assistant_messages[1].text_content(), "response 2\n patched");
    assert_eq!(assistant_messages[2].text_content(), "response 3");
    assert!(
        store
            .events(&runtime.session_id())
            .await
            .unwrap()
            .iter()
            .any(|event| {
                matches!(
                    &event.event,
                    SessionEventKind::TranscriptMessagePatched { message_id, .. }
                        if message_id == &second_assistant_id
                )
            })
    );
}
