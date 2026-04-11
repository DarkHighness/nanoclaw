use crate::interaction::ApprovalDecision;
pub(crate) use crate::interaction::ApprovalPrompt;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn approval_decision_for_key(key: KeyEvent) -> Option<ApprovalDecision> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(ApprovalDecision::Approve),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(ApprovalDecision::Deny {
            reason: Some("user denied tool call".to_string()),
        }),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ApprovalDecision::Deny {
                reason: Some("user cancelled tool approval".to_string()),
            })
        }
        _ => None,
    }
}
