use crate::{CallId, MessagePart, ToolCallId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputMode {
    Text,
    ContentParts,
}

/// Tool names are shared protocol identifiers across registry lookup,
/// approval policy, provider mapping, and persisted run events. Keeping them
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    pub output_mode: ToolOutputMode,
    pub origin: ToolOrigin,
    #[serde(default)]
    pub annotations: BTreeMap<String, Value>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: ToolCallId,
    #[serde(default = "CallId::new")]
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub parts: Vec<MessagePart>,
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
