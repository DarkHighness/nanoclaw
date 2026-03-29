use super::support::{RecordingBackend, RecordingObserver, StaticPromptEvaluator};
use crate::{
    AgentRuntimeBuilder, DefaultCommandHookExecutor, DefaultWasmHookExecutor,
    FailClosedAgentHookEvaluator, HookRunner, ReqwestHttpHookExecutor,
};
use skills::{Skill, SkillCatalog};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use store::{InMemorySessionStore, SessionStore};
use tools::ToolExecutionContext;
use types::{HookEvent, HookHandler, HookRegistration, PromptHookHandler, SessionEventKind};

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
            name: "inject_context".to_string(),
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
