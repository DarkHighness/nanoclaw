use super::support::{RecordingBackend, RecordingObserver, StaticPromptEvaluator};
use crate::{
    AgentRuntimeBuilder, DefaultCommandHookExecutor, DefaultWasmHookExecutor,
    FailClosedAgentHookEvaluator, HookRunner, ReqwestHttpHookExecutor,
};
use async_trait::async_trait;
use skills::{Skill, SkillCatalog};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use store::{InMemorySessionStore, SessionStore};
use tools::ToolExecutionContext;
use types::{
    HookContext, HookEffect, HookEffectPolicy, HookEvent, HookExecutionPolicy, HookHandler,
    HookRegistration, HookResult, PromptHookHandler, SessionEventKind,
};

struct UiPromptEvaluator;

#[async_trait]
impl crate::PromptHookEvaluator for UiPromptEvaluator {
    async fn evaluate(
        &self,
        _registration: &HookRegistration,
        _context: HookContext,
    ) -> crate::Result<HookResult> {
        Ok(HookResult {
            effects: vec![
                HookEffect::ShowToast {
                    variant: "warning".to_string(),
                    message: "review the hook result".to_string(),
                },
                HookEffect::AppendPrompt {
                    text: "queue a follow-up".to_string(),
                    only_when_empty: true,
                },
            ],
        })
    }
}

#[tokio::test]
async fn runtime_applies_hook_effects_without_mutating_base_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
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
        activation: Default::default(),
        provenance: skills::SkillProvenance {
            root: skills::SkillRoot::managed(PathBuf::from("/tmp/skills")),
            skill_dir: PathBuf::from("/tmp/pdf"),
        },
    }]);
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(DefaultCommandHookExecutor::default()),
        Arc::new(ReqwestHttpHookExecutor::default()),
        Arc::new(StaticPromptEvaluator),
        Arc::new(FailClosedAgentHookEvaluator),
        Arc::new(DefaultWasmHookExecutor::default()),
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
            name: "inject_context".into(),
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            handler: HookHandler::Prompt(PromptHookHandler {
                prompt: "ignored".to_string(),
            }),
            timeout_ms: None,
            execution: None,
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
    assert_eq!(
        requests[0].additional_context,
        vec!["hook additional context".to_string()]
    );
    assert_eq!(requests[0].messages.len(), 2);
    assert_eq!(requests[0].messages[0].role, types::MessageRole::System);
    assert_eq!(
        requests[0].messages[0].text_content(),
        "hook system message"
    );
    assert_eq!(requests[0].messages[1].role, types::MessageRole::User);
    assert_eq!(
        requests[0].messages[1].text_content(),
        "please use acrobat skill on this file"
    );

    let transcript = store
        .replay_transcript(&runtime.session_id())
        .await
        .unwrap();
    assert_eq!(transcript.len(), 3);
    assert_eq!(transcript[0].text_content(), "hook system message");
    assert_eq!(
        transcript[1].text_content(),
        "please use acrobat skill on this file"
    );
    assert_eq!(transcript[2].text_content(), "ok");

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        SessionEventKind::HookInvoked { hook_name, event }
            if hook_name == "inject_context" && event == &HookEvent::UserPromptSubmit
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event,
        SessionEventKind::HookCompleted {
            hook_name,
            event,
            output,
        } if hook_name == "inject_context"
            && event == &HookEvent::UserPromptSubmit
            && !output.effects.is_empty()
    )));
}

#[tokio::test]
async fn runtime_projects_hook_tui_events_into_live_observer_stream() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(RecordingBackend::default());
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(DefaultCommandHookExecutor::default()),
        Arc::new(ReqwestHttpHookExecutor::default()),
        Arc::new(UiPromptEvaluator),
        Arc::new(FailClosedAgentHookEvaluator),
        Arc::new(DefaultWasmHookExecutor::default()),
    ));
    let mut runtime = AgentRuntimeBuilder::new(backend, store)
        .hook_runner(hook_runner)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .hooks(vec![HookRegistration {
            name: "ui_hint".into(),
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            handler: HookHandler::Prompt(PromptHookHandler {
                prompt: "ignored".to_string(),
            }),
            timeout_ms: None,
            execution: Some(HookExecutionPolicy {
                effects: HookEffectPolicy {
                    allow_tui_event_emission: true,
                    ..HookEffectPolicy::default()
                },
                ..HookExecutionPolicy::default()
            }),
        }])
        .build();
    let mut observer = RecordingObserver::default();

    runtime
        .run_user_prompt_with_observer("hello", &mut observer)
        .await
        .unwrap();

    assert!(observer.events().iter().any(|event| matches!(
        event,
        crate::RuntimeProgressEvent::TuiToastShow { variant, message }
            if variant == "warning" && message == "review the hook result"
    )));
    assert!(observer.events().iter().any(|event| matches!(
        event,
        crate::RuntimeProgressEvent::TuiPromptAppend {
            text,
            only_when_empty,
        } if text == "queue a follow-up" && *only_when_empty
    )));
}

#[tokio::test]
async fn runtime_rejects_hook_tui_events_without_effect_grant() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySessionStore::new());
    let backend = Arc::new(RecordingBackend::default());
    let hook_runner = Arc::new(HookRunner::with_services(
        Arc::new(DefaultCommandHookExecutor::default()),
        Arc::new(ReqwestHttpHookExecutor::default()),
        Arc::new(UiPromptEvaluator),
        Arc::new(FailClosedAgentHookEvaluator),
        Arc::new(DefaultWasmHookExecutor::default()),
    ));
    let mut runtime = AgentRuntimeBuilder::new(backend, store)
        .hook_runner(hook_runner)
        .tool_context(ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            ..Default::default()
        })
        .hooks(vec![HookRegistration {
            name: "ui_hint".into(),
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            handler: HookHandler::Prompt(PromptHookHandler {
                prompt: "ignored".to_string(),
            }),
            timeout_ms: None,
            execution: Some(HookExecutionPolicy::default()),
        }])
        .build();
    let mut observer = RecordingObserver::default();

    let error = match runtime
        .run_user_prompt_with_observer("hello", &mut observer)
        .await
    {
        Ok(_) => panic!("ui events without effect grant should fail"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("not allowed to emit tui events"));
}
