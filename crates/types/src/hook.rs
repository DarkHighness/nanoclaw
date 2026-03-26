use crate::{RunId, SessionId, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    SessionStart,
    InstructionsLoaded,
    UserPromptSubmit,
    PreToolUse,
    PermissionRequest,
    PostToolUse,
    PostToolUseFailure,
    Notification,
    SubagentStart,
    SubagentStop,
    Stop,
    StopFailure,
    ConfigChange,
    PreCompact,
    PostCompact,
    SessionEnd,
    Elicitation,
    ElicitationResult,
}

impl HookEvent {
    #[must_use]
    pub fn default_match_field(self) -> Option<&'static str> {
        match self {
            HookEvent::PreToolUse
            | HookEvent::PermissionRequest
            | HookEvent::PostToolUse
            | HookEvent::PostToolUseFailure => Some("tool_name"),
            HookEvent::Elicitation | HookEvent::ElicitationResult => Some("mcp_server_name"),
            HookEvent::SessionStart
            | HookEvent::InstructionsLoaded
            | HookEvent::ConfigChange
            | HookEvent::Stop
            | HookEvent::StopFailure => Some("reason"),
            HookEvent::SubagentStart | HookEvent::SubagentStop => Some("agent_name"),
            HookEvent::Notification => Some("source"),
            HookEvent::UserPromptSubmit
            | HookEvent::PreCompact
            | HookEvent::PostCompact
            | HookEvent::SessionEnd => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookMatcher {
    pub pattern: String,
    pub field: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandHookHandler {
    pub command: String,
    #[serde(default)]
    pub asynchronous: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHookHandler {
    pub url: String,
    #[serde(default = "default_http_method")]
    pub method: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_http_method() -> String {
    "POST".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptHookHandler {
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHookHandler {
    pub prompt: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookHandler {
    Command(CommandHookHandler),
    Http(HttpHookHandler),
    Prompt(PromptHookHandler),
    Agent(AgentHookHandler),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRegistration {
    pub name: String,
    pub event: HookEvent,
    pub matcher: Option<HookMatcher>,
    pub handler: HookHandler,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionBehavior {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    Allow,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookDecision {
    PreToolUse {
        permission_decision: PermissionDecision,
    },
    PermissionRequest {
        behavior: PermissionBehavior,
        reason: Option<String>,
    },
    Gate {
        decision: GateDecision,
        reason: Option<String>,
    },
    Elicitation {
        action: ElicitationAction,
        content: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HookOutput {
    #[serde(default = "default_true")]
    pub r#continue: bool,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub system_message: Option<String>,
    #[serde(default)]
    pub additional_context: Vec<String>,
    #[serde(default)]
    pub decision: Option<HookDecision>,
}

fn default_true() -> bool {
    true
}

impl Default for HookOutput {
    fn default() -> Self {
        Self {
            r#continue: true,
            stop_reason: None,
            system_message: None,
            additional_context: Vec::new(),
            decision: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HookContext {
    pub event: HookEvent,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub payload: Value,
}

impl HookContext {
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
}
