use super::super::AgentRuntime;
use super::support::{DangerousTool, MockApprovalHandler, MockBackend};
use crate::{
    AgentRuntimeBuilder, HookRunner, ModelBackend, Result, StringMatcher, ToolApprovalMatcher,
    ToolApprovalOutcome, ToolApprovalPolicy, ToolApprovalRule, ToolApprovalRuleSet,
    ToolArgumentMatcher, ToolSourceMatcher,
};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use std::sync::Arc;
use store::{InMemorySessionStore, SessionStore};
use tools::{ReadTool, ToolExecutionContext, ToolRegistry};
use types::{
    ModelEvent, ModelRequest, SessionEventKind, ToolCall, ToolCallId, ToolOrigin, ToolOutputMode,
    ToolSource, ToolSpec,
};

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
                    usage: None,
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
                    usage: None,
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
    let store = Arc::new(InMemorySessionStore::new());
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
            .any(|reason: &String| reason.contains("mutates workspace"))
    );

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            SessionEventKind::ToolApprovalRequested { call, .. }
                if call.tool_name == types::ToolName::from("danger")
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            SessionEventKind::ToolApprovalResolved { call, approved, .. }
                if call.tool_name == types::ToolName::from("danger") && !approved
        )
    }));
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            SessionEventKind::TranscriptMessage { message }
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
            sources: vec![ToolSourceMatcher::Builtin],
            argument_matchers: vec![ToolArgumentMatcher::String {
                pointer: "/path".to_string(),
                matcher: StringMatcher::Prefix("sample".to_string()),
            }],
            mcp_boundary: None,
        },
        "allow the sample fixture destructive tool",
    )]));
    let store = Arc::new(InMemorySessionStore::new());
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
            .build();

    let outcome = runtime
        .run_user_prompt("please use the dangerous tool")
        .await
        .unwrap();

    assert_eq!(outcome.assistant_text, "approval recovered");
    assert!(approval_handler.requests().is_empty());
}

#[tokio::test]
async fn approval_policy_can_auto_allow_shared_exec_argv_rules() {
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
            sources: vec![ToolSourceMatcher::Builtin],
            argument_matchers: vec![ToolArgumentMatcher::SimpleShellArgvPrefix {
                pointer: "/path".to_string(),
                argv: vec!["sample.txt".to_string()],
            }],
            mcp_boundary: None,
        },
        "allow a simple argv-shaped command payload",
    )]));

    // DangerousTool does not actually use shell args, so this stays a pure matcher
    // regression proving the runtime can evaluate the new argv matcher path.
    let request = crate::ToolApprovalRequest {
        call: ToolCall {
            id: ToolCallId::new(),
            call_id: "call-shell-1".into(),
            tool_name: "danger".into(),
            arguments: serde_json::json!({"path": "sample.txt --check"}),
            origin: ToolOrigin::Local,
        },
        spec: ToolSpec::function(
            "danger",
            "danger",
            serde_json::json!({"type":"object"}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        ),
        reasons: Vec::new(),
    };

    assert_eq!(
        policy.decide(&request),
        crate::ToolApprovalPolicyDecision::Allow
    );

    let mut runtime: AgentRuntime = AgentRuntimeBuilder::new(
        Arc::new(ApprovalRecoveringBackend),
        Arc::new(InMemorySessionStore::new()),
    )
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
            sources: vec![ToolSourceMatcher::Builtin],
            argument_matchers: vec![ToolArgumentMatcher::String {
                pointer: "/path".to_string(),
                matcher: StringMatcher::Exact("sample.txt".to_string()),
            }],
            mcp_boundary: None,
        },
        "sensitive file read requires review",
    )]));
    let store = Arc::new(InMemorySessionStore::new());
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
        .build();

    let outcome = runtime.run_user_prompt("please use tool").await.unwrap();
    assert_eq!(outcome.assistant_text, "done");

    let requests = approval_handler.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .reasons
            .iter()
            .any(|reason: &String| reason.contains("sensitive file read requires review"))
    );
    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            SessionEventKind::ToolApprovalRequested { reasons, .. }
                if reasons
                    .iter()
                    .any(|reason: &String| reason.contains("sensitive file read requires review"))
        )
    }));
}

#[test]
fn mcp_server_matcher_is_exact_not_wildcard() {
    let request = crate::ToolApprovalRequest {
        call: ToolCall {
            id: ToolCallId::new(),
            call_id: "call-mcp-exact".into(),
            tool_name: "inspect_context".into(),
            arguments: serde_json::json!({}),
            origin: ToolOrigin::Mcp {
                server_name: "fixture".into(),
            },
        },
        spec: ToolSpec::function(
            "inspect_context",
            "inspect",
            serde_json::json!({"type":"object"}),
            ToolOutputMode::Text,
            ToolOrigin::Mcp {
                server_name: "fixture".into(),
            },
            ToolSource::McpTool {
                server_name: "fixture".into(),
            },
        ),
        reasons: Vec::new(),
    };
    let policy = ToolApprovalRuleSet::new(vec![ToolApprovalRule::allow(
        ToolApprovalMatcher {
            tool_names: [types::ToolName::from("inspect_context")]
                .into_iter()
                .collect(),
            origins: vec![crate::ToolOriginMatcher::McpServer {
                server_name: "*".into(),
            }],
            sources: vec![ToolSourceMatcher::McpTool],
            argument_matchers: Vec::new(),
            mcp_boundary: None,
        },
        "wildcard-like names are not supported",
    )]);

    assert_eq!(
        policy.decide(&request),
        crate::ToolApprovalPolicyDecision::Abstain
    );
}
