use crate::{MessageId, ReasoningId, ToolCall, ToolCallId, ToolName, ToolResult};
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
    ImageUrl {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    File {
        file_name: Option<String>,
        mime_type: Option<String>,
        data_base64: Option<String>,
        uri: Option<String>,
    },
    Reference {
        kind: String,
        name: Option<String>,
        uri: Option<String>,
        text: Option<String>,
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

    #[must_use]
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::ImageUrl {
            url: url.into(),
            mime_type: None,
        }
    }

    #[must_use]
    pub fn reference(
        kind: impl Into<String>,
        name: Option<String>,
        uri: Option<String>,
        text: Option<String>,
    ) -> Self {
        Self::Reference {
            kind: kind.into(),
            name,
            uri,
            text,
        }
    }
}

#[must_use]
pub fn file_display_text(
    file_name: Option<&str>,
    mime_type: Option<&str>,
    uri: Option<&str>,
) -> String {
    format!(
        "[file:{}{}{}]",
        file_name.unwrap_or("unnamed"),
        mime_type
            .map(|value| format!(" {value}"))
            .unwrap_or_default(),
        uri.map(|value| format!(" {value}")).unwrap_or_default(),
    )
}

#[must_use]
pub fn image_url_display_text(url: &str, mime_type: Option<&str>) -> String {
    format!(
        "[image_url:{}{}]",
        url,
        mime_type
            .map(|value| format!(" {value}"))
            .unwrap_or_default(),
    )
}

#[must_use]
pub fn resource_display_text(uri: &str, mime_type: Option<&str>, text: Option<&str>) -> String {
    format!(
        "[resource:{}{}{}]",
        uri,
        mime_type
            .map(|value| format!(" {value}"))
            .unwrap_or_default(),
        text.map(|value| format!(" {}", value.replace('\n', " ")))
            .unwrap_or_default(),
    )
}

#[must_use]
pub fn reference_display_text(
    kind: &str,
    name: Option<&str>,
    uri: Option<&str>,
    text: Option<&str>,
) -> Option<String> {
    match (text, name, uri) {
        (Some(text), Some(name), _) => Some(format!("{name}\n{text}")),
        (Some(text), None, Some(uri)) => Some(format!("{uri}\n{text}")),
        (Some(text), None, None) => Some(text.to_string()),
        (None, Some(name), _) => Some(name.to_string()),
        (None, None, Some(uri)) => Some(uri.to_string()),
        (None, None, None) => {
            let kind = kind.trim();
            (!kind.is_empty()).then(|| format!("[{kind}]"))
        }
    }
}

#[must_use]
pub fn reference_operator_text(
    kind: &str,
    name: Option<&str>,
    uri: Option<&str>,
    text: Option<&str>,
) -> String {
    format!(
        "[reference:{}{}{}{}]",
        kind,
        name.map(|value| format!(" {value}")).unwrap_or_default(),
        uri.map(|value| format!(" {value}")).unwrap_or_default(),
        text.map(|value| format!(" {}", value.replace('\n', " ")))
            .unwrap_or_default(),
    )
}

#[must_use]
pub fn message_part_display_text(part: &MessagePart) -> Option<String> {
    match part {
        MessagePart::Text { text } => Some(text.clone()),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            (!text.is_empty()).then_some(text)
        }
        MessagePart::Image { mime_type, .. } => Some(format!("[image:{mime_type}]")),
        MessagePart::ImageUrl { url, mime_type } => {
            Some(image_url_display_text(url, mime_type.as_deref()))
        }
        MessagePart::File {
            file_name,
            mime_type,
            uri,
            ..
        } => Some(file_display_text(
            file_name.as_deref(),
            mime_type.as_deref(),
            uri.as_deref(),
        )),
        MessagePart::Resource {
            text: Some(text), ..
        } => Some(text.clone()),
        MessagePart::Resource {
            text: None, uri, ..
        } => Some(uri.clone()),
        MessagePart::Reference {
            kind,
            name,
            uri,
            text,
        } => reference_display_text(kind, name.as_deref(), uri.as_deref(), text.as_deref()),
        MessagePart::ToolResult { result } => Some(result.text_content()),
        MessagePart::Json { .. }
        | MessagePart::ProviderExtension { .. }
        | MessagePart::ToolCall { .. } => None,
    }
}

/// Render a message part for operator-visible transcript, replay, and export
/// surfaces. Unlike `message_part_display_text`, this keeps structural
/// markers for non-text parts so different host views cannot silently drift.
#[must_use]
pub fn message_part_operator_text(part: &MessagePart) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            if text.is_empty() {
                "[reasoning]".to_string()
            } else {
                format!("[reasoning] {text}")
            }
        }
        MessagePart::Image { mime_type, .. } => format!("[image:{mime_type}]"),
        MessagePart::ImageUrl { url, mime_type } => {
            image_url_display_text(url, mime_type.as_deref())
        }
        MessagePart::File {
            file_name,
            mime_type,
            uri,
            ..
        } => file_display_text(file_name.as_deref(), mime_type.as_deref(), uri.as_deref()),
        MessagePart::Reference {
            kind,
            name,
            uri,
            text,
        } => reference_operator_text(kind, name.as_deref(), uri.as_deref(), text.as_deref()),
        MessagePart::ToolCall { call } => format!("[tool_call:{}]", call.tool_name),
        MessagePart::ToolResult { result } => {
            let text = result.text_content();
            if text.is_empty() {
                format!("[tool_result:{}]", result.tool_name)
            } else {
                format!("[tool_result:{}] {text}", result.tool_name)
            }
        }
        MessagePart::Resource {
            uri,
            mime_type,
            text,
            ..
        } => resource_display_text(uri, mime_type.as_deref(), text.as_deref()),
        MessagePart::Json { value } => format!("[json] {value}"),
        MessagePart::ProviderExtension { provider, kind, .. } => {
            format!("[provider_extension:{provider}:{kind}]")
        }
    }
}

#[must_use]
pub fn message_operator_text(message: &Message) -> String {
    message
        .parts
        .iter()
        .map(message_part_operator_text)
        .collect::<Vec<_>>()
        .join("\n")
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
    pub id: Option<ReasoningId>,
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
            name: Some(result.tool_name.to_string()),
            message_id: MessageId::new(),
            metadata: Default::default(),
        }
    }

    #[must_use]
    pub fn tool_text(
        call_id: ToolCallId,
        name: impl Into<ToolName>,
        text: impl Into<String>,
    ) -> Self {
        let tool_name = name.into();
        Self::tool_result(ToolResult::text(call_id, tool_name, text))
    }

    #[must_use]
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(message_part_display_text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, MessagePart, MessageRole, message_operator_text};
    use crate::{ToolCall, ToolCallId, ToolName};
    use serde_json::Value;

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

    #[test]
    fn text_content_includes_reference_display_text() {
        let message = Message::new(
            MessageRole::User,
            vec![MessagePart::reference(
                "mention",
                Some("workspace".to_string()),
                Some("app://workspace/snapshot".to_string()),
                None,
            )],
        );

        assert_eq!(message.text_content(), "workspace");
    }

    #[test]
    fn text_content_includes_attachment_placeholders() {
        let message = Message::new(
            MessageRole::User,
            vec![
                MessagePart::ImageUrl {
                    url: "https://example.com/failure.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                },
                MessagePart::File {
                    file_name: Some("report.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    data_base64: None,
                    uri: Some("https://example.com/report.pdf".to_string()),
                },
            ],
        );

        assert_eq!(
            message.text_content(),
            "[image_url:https://example.com/failure.png image/png]\n[file:report.pdf application/pdf https://example.com/report.pdf]"
        );
    }

    #[test]
    fn operator_text_keeps_structural_markers_for_non_text_parts() {
        let message = Message::new(
            MessageRole::Assistant,
            vec![
                MessagePart::Reference {
                    kind: "skill".to_string(),
                    name: Some("openai-docs".to_string()),
                    uri: None,
                    text: Some("Use official docs".to_string()),
                },
                MessagePart::ToolCall {
                    call: ToolCall {
                        id: ToolCallId::new(),
                        call_id: crate::CallId::new(),
                        tool_name: ToolName::from("read"),
                        arguments: Value::Null,
                        origin: crate::ToolOrigin::Local,
                    },
                },
            ],
        );

        assert_eq!(
            message_operator_text(&message),
            "[reference:skill openai-docs Use official docs]\n[tool_call:read]"
        );
    }
}
