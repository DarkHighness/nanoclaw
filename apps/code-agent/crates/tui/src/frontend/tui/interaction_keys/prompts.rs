use super::*;

impl CodeAgentTui {
    pub(crate) fn handle_approval_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.approval_prompt() else {
            return false;
        };
        if let Some(decision) = approval_decision_for_key(key) {
            let approved = matches!(decision, crate::interaction::ApprovalDecision::Approve);
            if self.resolve_approval(decision) {
                self.ui_state.mutate(|state| {
                    if approved {
                        state.status = format!("Approved {}", prompt.tool_name);
                        state.turn_phase = super::super::state::TurnPhase::Working;
                        state.push_activity(format!("approved {}", prompt.tool_name));
                    } else {
                        state.status = format!("Denied {}", prompt.tool_name);
                        state.turn_phase = super::super::state::TurnPhase::Failed;
                        state.push_activity(format!("denied {}", prompt.tool_name));
                    }
                });
            }
            return true;
        }
        true
    }

    pub(crate) fn handle_permission_request_key(&mut self, key: KeyEvent) -> bool {
        let Some(_prompt) = self.permission_request_prompt() else {
            return false;
        };
        let decision = match key.code {
            KeyCode::Char('y') => Some(PermissionRequestDecision::GrantOnce),
            KeyCode::Char('a') => Some(PermissionRequestDecision::GrantForSession),
            KeyCode::Char('n') | KeyCode::Esc => Some(PermissionRequestDecision::Deny),
            _ => None,
        };
        if let Some(decision) = decision {
            if self.resolve_permission_request(decision) {
                self.ui_state.mutate(|state| match decision {
                    PermissionRequestDecision::GrantOnce => {
                        state.status = "Granted additional permissions for the turn".to_string();
                        state.turn_phase = super::super::state::TurnPhase::Working;
                        state.push_activity("granted additional permissions for the turn");
                    }
                    PermissionRequestDecision::GrantForSession => {
                        state.status = "Granted additional permissions for the session".to_string();
                        state.turn_phase = super::super::state::TurnPhase::Working;
                        state.push_activity("granted additional permissions for the session");
                    }
                    PermissionRequestDecision::Deny => {
                        state.status = "Denied additional permissions".to_string();
                        state.turn_phase = super::super::state::TurnPhase::Failed;
                        state.push_activity("denied additional permissions");
                    }
                });
            }
            return true;
        }
        true
    }
}
