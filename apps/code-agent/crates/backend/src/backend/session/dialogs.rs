use super::CodeAgentSession;
use crate::{
    ApprovalDecision, ApprovalPrompt, PermissionRequestPrompt, UserInputPrompt,
    interaction::{PermissionRequestDecision, UserInputSubmission},
};

impl CodeAgentSession {
    pub fn approval_prompt(&self) -> Option<ApprovalPrompt> {
        self.approvals.snapshot()
    }

    pub fn resolve_approval(&self, decision: ApprovalDecision) -> bool {
        self.approvals.resolve(decision)
    }

    pub fn user_input_prompt(&self) -> Option<UserInputPrompt> {
        self.user_inputs.snapshot()
    }

    pub fn resolve_user_input(&self, submission: UserInputSubmission) -> bool {
        self.user_inputs.resolve(submission)
    }

    pub fn cancel_user_input(&self, reason: impl Into<String>) -> bool {
        self.user_inputs.cancel(reason)
    }

    pub fn permission_request_prompt(&self) -> Option<PermissionRequestPrompt> {
        self.permission_requests.snapshot()
    }

    pub fn resolve_permission_request(&self, decision: PermissionRequestDecision) -> bool {
        self.permission_requests.resolve(decision)
    }
}
