use crate::{ProviderError, Result};
use serde_json::{Map, Value};
use types::{MessagePart, ReasoningContent, ToolSpec};

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
        MessagePart::File { uri: Some(uri), .. } => Some(uri.clone()),
        MessagePart::File {
            data_base64: Some(_),
            ..
        } => Some("<inline file payload>".to_string()),
        MessagePart::File {
            data_base64: None,
            uri: None,
            ..
        } => None,
        MessagePart::ToolResult { result } => Some(result.text_content()),
        MessagePart::ToolCall { call } => Some(stringify_json(&call.arguments)),
        MessagePart::Image { .. } => None,
    }
}

#[must_use]
pub fn tool_schema(spec: &ToolSpec) -> Value {
    serde_json::json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": coerce_object_schema(&spec.input_schema),
    })
}

#[cfg(test)]
mod tests {
    use super::{coerce_object_schema, tool_schema};
    use serde_json::json;
    use std::collections::BTreeMap;
    use types::{ToolOrigin, ToolOutputMode, ToolSpec};

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
        let spec = ToolSpec {
            name: "read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({
                "properties": {
                    "path": { "type": "string" }
                }
            }),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: BTreeMap::new(),
        };

        let definition = tool_schema(&spec);

        assert_eq!(definition["name"], json!("read"));
        assert_eq!(definition["parameters"]["type"], json!("object"));
    }
}
