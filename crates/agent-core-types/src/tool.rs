use crate::{MessagePart, ToolCallId, new_opaque_id};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputMode {
    Text,
    ContentParts,
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
    pub name: String,
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
    #[serde(default = "new_opaque_id", alias = "provider_call_id")]
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub origin: ToolOrigin,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: ToolCallId,
    #[serde(default = "new_opaque_id", alias = "provider_call_id")]
    pub call_id: String,
    pub tool_name: String,
    pub parts: Vec<MessagePart>,
    pub metadata: Option<Value>,
    pub is_error: bool,
}

impl ToolResult {
    #[must_use]
    pub fn text(id: ToolCallId, tool_name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id,
            call_id: new_opaque_id(),
            tool_name: tool_name.into(),
            parts: vec![MessagePart::text(text)],
            metadata: None,
            is_error: false,
        }
    }

    #[must_use]
    pub fn error(id: ToolCallId, tool_name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id,
            call_id: new_opaque_id(),
            tool_name: tool_name.into(),
            parts: vec![MessagePart::text(text)],
            metadata: None,
            is_error: true,
        }
    }

    #[must_use]
    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
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
