use crate::{CallId, MessagePart, ToolCallId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Borrow;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputMode {
    Text,
    ContentParts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    #[default]
    Function,
    Freeform,
    Native,
}

/// Tool names are shared protocol identifiers across registry lookup,
/// approval policy, provider mapping, and persisted session events.
/// Keeping them
/// typed inside the substrate avoids repeatedly degrading them into raw
/// strings before crossing a real JSON or UI boundary.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for ToolName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ToolName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ToolName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOrigin {
    Local,
    Mcp { server_name: String },
    Provider { provider: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSource {
    #[default]
    Builtin,
    Dynamic,
    Plugin {
        plugin: String,
    },
    McpTool {
        server_name: String,
    },
    McpResource {
        server_name: String,
    },
    ProviderBuiltin {
        provider: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolAvailability {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role_allowlist: Vec<String>,
    #[serde(default)]
    pub hidden_from_model: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolApprovalProfile {
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub mutates_state: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(default)]
    pub open_world: bool,
    #[serde(default)]
    pub needs_network: bool,
    #[serde(default)]
    pub needs_host_escape: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_message: Option<String>,
}

impl Default for ToolApprovalProfile {
    fn default() -> Self {
        Self {
            read_only: false,
            mutates_state: true,
            idempotent: None,
            open_world: true,
            needs_network: false,
            needs_host_escape: false,
            approval_message: None,
        }
    }
}

impl ToolApprovalProfile {
    #[must_use]
    pub fn new(
        read_only: bool,
        mutates_state: bool,
        idempotent: Option<bool>,
        open_world: bool,
    ) -> Self {
        Self {
            read_only,
            mutates_state,
            idempotent,
            open_world,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_network(mut self, needs_network: bool) -> Self {
        self.needs_network = needs_network;
        self
    }

    #[must_use]
    pub fn with_host_escape(mut self, needs_host_escape: bool) -> Self {
        self.needs_host_escape = needs_host_escape;
        self
    }

    #[must_use]
    pub fn with_approval_message(mut self, approval_message: impl Into<String>) -> Self {
        self.approval_message = Some(approval_message.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    #[serde(default)]
    pub kind: ToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    pub output_mode: ToolOutputMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub origin: ToolOrigin,
    #[serde(default)]
    pub source: ToolSource,
    #[serde(default)]
    pub aliases: Vec<ToolName>,
    #[serde(default)]
    pub supports_parallel_tool_calls: bool,
    #[serde(default)]
    pub availability: ToolAvailability,
    #[serde(default)]
    pub approval: ToolApprovalProfile,
}

impl ToolSpec {
    #[must_use]
    pub fn function(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        input_schema: Value,
        output_mode: ToolOutputMode,
        origin: ToolOrigin,
        source: ToolSource,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolKind::Function,
            input_schema: Some(input_schema),
            output_mode,
            output_schema: None,
            origin,
            source,
            aliases: Vec::new(),
            supports_parallel_tool_calls: false,
            availability: ToolAvailability::default(),
            approval: ToolApprovalProfile::default(),
        }
    }

    #[must_use]
    pub fn with_output_schema(mut self, output_schema: Value) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    #[must_use]
    pub fn with_approval(mut self, approval: ToolApprovalProfile) -> Self {
        self.approval = approval;
        self
    }

    #[must_use]
    pub fn with_parallel_support(mut self, supports_parallel_tool_calls: bool) -> Self {
        self.supports_parallel_tool_calls = supports_parallel_tool_calls;
        self
    }

    #[must_use]
    pub fn with_aliases(mut self, aliases: Vec<ToolName>) -> Self {
        self.aliases = aliases;
        self
    }

    #[must_use]
    pub fn with_availability(mut self, availability: ToolAvailability) -> Self {
        self.availability = availability;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    #[serde(default = "CallId::new")]
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub arguments: Value,
    pub origin: ToolOrigin,
}

/// Tool continuations carry the stable machine-readable cursor that a follow-up
/// call should reuse instead of scraping prose from the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolContinuation {
    FileWindow {
        snapshot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection_hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_start_line: Option<usize>,
    },
    StreamWindow {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_start_char: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_start_char: Option<usize>,
    },
    DocumentWindow {
        document_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_start_index: Option<usize>,
    },
}

/// Attachments describe side-band artifacts that hosts may render or persist
/// without forcing every provider transport to understand the local message-part
/// variants directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolAttachment {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: ToolCallId,
    #[serde(default = "CallId::new")]
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub parts: Vec<MessagePart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ToolContinuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub is_error: bool,
}

impl ToolResult {
    #[must_use]
    pub fn text(id: ToolCallId, tool_name: impl Into<ToolName>, text: impl Into<String>) -> Self {
        Self {
            id,
            call_id: CallId::new(),
            tool_name: tool_name.into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: None,
            continuation: None,
            metadata: None,
            is_error: false,
        }
    }

    #[must_use]
    pub fn error(id: ToolCallId, tool_name: impl Into<ToolName>, text: impl Into<String>) -> Self {
        Self {
            id,
            call_id: CallId::new(),
            tool_name: tool_name.into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: None,
            continuation: None,
            metadata: None,
            is_error: true,
        }
    }

    #[must_use]
    pub fn with_call_id(mut self, call_id: impl Into<CallId>) -> Self {
        self.call_id = call_id.into();
        self
    }

    #[must_use]
    pub fn with_structured_content(mut self, structured_content: Value) -> Self {
        self.structured_content = Some(structured_content);
        self
    }

    #[must_use]
    pub fn with_continuation(mut self, continuation: ToolContinuation) -> Self {
        self.continuation = Some(continuation);
        self
    }

    #[must_use]
    pub fn with_attachment(mut self, attachment: ToolAttachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    #[must_use]
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
