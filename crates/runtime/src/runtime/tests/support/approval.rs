use crate::{Result, ToolApprovalHandler, ToolApprovalOutcome, ToolApprovalRequest};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Default)]
pub(in crate::runtime::tests) struct MockApprovalHandler {
    requests: Mutex<Vec<ToolApprovalRequest>>,
    outcomes: Mutex<VecDeque<ToolApprovalOutcome>>,
}

impl MockApprovalHandler {
    pub(in crate::runtime::tests) fn with_outcomes(
        outcomes: impl IntoIterator<Item = ToolApprovalOutcome>,
    ) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            outcomes: Mutex::new(outcomes.into_iter().collect()),
        }
    }

    pub(in crate::runtime::tests) fn requests(&self) -> Vec<ToolApprovalRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolApprovalHandler for MockApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> Result<ToolApprovalOutcome> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ToolApprovalOutcome::Approve))
    }
}
