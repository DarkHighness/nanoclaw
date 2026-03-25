use crate::Result;
use agent_core_types::{ToolCall, ToolSpec};
use async_trait::async_trait;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolApprovalRequest {
    pub call: ToolCall,
    pub spec: ToolSpec,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolApprovalOutcome {
    Approve,
    Deny { reason: Option<String> },
}

#[async_trait]
pub trait ToolApprovalHandler: Send + Sync {
    async fn decide(&self, request: ToolApprovalRequest) -> Result<ToolApprovalOutcome>;
}

#[derive(Default)]
pub struct AlwaysAllowToolApprovalHandler;

#[async_trait]
impl ToolApprovalHandler for AlwaysAllowToolApprovalHandler {
    async fn decide(&self, _request: ToolApprovalRequest) -> Result<ToolApprovalOutcome> {
        Ok(ToolApprovalOutcome::Approve)
    }
}
