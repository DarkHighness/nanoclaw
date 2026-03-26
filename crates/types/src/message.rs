use crate::{MessageId, ToolCall, ToolCallId, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        data_base64: String,
    },
    File {
        file_name: Option<String>,
        mime_type: Option<String>,
        data_base64: Option<String>,
        uri: Option<String>,
    },
    ToolCall {
        call: ToolCall,
    },
    ToolResult {
        result: ToolResult,
    },
    Reasoning {
        reasoning: Reasoning,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
        metadata: Option<Value>,
    },
    Json {
        value: Value,
    },
    ProviderExtension {
        provider: String,
        kind: String,
        payload: Value,
    },
}

impl MessagePart {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
pub enum ReasoningContent {
    Text {
        text: String,
        #[serde(default)]
        signature: Option<String>,
    },
    Encrypted(String),
    Redacted {
        data: String,
    },
    Summary(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reasoning {
    #[serde(default)]
    pub id: Option<String>,
    pub content: Vec<ReasoningContent>,
}

impl Reasoning {
    #[must_use]
    pub fn display_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|item| match item {
                ReasoningContent::Text { text, .. } => Some(text.as_str()),
                ReasoningContent::Redacted { data } => Some(data.as_str()),
                ReasoningContent::Summary(summary) => Some(summary.as_str()),
                ReasoningContent::Encrypted(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub parts: Vec<MessagePart>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "MessageId::new")]
    pub message_id: MessageId,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl Message {
    #[must_use]
    pub fn new(role: MessageRole, parts: Vec<MessagePart>) -> Self {
        Self {
            role,
            parts,
            name: None,
            message_id: MessageId::new(),
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self::new(role, vec![MessagePart::text(text)])
    }

    #[must_use]
    pub fn system(text: impl Into<String>) -> Self {
        Self::text(MessageRole::System, text)
    }

    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self::text(MessageRole::User, text)
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(MessageRole::Assistant, text)
    }

    #[must_use]
    pub fn assistant_parts(parts: Vec<MessagePart>) -> Self {
        Self::new(MessageRole::Assistant, parts)
    }

    #[must_use]
    pub fn with_message_id(mut self, message_id: impl Into<MessageId>) -> Self {
        self.message_id = message_id.into();
        self
    }

    #[must_use]
    pub fn tool_result(result: ToolResult) -> Self {
        Self {
            role: MessageRole::Tool,
            parts: vec![MessagePart::ToolResult {
                result: result.clone(),
            }],
            name: Some(result.tool_name.clone()),
            message_id: MessageId::new(),
            metadata: Default::default(),
        }
    }

    #[must_use]
    pub fn tool_text(
        call_id: ToolCallId,
        name: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        let tool_name = name.into();
        Self::tool_result(ToolResult::text(call_id, tool_name, text))
    }

    #[must_use]
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text { text } => Some(text.clone()),
                MessagePart::Reasoning { reasoning } => {
                    let text = reasoning.display_text();
                    if text.is_empty() { None } else { Some(text) }
                }
                MessagePart::Resource {
                    text: Some(text), ..
                } => Some(text.clone()),
                MessagePart::ToolResult { result } => Some(result.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, MessagePart, MessageRole};

    #[test]
    fn text_content_joins_text_parts() {
        let message = Message::new(
            MessageRole::Assistant,
            vec![
                MessagePart::text("hello"),
                MessagePart::Json {
                    value: serde_json::json!({"ignored": true}),
                },
                MessagePart::text("world"),
            ],
        );

        assert_eq!(message.text_content(), "hello\nworld");
    }
}
