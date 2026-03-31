use crate::{ProviderError, Result};
use serde_json::{Map, Value};
use types::{
    MessagePart, ReasoningContent, ToolKind, ToolResult, ToolSpec, reference_display_text,
};

#[must_use]
pub fn coerce_object_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if schema.get("type").is_none() && schema.get("properties").is_some() {
        if let Some(object) = schema.as_object_mut() {
            object.insert("type".to_string(), Value::String("object".to_string()));
        }
    }
    schema
}

pub fn merge_top_level_object(
    target: &mut Map<String, Value>,
    additional_params: Option<&Value>,
    provider_name: &str,
) -> Result<()> {
    let Some(additional_params) = additional_params else {
        return Ok(());
    };
    let object = additional_params.as_object().ok_or_else(|| {
        ProviderError::config(format!(
            "{provider_name} additional_params must be a JSON object when provided"
        ))
    })?;
    for (key, value) in object {
        target.insert(key.clone(), value.clone());
    }
    Ok(())
}

#[must_use]
pub fn render_instruction_text(values: &[String]) -> Option<String> {
    let rendered = values
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if rendered.is_empty() {
        None
    } else {
        Some(rendered)
    }
}

#[must_use]
pub fn stringify_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

#[must_use]
pub fn data_url(mime_type: &str, data_base64: &str) -> String {
    format!("data:{mime_type};base64,{data_base64}")
}

#[must_use]
pub fn message_parts_text(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(message_part_text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[must_use]
pub fn message_part_text(part: &MessagePart) -> Option<String> {
    match part {
        MessagePart::Text { text } => Some(text.clone()),
        MessagePart::Reasoning { reasoning } => Some(
            reasoning
                .content
                .iter()
                .filter_map(|content| match content {
                    ReasoningContent::Text { text, .. } => Some(text.clone()),
                    ReasoningContent::Redacted { data } => Some(data.clone()),
                    ReasoningContent::Summary(summary) => Some(summary.clone()),
                    ReasoningContent::Encrypted(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .filter(|text| !text.is_empty()),
        MessagePart::Resource {
            text: Some(text), ..
        } => Some(text.clone()),
        MessagePart::Resource {
            text: None, uri, ..
        } => Some(uri.clone()),
        MessagePart::Json { value } => Some(stringify_json(value)),
        MessagePart::ProviderExtension { payload, .. } => Some(stringify_json(payload)),
        MessagePart::Reference {
            kind,
            name,
            uri,
            text,
        } => reference_display_text(kind, name.as_deref(), uri.as_deref(), text.as_deref()),
        MessagePart::File { uri: Some(uri), .. } => Some(uri.clone()),
        MessagePart::File {
            file_name: Some(file_name),
            mime_type,
            data_base64: Some(_),
            ..
        } => Some(format!(
            "[file:{}{}]",
            file_name,
            mime_type
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default()
        )),
        MessagePart::File {
            file_name: None,
            mime_type,
            data_base64: Some(_),
            ..
        } => Some(format!(
            "[inline_file{}]",
            mime_type
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default()
        )),
        MessagePart::File {
            data_base64: None,
            uri: None,
            ..
        } => None,
        MessagePart::ImageUrl { url, .. } => Some(url.clone()),
        MessagePart::ToolResult { result } => Some(tool_result_roundtrip_text(result)),
        MessagePart::ToolCall { call } => Some(stringify_json(&call.arguments)),
        MessagePart::Image { .. } => None,
    }
}

#[must_use]
pub fn tool_result_roundtrip_text(result: &ToolResult) -> String {
    let plain_text_only = result
        .parts
        .iter()
        .all(|part| matches!(part, MessagePart::Text { .. }));
    if plain_text_only
        && result.attachments.is_empty()
        && result.structured_content.is_none()
        && result.continuation.is_none()
        && result.metadata.is_none()
    {
        return result.text_content();
    }

    // Some provider tool-result surfaces still only accept text. In that case we
    // degrade rich local results to a stable JSON envelope instead of flattening
    // them to prose. This keeps correlation ids, error state, multipart content,
    // and structured payloads attached to the round-trip text.
    let mut envelope = Map::new();
    envelope.insert(
        "type".to_string(),
        Value::String("agent_core_tool_result".to_string()),
    );
    envelope.insert(
        "version".to_string(),
        Value::Number(serde_json::Number::from(1)),
    );
    envelope.insert(
        "id".to_string(),
        Value::String(result.id.as_str().to_string()),
    );
    envelope.insert(
        "call_id".to_string(),
        Value::String(result.call_id.as_str().to_string()),
    );
    envelope.insert(
        "tool_name".to_string(),
        Value::String(result.tool_name.to_string()),
    );
    envelope.insert("is_error".to_string(), Value::Bool(result.is_error));
    let summary_text = result.text_content();
    if !summary_text.is_empty() {
        envelope.insert("summary_text".to_string(), Value::String(summary_text));
    }
    if !result.parts.is_empty() {
        envelope.insert(
            "content".to_string(),
            serde_json::to_value(&result.parts).unwrap_or(Value::Null),
        );
    }
    if !result.attachments.is_empty() {
        envelope.insert(
            "attachments".to_string(),
            serde_json::to_value(&result.attachments).unwrap_or(Value::Null),
        );
    }
    if let Some(structured_content) = &result.structured_content {
        envelope.insert("structured_content".to_string(), structured_content.clone());
    }
    if let Some(continuation) = &result.continuation {
        envelope.insert(
            "continuation".to_string(),
            serde_json::to_value(continuation).unwrap_or(Value::Null),
        );
    }
    if let Some(metadata) = &result.metadata {
        envelope.insert("metadata".to_string(), metadata.clone());
    }
    stringify_json(&Value::Object(envelope))
}

#[must_use]
pub fn tool_schema(spec: &ToolSpec) -> Value {
    match spec.kind {
        ToolKind::Function => {
            let input_schema = spec
                .input_schema
                .as_ref()
                .expect("function tools must define an input schema");
            serde_json::json!({
                "type": "function",
                "name": spec.name,
                "description": spec.description,
                "parameters": coerce_object_schema(input_schema),
            })
        }
        ToolKind::Freeform => {
            let format = spec
                .freeform_format
                .as_ref()
                .expect("freeform tools must define a freeform format");
            serde_json::json!({
                "type": "custom",
                "name": spec.name,
                "description": spec.description,
                "format": serde_json::to_value(format).expect("freeform format"),
            })
        }
        ToolKind::Native => {
            panic!("native tools are host control surfaces and must not be exposed to providers")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{coerce_object_schema, message_part_text, tool_result_roundtrip_text, tool_schema};
    use serde_json::json;
    use types::{
        MessagePart, ToolAttachment, ToolContinuation, ToolFreeformFormat, ToolOrigin,
        ToolOutputMode, ToolResult, ToolSource, ToolSpec,
    };

    #[test]
    fn coerce_object_schema_adds_missing_type_for_property_schemas() {
        let schema = json!({
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        });

        let coerced = coerce_object_schema(&schema);

        assert_eq!(coerced["type"], json!("object"));
        assert_eq!(coerced["properties"]["path"]["type"], json!("string"));
    }

    #[test]
    fn tool_schema_uses_coerced_schema() {
        let spec = ToolSpec::function(
            "read",
            "Read a file",
            json!({
                "properties": {
                    "path": { "type": "string" }
                }
            }),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        );

        let definition = tool_schema(&spec);

        assert_eq!(definition["name"], json!("read"));
        assert_eq!(definition["parameters"]["type"], json!("object"));
    }

    #[test]
    fn tool_schema_emits_openai_custom_tools_for_freeform_specs() {
        let spec = ToolSpec::freeform(
            "apply_patch",
            "Apply a patch",
            ToolFreeformFormat::grammar("lark", "start: \"*** Begin Patch\""),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        );

        let definition = tool_schema(&spec);

        assert_eq!(definition["type"], json!("custom"));
        assert_eq!(definition["name"], json!("apply_patch"));
        assert_eq!(definition["format"]["type"], json!("grammar"));
        assert_eq!(definition["format"]["syntax"], json!("lark"));
    }

    #[test]
    fn tool_result_roundtrip_text_emits_structured_envelope_when_needed() {
        let result = ToolResult {
            id: "call_123".into(),
            call_id: "opaque_123".into(),
            tool_name: "list".into(),
            parts: vec![MessagePart::text("[list entries=1]")],
            attachments: vec![ToolAttachment {
                kind: "report".to_string(),
                name: Some("entries.json".to_string()),
                mime_type: Some("application/json".to_string()),
                uri: None,
                metadata: None,
            }],
            structured_content: Some(json!({
                "entries": [{"path": "src/lib.rs", "kind": "file"}]
            })),
            continuation: Some(ToolContinuation::FileWindow {
                snapshot_id: "snap_123".to_string(),
                selection_hash: Some("slice_123".to_string()),
                next_start_line: Some(41),
            }),
            metadata: Some(json!({
                "header": "[list entries=1]"
            })),
            is_error: false,
        };

        let rendered = tool_result_roundtrip_text(&result);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(parsed["type"], json!("agent_core_tool_result"));
        assert_eq!(parsed["version"], json!(1));
        assert_eq!(parsed["id"], json!("call_123"));
        assert_eq!(parsed["call_id"], json!("opaque_123"));
        assert_eq!(parsed["tool_name"], json!("list"));
        assert_eq!(parsed["is_error"], json!(false));
        assert_eq!(parsed["summary_text"], json!("[list entries=1]"));
        assert_eq!(parsed["attachments"][0]["kind"], json!("report"));
        assert_eq!(
            parsed["structured_content"]["entries"][0]["path"],
            json!("src/lib.rs")
        );
        assert_eq!(parsed["continuation"]["kind"], json!("file_window"));
        assert_eq!(parsed["continuation"]["next_start_line"], json!(41));
        assert_eq!(parsed["metadata"]["header"], json!("[list entries=1]"));
    }

    #[test]
    fn message_part_text_renders_reference_parts_without_json_fallback() {
        let part = MessagePart::reference(
            "mention",
            Some("workspace".to_string()),
            Some("app://workspace/snapshot".to_string()),
            None,
        );

        assert_eq!(message_part_text(&part).as_deref(), Some("workspace"));
    }
}
