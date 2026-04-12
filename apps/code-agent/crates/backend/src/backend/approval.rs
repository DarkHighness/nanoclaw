use crate::interaction::{
    ApprovalContent, ApprovalContentKind, ApprovalDecision, ApprovalOrigin, ApprovalPrompt,
};
use crate::preview::{PreviewCollapse, collapse_preview_text};
use crate::tool_render::ToolRenderKind;
use agent::ToolOrigin;
use agent::runtime::{
    Result as RuntimeResult, RuntimeError, ToolApprovalHandler, ToolApprovalOutcome,
    ToolApprovalRequest,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

fn approval_prompt_from_request(request: &ToolApprovalRequest) -> ApprovalPrompt {
    let content =
        approval_content_preview(request.call.tool_name.as_str(), &request.call.arguments);
    ApprovalPrompt {
        tool_name: request.call.tool_name.to_string(),
        origin: tool_origin_label(&request.call.origin),
        mode: approval_mode(&request.call.arguments),
        working_directory: approval_working_directory(&request.call.arguments),
        content,
        reasons: request.reasons.clone(),
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
pub struct ApprovalCoordinator {
    inner: Arc<RwLock<ApprovalCoordinatorState>>,
}

impl ApprovalCoordinator {
    pub fn snapshot(&self) -> Option<ApprovalPrompt> {
        self.inner.read().unwrap().prompt.clone()
    }

    pub fn resolve(&self, decision: ApprovalDecision) -> bool {
        let mut inner = self.inner.write().unwrap();
        let responder = inner.responder.take();
        inner.prompt = None;
        if let Some(responder) = responder {
            let outcome = match decision {
                ApprovalDecision::Approve => ToolApprovalOutcome::Approve,
                ApprovalDecision::Deny { reason } => ToolApprovalOutcome::Deny { reason },
            };
            let _ = responder.send(outcome);
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

pub struct SessionToolApprovalHandler {
    coordinator: ApprovalCoordinator,
}

impl SessionToolApprovalHandler {
    pub fn new(coordinator: ApprovalCoordinator) -> Self {
        Self { coordinator }
    }
}

#[async_trait]
impl ToolApprovalHandler for SessionToolApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> RuntimeResult<ToolApprovalOutcome> {
        let prompt = approval_prompt_from_request(&request);
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

pub struct NonInteractiveToolApprovalHandler {
    reason: String,
}

impl NonInteractiveToolApprovalHandler {
    pub fn new(reason: impl Into<String>) -> Self {
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

fn tool_origin_label(origin: &ToolOrigin) -> ApprovalOrigin {
    match origin {
        ToolOrigin::Local => ApprovalOrigin::Local,
        ToolOrigin::Mcp { server_name } => ApprovalOrigin::Mcp {
            server_name: server_name.to_string(),
        },
        ToolOrigin::Provider { provider } => ApprovalOrigin::Provider {
            provider: provider.clone(),
        },
    }
}

fn approval_content_preview(tool_name: &str, arguments: &Value) -> ApprovalContent {
    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::ExecCommand => {
            let command = arguments.get("cmd").and_then(Value::as_str);
            if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
                return ApprovalContent {
                    kind: ApprovalContentKind::Command,
                    preview: collapse_preview_text(
                        &format!("$ {command}"),
                        6,
                        96,
                        PreviewCollapse::Head,
                    ),
                };
            }
        }
        ToolRenderKind::WriteStdin => {
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
            let preview = if close_stdin && chars.is_empty() {
                vec![format!("close stdin {session_id}")]
            } else if chars.is_empty() {
                vec![format!("poll session {session_id}")]
            } else {
                let mut lines = vec![format!("session {session_id}")];
                lines.extend(collapse_preview_text(
                    &format!("stdin {}", chars.escape_default()),
                    4,
                    96,
                    PreviewCollapse::Head,
                ));
                lines
            };
            return ApprovalContent {
                kind: ApprovalContentKind::Stdin,
                preview,
            };
        }
        ToolRenderKind::NotebookRead => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            let mut preview = vec![format!("read notebook {path}")];
            let start_cell = arguments.get("start_cell").and_then(Value::as_u64);
            let end_cell = arguments.get("end_cell").and_then(Value::as_u64);
            if let (Some(start_cell), Some(end_cell)) = (start_cell, end_cell) {
                preview.push(format!("cells {start_cell}-{end_cell}"));
            }
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview,
            };
        }
        ToolRenderKind::NotebookEdit => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            let operation_count = arguments
                .get("operations")
                .and_then(Value::as_array)
                .map(|operations| operations.len())
                .unwrap_or(0);
            let mut preview = vec![format!("edit notebook {path}")];
            if operation_count > 0 {
                preview.push(format!("{operation_count} operation(s)"));
            }
            if arguments
                .get("expected_snapshot")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
            {
                preview.push("snapshot guarded".to_string());
            }
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview,
            };
        }
        ToolRenderKind::CodeSearch => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<empty>");
            let preview = arguments
                .get("path_prefix")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|path_prefix| vec![format!("search code for {query} in {path_prefix}")])
                .unwrap_or_else(|| vec![format!("search code for {query}")]);
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview,
            };
        }
        ToolRenderKind::CodeDiagnostics => {
            let preview = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|path| vec![format!("inspect diagnostics for {path}")])
                .unwrap_or_else(|| vec!["inspect workspace diagnostics".to_string()]);
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview,
            };
        }
        ToolRenderKind::MonitorStart => {
            let command = arguments.get("cmd").and_then(Value::as_str);
            if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
                let mut preview =
                    collapse_preview_text(&format!("$ {command}"), 4, 96, PreviewCollapse::Head);
                if let Some(workdir) = arguments
                    .get("workdir")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    preview.push(format!("cwd {workdir}"));
                }
                return ApprovalContent {
                    kind: ApprovalContentKind::Command,
                    preview,
                };
            }
        }
        ToolRenderKind::MonitorList => {
            let include_closed = arguments
                .get("include_closed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview: vec![if include_closed {
                    "list monitors including closed".to_string()
                } else {
                    "list active monitors".to_string()
                }],
            };
        }
        ToolRenderKind::MonitorStop => {
            let monitor_id = arguments
                .get("monitor_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            let mut preview = vec![format!("stop monitor {monitor_id}")];
            if let Some(reason) = arguments
                .get("reason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                preview.extend(collapse_preview_text(reason, 2, 96, PreviewCollapse::Head));
            }
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview,
            };
        }
        ToolRenderKind::WorktreeEnter => {
            let mut preview = vec!["enter session worktree".to_string()];
            if let Some(label) = arguments
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                preview.extend(collapse_preview_text(label, 2, 96, PreviewCollapse::Head));
            }
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview,
            };
        }
        ToolRenderKind::WorktreeList => {
            let include_inactive = arguments
                .get("include_inactive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview: vec![if include_inactive {
                    "list worktrees including inactive".to_string()
                } else {
                    "list active worktrees".to_string()
                }],
            };
        }
        ToolRenderKind::WorktreeExit => {
            let worktree_id = arguments
                .get("worktree_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("current");
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview: vec![format!("exit worktree {worktree_id}")],
            };
        }
        ToolRenderKind::SendInput
        | ToolRenderKind::SpawnAgent
        | ToolRenderKind::WaitAgent
        | ToolRenderKind::ResumeAgent
        | ToolRenderKind::CloseAgent
        | ToolRenderKind::FileMutation
        | ToolRenderKind::Generic => {}
    }

    for key in ["path", "uri", "query", "prompt", "message"] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str)
            && !value.trim().is_empty()
        {
            return ApprovalContent {
                kind: ApprovalContentKind::Arguments,
                preview: collapse_preview_text(value.trim(), 6, 96, PreviewCollapse::Head),
            };
        }
    }

    ApprovalContent {
        kind: ApprovalContentKind::Arguments,
        preview: collapse_preview_text(&arguments.to_string(), 8, 88, PreviewCollapse::Head),
    }
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
        ApprovalCoordinator, NonInteractiveToolApprovalHandler, approval_prompt_from_request,
        tool_origin_label,
    };
    use crate::{ApprovalContent, ApprovalContentKind, ApprovalDecision, ApprovalOrigin};
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
        assert_eq!(tool_origin_label(&ToolOrigin::Local), ApprovalOrigin::Local);
        assert_eq!(
            tool_origin_label(&ToolOrigin::Mcp {
                server_name: "docs".into(),
            }),
            ApprovalOrigin::Mcp {
                server_name: "docs".to_string(),
            }
        );
    }

    #[test]
    fn approval_prompt_extracts_exec_command_context() {
        let prompt = approval_prompt_from_request(&ToolApprovalRequest {
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
        assert_eq!(prompt.origin, ApprovalOrigin::Local);
        assert_eq!(prompt.mode, None);
        assert_eq!(
            prompt.working_directory.as_deref(),
            Some("/workspace/apps/code-agent")
        );
        assert_eq!(
            prompt.content,
            ApprovalContent {
                kind: ApprovalContentKind::Command,
                preview: vec!["$ cargo test -p code-agent".to_string()],
            }
        );
    }

    #[test]
    fn approval_prompt_extracts_monitor_start_context() {
        let prompt = approval_prompt_from_request(&ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: "call-monitor".into(),
                tool_name: "monitor_start".into(),
                arguments: json!({
                    "cmd": "npm run dev",
                    "workdir": "/workspace/web"
                }),
                origin: ToolOrigin::Local,
            },
            spec: ToolSpec::function(
                "monitor_start",
                "start background monitor",
                json!({"type":"object"}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                ToolSource::Builtin,
            ),
            reasons: vec!["host command requires review".to_string()],
        });

        assert_eq!(prompt.tool_name, "monitor_start");
        assert_eq!(prompt.working_directory.as_deref(), Some("/workspace/web"));
        assert_eq!(
            prompt.content,
            ApprovalContent {
                kind: ApprovalContentKind::Command,
                preview: vec![
                    "$ npm run dev".to_string(),
                    "cwd /workspace/web".to_string()
                ],
            }
        );
    }
}
