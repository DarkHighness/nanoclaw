use crate::preview::{PreviewCollapse, collapse_preview_text};
use agent::ToolOrigin;
use agent::runtime::{
    Result as RuntimeResult, RuntimeError, ToolApprovalHandler, ToolApprovalOutcome,
    ToolApprovalRequest,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalPrompt {
    pub(crate) tool_name: String,
    pub(crate) origin: String,
    pub(crate) mode: Option<String>,
    pub(crate) working_directory: Option<String>,
    pub(crate) content_label: String,
    pub(crate) content_preview: Vec<String>,
    pub(crate) reasons: Vec<String>,
}

impl ApprovalPrompt {
    pub(crate) fn from_request(request: &ToolApprovalRequest) -> Self {
        let (content_label, content_preview) =
            approval_content_preview(request.call.tool_name.as_str(), &request.call.arguments);
        Self {
            tool_name: request.call.tool_name.to_string(),
            origin: tool_origin_label(&request.call.origin),
            mode: approval_mode(&request.call.arguments),
            working_directory: approval_working_directory(&request.call.arguments),
            content_label,
            content_preview,
            reasons: request.reasons.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalDecision {
    Approve,
    Deny { reason: Option<String> },
}

impl ApprovalDecision {
    fn into_runtime(self) -> ToolApprovalOutcome {
        match self {
            Self::Approve => ToolApprovalOutcome::Approve,
            Self::Deny { reason } => ToolApprovalOutcome::Deny { reason },
        }
    }
}

#[derive(Default)]
struct ApprovalCoordinatorState {
    prompt: Option<ApprovalPrompt>,
    responder: Option<oneshot::Sender<ToolApprovalOutcome>>,
}

/// This coordinator keeps the pending approval request in backend-owned state
/// so any frontend can render and resolve it without owning runtime internals.
#[derive(Clone, Default)]
pub(crate) struct ApprovalCoordinator {
    inner: Arc<RwLock<ApprovalCoordinatorState>>,
}

impl ApprovalCoordinator {
    pub(crate) fn snapshot(&self) -> Option<ApprovalPrompt> {
        self.inner.read().unwrap().prompt.clone()
    }

    pub(crate) fn resolve(&self, decision: ApprovalDecision) -> bool {
        let mut inner = self.inner.write().unwrap();
        let responder = inner.responder.take();
        inner.prompt = None;
        if let Some(responder) = responder {
            let _ = responder.send(decision.into_runtime());
            true
        } else {
            false
        }
    }

    fn present(&self, prompt: ApprovalPrompt, responder: oneshot::Sender<ToolApprovalOutcome>) {
        let mut inner = self.inner.write().unwrap();
        inner.prompt = Some(prompt);
        inner.responder = Some(responder);
    }
}

pub(crate) struct SessionToolApprovalHandler {
    coordinator: ApprovalCoordinator,
}

impl SessionToolApprovalHandler {
    pub(crate) fn new(coordinator: ApprovalCoordinator) -> Self {
        Self { coordinator }
    }
}

#[async_trait]
impl ToolApprovalHandler for SessionToolApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> RuntimeResult<ToolApprovalOutcome> {
        let prompt = ApprovalPrompt::from_request(&request);
        let (tx, rx) = oneshot::channel();
        self.coordinator.present(prompt, tx);
        match rx.await {
            Ok(outcome) => Ok(outcome),
            Err(error) => Err(RuntimeError::hook(format!(
                "approval dialog closed unexpectedly: {error}"
            ))),
        }
        .or_else(|_| {
            Ok(ToolApprovalOutcome::Deny {
                reason: Some("approval dialog closed".to_string()),
            })
        })
    }
}

pub(crate) struct NonInteractiveToolApprovalHandler {
    reason: String,
}

impl NonInteractiveToolApprovalHandler {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl ToolApprovalHandler for NonInteractiveToolApprovalHandler {
    async fn decide(&self, _request: ToolApprovalRequest) -> RuntimeResult<ToolApprovalOutcome> {
        // Headless hosts do not have an approval UI to resume from, so approval
        // requests must fail closed instead of waiting indefinitely.
        Ok(ToolApprovalOutcome::Deny {
            reason: Some(self.reason.clone()),
        })
    }
}

fn tool_origin_label(origin: &ToolOrigin) -> String {
    match origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    }
}

fn approval_content_preview(tool_name: &str, arguments: &Value) -> (String, Vec<String>) {
    if tool_name == "exec_command" {
        let command = arguments.get("cmd").and_then(Value::as_str);
        if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
            return (
                "command".to_string(),
                collapse_preview_text(&format!("$ {command}"), 6, 96, PreviewCollapse::Head),
            );
        }
    }

    if tool_name == "write_stdin" {
        let session_id = arguments
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("<unknown>");
        let close_stdin = arguments
            .get("close_stdin")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let chars = arguments
            .get("chars")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if close_stdin && chars.is_empty() {
            return (
                "stdin".to_string(),
                vec![format!("close stdin {session_id}")],
            );
        }
        if chars.is_empty() {
            return (
                "stdin".to_string(),
                vec![format!("poll session {session_id}")],
            );
        }
        let mut lines = vec![format!("session {session_id}")];
        lines.extend(collapse_preview_text(
            &format!("stdin {}", chars.escape_default()),
            4,
            96,
            PreviewCollapse::Head,
        ));
        return ("stdin".to_string(), lines);
    }

    if tool_name == "update_plan" {
        let item_count = arguments
            .get("plan")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let mut lines = vec![if item_count == 0 {
            "clear plan".to_string()
        } else {
            format!("set {item_count} plan step(s)")
        }];
        if let Some(explanation) = arguments
            .get("explanation")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.extend(collapse_preview_text(
                explanation,
                2,
                96,
                PreviewCollapse::Head,
            ));
        }
        return ("arguments".to_string(), lines);
    }

    for key in ["path", "uri", "query", "prompt", "message"] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str)
            && !value.trim().is_empty()
        {
            return (
                "arguments".to_string(),
                collapse_preview_text(value.trim(), 6, 96, PreviewCollapse::Head),
            );
        }
    }

    (
        "arguments".to_string(),
        collapse_preview_text(&arguments.to_string(), 8, 88, PreviewCollapse::Head),
    )
}

fn approval_mode(arguments: &Value) -> Option<String> {
    arguments
        .get("mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn approval_working_directory(arguments: &Value) -> Option<String> {
    for key in ["cwd", "workdir", "working_directory", "working_dir"] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str)
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, NonInteractiveToolApprovalHandler,
        tool_origin_label,
    };
    use agent::runtime::{ToolApprovalHandler, ToolApprovalOutcome, ToolApprovalRequest};
    use agent::types::{ToolCall, ToolCallId, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
    use serde_json::json;

    #[test]
    fn resolving_missing_request_is_a_noop() {
        assert!(!ApprovalCoordinator::default().resolve(ApprovalDecision::Approve));
    }

    #[tokio::test]
    async fn non_interactive_handler_denies_immediately() {
        let handler = NonInteractiveToolApprovalHandler::new("non-interactive mode");
        let outcome = handler
            .decide(ToolApprovalRequest {
                call: ToolCall {
                    id: ToolCallId::new(),
                    call_id: "call-1".into(),
                    tool_name: "write".into(),
                    arguments: json!({"path":"sample.txt"}),
                    origin: ToolOrigin::Local,
                },
                spec: ToolSpec::function(
                    "write",
                    "write",
                    json!({"type":"object"}),
                    ToolOutputMode::Text,
                    ToolOrigin::Local,
                    ToolSource::Builtin,
                ),
                reasons: vec!["destructive".to_string()],
            })
            .await
            .unwrap();

        assert_eq!(
            outcome,
            ToolApprovalOutcome::Deny {
                reason: Some("non-interactive mode".to_string()),
            }
        );
    }

    #[test]
    fn tool_origin_labels_provider_variants() {
        assert_eq!(tool_origin_label(&ToolOrigin::Local), "local");
        assert_eq!(
            tool_origin_label(&ToolOrigin::Mcp {
                server_name: "docs".into(),
            }),
            "mcp:docs"
        );
    }

    #[test]
    fn approval_prompt_extracts_exec_command_context() {
        let prompt = ApprovalPrompt::from_request(&ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: "call-1".into(),
                tool_name: "exec_command".into(),
                arguments: json!({
                    "cmd": "cargo test -p code-agent",
                    "workdir": "/workspace/apps/code-agent"
                }),
                origin: ToolOrigin::Local,
            },
            spec: ToolSpec::function(
                "exec_command",
                "run shell commands",
                json!({"type":"object"}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                ToolSource::Builtin,
            ),
            reasons: vec!["sandbox policy requires approval".to_string()],
        });

        assert_eq!(prompt.tool_name, "exec_command");
        assert_eq!(prompt.origin, "local");
        assert_eq!(prompt.mode, None);
        assert_eq!(
            prompt.working_directory.as_deref(),
            Some("/workspace/apps/code-agent")
        );
        assert_eq!(prompt.content_label, "command");
        assert_eq!(prompt.content_preview, vec!["$ cargo test -p code-agent"]);
    }
}
