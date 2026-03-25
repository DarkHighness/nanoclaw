use super::state::{SharedUiState, preview_text, truncate_preview};
use agent_core::ToolOrigin;
use agent_core::runtime::{
    Result as RuntimeResult, RuntimeError, ToolApprovalHandler, ToolApprovalOutcome,
    ToolApprovalRequest,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Clone, Debug)]
pub(crate) struct ApprovalPrompt {
    pub(crate) tool_name: String,
    pub(crate) origin: String,
    pub(crate) reasons: Vec<String>,
    pub(crate) arguments_preview: Vec<String>,
}

impl ApprovalPrompt {
    pub(crate) fn from_request(request: &ToolApprovalRequest) -> Self {
        Self {
            tool_name: request.call.tool_name.clone(),
            origin: tool_origin_label(&request.call.origin),
            reasons: request.reasons.clone(),
            arguments_preview: truncate_preview(&request.call.arguments.to_string(), 14, 72),
        }
    }
}

#[derive(Default)]
struct ApprovalBridgeState {
    prompt: Option<ApprovalPrompt>,
    responder: Option<oneshot::Sender<ToolApprovalOutcome>>,
}

#[derive(Clone, Default)]
pub(crate) struct ApprovalBridge {
    inner: Arc<Mutex<ApprovalBridgeState>>,
}

impl ApprovalBridge {
    pub(crate) fn present(
        &self,
        prompt: ApprovalPrompt,
        responder: oneshot::Sender<ToolApprovalOutcome>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        inner.prompt = Some(prompt);
        inner.responder = Some(responder);
    }

    pub(crate) fn snapshot(&self) -> Option<ApprovalPrompt> {
        self.inner.lock().unwrap().prompt.clone()
    }

    pub(crate) fn respond(&self, outcome: ToolApprovalOutcome) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let responder = inner.responder.take();
        inner.prompt = None;
        if let Some(responder) = responder {
            let _ = responder.send(outcome);
            true
        } else {
            false
        }
    }
}

pub(crate) struct InteractiveToolApprovalHandler {
    approval_bridge: ApprovalBridge,
    ui_state: SharedUiState,
}

impl InteractiveToolApprovalHandler {
    pub(crate) fn new(approval_bridge: ApprovalBridge, ui_state: SharedUiState) -> Self {
        Self {
            approval_bridge,
            ui_state,
        }
    }
}

#[async_trait]
impl ToolApprovalHandler for InteractiveToolApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> RuntimeResult<ToolApprovalOutcome> {
        self.ui_state.mutate(|state| {
            state.status = format!("Approval required for {}", request.call.tool_name);
            state.push_activity(format!(
                "approval needed for {} ({})",
                request.call.tool_name,
                preview_text(&request.reasons.join("; "), 40)
            ));
        });
        let prompt = ApprovalPrompt::from_request(&request);
        let (tx, rx) = oneshot::channel();
        self.approval_bridge.present(prompt, tx);
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
