use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub tool_name: String,
    pub origin: String,
    pub mode: Option<String>,
    pub working_directory: Option<String>,
    pub content_label: String,
    pub content_preview: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny { reason: Option<String> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionProfile {
    pub read_roots: Vec<String>,
    pub write_roots: Vec<String>,
    pub network_full: bool,
    pub network_domains: Vec<String>,
}

impl PermissionProfile {
    pub fn is_empty(&self) -> bool {
        self.read_roots.is_empty()
            && self.write_roots.is_empty()
            && !self.network_full
            && self.network_domains.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionRequestDecision {
    GrantOnce,
    GrantForSession,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRequestPrompt {
    pub prompt_id: String,
    pub reason: Option<String>,
    pub requested: PermissionProfile,
    pub current_turn: PermissionProfile,
    pub current_session: PermissionProfile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<UserInputOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInputPrompt {
    pub prompt_id: String,
    pub questions: Vec<UserInputQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserInputAnswer {
    pub answers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserInputSubmission {
    pub answers: BTreeMap<String, UserInputAnswer>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionPermissionMode {
    #[default]
    Default,
    DangerFullAccess,
}

impl SessionPermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPermissionModeOutcome {
    pub previous: SessionPermissionMode,
    pub current: SessionPermissionMode,
    pub sandbox_summary: String,
    pub host_process_surfaces_allowed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelReasoningEffortOutcome {
    pub previous: Option<String>,
    pub current: Option<String>,
    pub supported: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingControlKind {
    Prompt,
    Steer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingControlSummary {
    pub id: String,
    pub kind: PendingControlKind,
    pub preview: String,
    pub reason: Option<String>,
}
