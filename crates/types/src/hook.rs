use crate::{Message, MessageId, MessagePart, MessageRole, RunId, SessionId, ToolName, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    pub allowed_tools: Vec<ToolName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmHookHandler {
    pub module: String,
    pub entrypoint: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookHandlerKind {
    Command,
    Http,
    Prompt,
    Agent,
    Wasm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookHandler {
    Command(CommandHookHandler),
    Http(HttpHookHandler),
    Prompt(PromptHookHandler),
    Agent(AgentHookHandler),
    Wasm(WasmHookHandler),
}

impl HookHandler {
    #[must_use]
    pub fn kind(&self) -> HookHandlerKind {
        match self {
            Self::Command(_) => HookHandlerKind::Command,
            Self::Http(_) => HookHandlerKind::Http,
            Self::Prompt(_) => HookHandlerKind::Prompt,
            Self::Agent(_) => HookHandlerKind::Agent,
            Self::Wasm(_) => HookHandlerKind::Wasm,
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookMutationPermission {
    #[default]
    Deny,
    Allow,
    ReviewRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookHostApiGrant {
    GetHookContext,
    EmitHookEffect,
    Log,
    ReadFile,
    WriteFile,
    ListDir,
    SpawnMcp,
    ResolveSkill,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HookEffectPolicy {
    #[serde(default)]
    pub message_mutation: HookMutationPermission,
    #[serde(default)]
    pub allow_context_injection: bool,
    #[serde(default)]
    pub allow_instruction_injection: bool,
    #[serde(default)]
    pub allow_tool_arg_rewrite: bool,
    #[serde(default)]
    pub allow_permission_decision: bool,
    #[serde(default)]
    pub allow_gate_decision: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookNetworkPolicy {
    Deny,
    Allow,
    AllowDomains { domains: Vec<String> },
}

impl Default for HookNetworkPolicy {
    fn default() -> Self {
        Self::Deny
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HookExecutionPolicy {
    #[serde(default)]
    pub plugin_id: Option<String>,
    #[serde(default)]
    pub plugin_root: Option<PathBuf>,
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
    #[serde(default)]
    pub write_roots: Vec<PathBuf>,
    #[serde(default)]
    pub exec_roots: Vec<PathBuf>,
    #[serde(default)]
    pub network: HookNetworkPolicy,
    #[serde(default)]
    pub host_api_grants: Vec<HookHostApiGrant>,
    #[serde(default)]
    pub effects: HookEffectPolicy,
}

impl HookExecutionPolicy {
    #[must_use]
    pub fn allows_host_api(&self, grant: HookHostApiGrant) -> bool {
        self.host_api_grants
            .iter()
            .any(|candidate| *candidate == grant)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRegistration {
    pub name: String,
    pub event: HookEvent,
    pub matcher: Option<HookMatcher>,
    pub handler: HookHandler,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub execution: Option<HookExecutionPolicy>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageSelector {
    Current,
    // `message_id` may only target an existing message in the current visible
    // transcript. Mutating compacted or otherwise hidden history would break
    // summary and continuation semantics, so the runtime rejects it explicitly.
    MessageId { message_id: MessageId },
    // `last_of_role` only scans the current visible transcript for the latest
    // persisted message with the requested role. It never targets the in-flight
    // `Current` message; use `Current` when mutating the message being built now.
    LastOfRole { role: MessageRole },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MessagePatch {
    #[serde(default)]
    pub role: Option<MessageRole>,
    #[serde(default)]
    pub replace_parts: Option<Vec<MessagePart>>,
    #[serde(default)]
    pub append_parts: Vec<MessagePart>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookEffect {
    AppendMessage {
        role: MessageRole,
        parts: Vec<MessagePart>,
    },
    ReplaceMessage {
        selector: MessageSelector,
        message: Message,
    },
    PatchMessage {
        selector: MessageSelector,
        patch: MessagePatch,
    },
    RemoveMessage {
        selector: MessageSelector,
    },
    AddContext {
        text: String,
    },
    SetPermissionDecision {
        decision: PermissionDecision,
        reason: Option<String>,
    },
    SetPermissionBehavior {
        behavior: PermissionBehavior,
        reason: Option<String>,
    },
    SetGateDecision {
        decision: GateDecision,
        reason: Option<String>,
    },
    Elicitation {
        action: ElicitationAction,
        content: Option<String>,
    },
    RewriteToolArgs {
        tool_name: ToolName,
        arguments: Value,
    },
    InjectInstruction {
        text: String,
    },
    Stop {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct HookResult {
    #[serde(default)]
    pub effects: Vec<HookEffect>,
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

#[cfg(test)]
mod tests {
    use super::{HookEffect, MessagePart, MessageRole, MessageSelector};
    use crate::MessageId;

    #[test]
    fn current_selector_round_trips() {
        let json = serde_json::to_string(&HookEffect::RemoveMessage {
            selector: MessageSelector::Current,
        })
        .unwrap();

        let parsed = serde_json::from_str::<HookEffect>(&json).unwrap();
        assert_eq!(
            parsed,
            HookEffect::RemoveMessage {
                selector: MessageSelector::Current,
            }
        );
    }

    #[test]
    fn unknown_message_selectors_are_rejected() {
        let patch = serde_json::json!({
            "kind": "patch_message",
            "selector": {
                "kind": "message_index",
                "index": 3
            },
            "patch": {
                "append_parts": [{ "type": "text", "text": "patched" }]
            }
        });

        let patch_error = serde_json::from_value::<HookEffect>(patch).unwrap_err();

        assert!(patch_error.to_string().contains("unknown variant"));
    }

    #[test]
    fn replace_message_with_current_selector_still_deserializes() {
        let value = serde_json::json!({
            "kind": "replace_message",
            "selector": { "kind": "current" },
            "message": {
                "role": "assistant",
                "parts": [{ "type": "text", "text": "rewritten" }]
            }
        });

        let parsed = serde_json::from_value::<HookEffect>(value).unwrap();
        let HookEffect::ReplaceMessage { selector, message } = parsed else {
            panic!("expected replace_message effect");
        };
        assert_eq!(selector, MessageSelector::Current);
        assert_eq!(message.role, MessageRole::Assistant);
        assert_eq!(
            message.parts,
            vec![MessagePart::Text {
                text: "rewritten".to_string(),
            }]
        );
    }

    #[test]
    fn replace_message_with_message_id_selector_deserializes() {
        let value = serde_json::json!({
            "kind": "replace_message",
            "selector": {
                "kind": "message_id",
                "message_id": "msg_1"
            },
            "message": {
                "role": "assistant",
                "parts": [{ "type": "text", "text": "rewritten" }]
            }
        });

        let parsed = serde_json::from_value::<HookEffect>(value).unwrap();
        let HookEffect::ReplaceMessage { selector, message } = parsed else {
            panic!("expected replace_message effect");
        };
        assert_eq!(
            selector,
            MessageSelector::MessageId {
                message_id: MessageId::from("msg_1"),
            }
        );
        assert_eq!(message.role, MessageRole::Assistant);
    }

    #[test]
    fn replace_message_with_last_of_role_selector_deserializes() {
        let value = serde_json::json!({
            "kind": "replace_message",
            "selector": {
                "kind": "last_of_role",
                "role": "assistant"
            },
            "message": {
                "role": "assistant",
                "parts": [{ "type": "text", "text": "rewritten" }]
            }
        });

        let parsed = serde_json::from_value::<HookEffect>(value).unwrap();
        let HookEffect::ReplaceMessage { selector, message } = parsed else {
            panic!("expected replace_message effect");
        };
        assert_eq!(
            selector,
            MessageSelector::LastOfRole {
                role: MessageRole::Assistant,
            }
        );
        assert_eq!(message.role, MessageRole::Assistant);
    }
}
