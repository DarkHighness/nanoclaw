use agent::ToolOrigin;
use agent::runtime::{
    Result as RuntimeResult, RuntimeError, ToolApprovalHandler, ToolApprovalOutcome,
    ToolApprovalRequest,
};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalPrompt {
    pub(crate) tool_name: String,
    pub(crate) origin: String,
    pub(crate) reasons: Vec<String>,
    pub(crate) arguments_preview: Vec<String>,
}

impl ApprovalPrompt {
    pub(crate) fn from_request(request: &ToolApprovalRequest) -> Self {
        Self {
            tool_name: request.call.tool_name.to_string(),
            origin: tool_origin_label(&request.call.origin),
            reasons: request.reasons.clone(),
            arguments_preview: truncate_preview(&request.call.arguments.to_string(), 14, 72),
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

fn tool_origin_label(origin: &ToolOrigin) -> String {
    match origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    }
}

fn truncate_preview(value: &str, max_lines: usize, max_columns: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in value.lines() {
        if lines.len() == max_lines {
            lines.push("...".to_string());
            break;
        }
        let clipped = if line.chars().count() > max_columns {
            format!(
                "{}...",
                line.chars()
                    .take(max_columns.saturating_sub(3))
                    .collect::<String>()
            )
        } else {
            line.to_string()
        };
        lines.push(clipped);
    }
    if lines.is_empty() {
        lines.push("<empty>".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{ApprovalCoordinator, ApprovalDecision};

    #[test]
    fn resolving_missing_request_is_a_noop() {
        assert!(!ApprovalCoordinator::default().resolve(ApprovalDecision::Approve));
    }
}
