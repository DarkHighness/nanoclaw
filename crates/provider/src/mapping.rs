use crate::{ProviderError, Result};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use types::{
    MessagePart, ReasoningContent, ToolKind, ToolResult, ToolSpec, reference_display_text,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderToolTransport {
    Function,
    Freeform,
}

#[must_use]
pub fn coerce_object_schema(schema: &Value) -> Value {
    // OpenAI Responses is noticeably less tolerant than the broader JSON Schema
    // ecosystem. Forwarding raw schemars output (especially `$ref`, `$defs`,
    // `allOf`, or boolean schemas) can turn the whole request into a backend
    // error. Compile the provider-facing tool schema into a small stable subset
    // instead of mutating the original value in place.
    compile_tool_schema(schema, schema, &mut BTreeSet::new())
}

fn compile_tool_schema(value: &Value, root: &Value, seen_refs: &mut BTreeSet<String>) -> Value {
    if let Some(reference) = value
        .as_object()
        .and_then(|map| map.get("$ref"))
        .and_then(Value::as_str)
    {
        return compile_ref_schema(reference, root, seen_refs);
    }

    match value {
        Value::Bool(_) => {
            // Provider tool surfaces do not accept JSON Schema boolean shorthand.
            // Downgrading to a permissive string schema is safer than forwarding
            // `true` / `false` and letting the backend reject the whole request.
            json!({ "type": "string" })
        }
        Value::Object(map) => compile_schema_object(map, root, seen_refs),
        _ => json!({ "type": "string" }),
    }
}

fn compile_ref_schema(reference: &str, root: &Value, seen_refs: &mut BTreeSet<String>) -> Value {
    if !seen_refs.insert(reference.to_string()) {
        return json!({ "type": "string" });
    }
    let compiled = resolve_local_schema_ref(root, reference)
        .map(|target| compile_tool_schema(target, root, seen_refs))
        .unwrap_or_else(|| json!({ "type": "string" }));
    seen_refs.remove(reference);
    compiled
}

fn compile_schema_object(
    map: &Map<String, Value>,
    root: &Value,
    seen_refs: &mut BTreeSet<String>,
) -> Value {
    if let Some(combined) = compile_combined_object_schema(map, root, seen_refs) {
        return combined;
    }

    match infer_schema_type(map).as_str() {
        "object" => compile_object_schema(map, root, seen_refs),
        "array" => compile_array_schema(map, root, seen_refs),
        "number" | "integer" => compile_scalar_schema("number", map),
        "boolean" => compile_scalar_schema("boolean", map),
        _ => compile_scalar_schema("string", map),
    }
}

fn compile_combined_object_schema(
    map: &Map<String, Value>,
    root: &Value,
    seen_refs: &mut BTreeSet<String>,
) -> Option<Value> {
    if normalized_schema_type(map.get("type")).is_some()
        || map.contains_key("properties")
        || map.contains_key("required")
        || map.contains_key("additionalProperties")
    {
        return None;
    }

    for combiner in ["allOf", "anyOf", "oneOf"] {
        let Some(Value::Array(variants)) = map.get(combiner) else {
            continue;
        };

        let compiled_objects = variants
            .iter()
            .map(|variant| compile_tool_schema(variant, root, seen_refs))
            .filter_map(|variant| match variant {
                Value::Object(object)
                    if object.get("type").and_then(Value::as_str) == Some("object") =>
                {
                    Some(object)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if compiled_objects.is_empty() {
            continue;
        }

        let mut properties = Map::new();
        let mut required = BTreeSet::new();
        let mut additional_properties = None;
        for object in compiled_objects {
            if let Some(object_properties) = object.get("properties").and_then(Value::as_object) {
                for (key, value) in object_properties {
                    properties.insert(key.clone(), value.clone());
                }
            }
            if let Some(object_required) = object.get("required").and_then(Value::as_array) {
                for field in object_required.iter().filter_map(Value::as_str) {
                    required.insert(field.to_string());
                }
            }
            if additional_properties.is_none() {
                additional_properties = object.get("additionalProperties").cloned();
            }
        }

        let mut compiled = Map::new();
        compiled.insert("type".to_string(), Value::String("object".to_string()));
        compiled.insert("properties".to_string(), Value::Object(properties));
        insert_description(&mut compiled, map);
        if !required.is_empty() {
            compiled.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        if let Some(additional_properties) = additional_properties {
            compiled.insert("additionalProperties".to_string(), additional_properties);
        }
        return Some(Value::Object(compiled));
    }

    None
}

fn compile_object_schema(
    map: &Map<String, Value>,
    root: &Value,
    seen_refs: &mut BTreeSet<String>,
) -> Value {
    let properties = map
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(key, value)| (key.clone(), compile_tool_schema(value, root, seen_refs)))
                .collect::<Map<String, Value>>()
        })
        .unwrap_or_default();

    let mut compiled = Map::new();
    compiled.insert("type".to_string(), Value::String("object".to_string()));
    compiled.insert("properties".to_string(), Value::Object(properties));
    insert_description(&mut compiled, map);
    if let Some(required) = normalized_required(map.get("required")) {
        compiled.insert(
            "required".to_string(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(additional_properties) =
        compile_additional_properties(map.get("additionalProperties"), root, seen_refs)
    {
        compiled.insert("additionalProperties".to_string(), additional_properties);
    }
    Value::Object(compiled)
}

fn compile_array_schema(
    map: &Map<String, Value>,
    root: &Value,
    seen_refs: &mut BTreeSet<String>,
) -> Value {
    let items = map
        .get("items")
        .map(|items| compile_tool_schema(items, root, seen_refs))
        .unwrap_or_else(|| json!({ "type": "string" }));

    let mut compiled = Map::new();
    compiled.insert("type".to_string(), Value::String("array".to_string()));
    compiled.insert("items".to_string(), items);
    insert_description(&mut compiled, map);
    Value::Object(compiled)
}

fn compile_scalar_schema(schema_type: &str, map: &Map<String, Value>) -> Value {
    let mut compiled = Map::new();
    compiled.insert("type".to_string(), Value::String(schema_type.to_string()));
    insert_description(&mut compiled, map);
    Value::Object(compiled)
}

fn compile_additional_properties(
    value: Option<&Value>,
    root: &Value,
    seen_refs: &mut BTreeSet<String>,
) -> Option<Value> {
    match value {
        Some(Value::Bool(allowed)) => Some(Value::Bool(*allowed)),
        Some(schema) => Some(compile_tool_schema(schema, root, seen_refs)),
        None => None,
    }
}

fn insert_description(target: &mut Map<String, Value>, map: &Map<String, Value>) {
    if let Some(description) = map
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        target.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
}

fn normalized_required(value: Option<&Value>) -> Option<Vec<String>> {
    let required = value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (!required.is_empty()).then_some(required)
}

fn resolve_local_schema_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let path = reference.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        let token = segment.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => map.get(token.as_str())?,
            Value::Array(values) => values.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn infer_schema_type(map: &Map<String, Value>) -> String {
    normalized_schema_type(map.get("type")).unwrap_or_else(|| {
        if map.contains_key("properties")
            || map.contains_key("required")
            || map.contains_key("additionalProperties")
        {
            "object".to_string()
        } else if map.contains_key("items") || map.contains_key("prefixItems") {
            "array".to_string()
        } else if map.contains_key("enum")
            || map.contains_key("const")
            || map.contains_key("format")
        {
            "string".to_string()
        } else if map.contains_key("minimum")
            || map.contains_key("maximum")
            || map.contains_key("exclusiveMinimum")
            || map.contains_key("exclusiveMaximum")
            || map.contains_key("multipleOf")
        {
            "number".to_string()
        } else {
            "string".to_string()
        }
    })
}

fn normalized_schema_type(value: Option<&Value>) -> Option<String> {
    const SUPPORTED: &[&str] = &["object", "array", "string", "number", "integer", "boolean"];

    match value {
        Some(Value::String(value)) if SUPPORTED.contains(&value.as_str()) => Some(value.clone()),
        Some(Value::Array(values)) => values.iter().find_map(|candidate| match candidate {
            Value::String(value) if SUPPORTED.contains(&value.as_str()) => Some(value.clone()),
            _ => None,
        }),
        _ => None,
    }
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
        MessagePart::Text { text } | MessagePart::InlineText { text } => Some(text.clone()),
        MessagePart::Paste { text, .. } => Some(text.clone()),
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
    let transport = match spec.kind {
        ToolKind::Freeform => ProviderToolTransport::Freeform,
        ToolKind::Function => ProviderToolTransport::Function,
        ToolKind::Native => {
            panic!("native tools are host control surfaces and must not be exposed to providers")
        }
    };
    tool_schema_for_transport(spec, transport)
}

#[must_use]
pub fn tool_schema_for_transport(spec: &ToolSpec, transport: ProviderToolTransport) -> Value {
    match transport {
        ProviderToolTransport::Function => {
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
        ProviderToolTransport::Freeform => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderToolTransport, coerce_object_schema, message_part_text, tool_result_roundtrip_text,
        tool_schema, tool_schema_for_transport,
    };
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
        assert!(coerced.get("allOf").is_none());
    }

    #[test]
    fn coerce_object_schema_sanitizes_nested_composition_and_array_defaults() {
        let schema = json!({
            "properties": {
                "payload": {
                    "type": ["object", "null"],
                    "additionalProperties": true
                },
                "targets": {
                    "type": "array"
                },
                "mode": {
                    "oneOf": [
                        true,
                        {
                            "enum": ["fast", "safe"]
                        }
                    ]
                }
            },
            "required": ["payload"]
        });

        let coerced = coerce_object_schema(&schema);

        assert_eq!(coerced["type"], json!("object"));
        assert_eq!(coerced["properties"]["payload"]["type"], json!("object"));
        assert_eq!(coerced["properties"]["payload"]["properties"], json!({}));
        assert_eq!(
            coerced["properties"]["payload"]["additionalProperties"],
            json!(true)
        );
        assert_eq!(coerced["properties"]["targets"]["type"], json!("array"));
        assert_eq!(
            coerced["properties"]["targets"]["items"]["type"],
            json!("string")
        );
        assert_eq!(coerced["properties"]["mode"]["type"], json!("string"));
        assert!(coerced["properties"]["mode"]["oneOf"].is_null());
    }

    #[test]
    fn coerce_object_schema_preserves_object_additional_properties_schema() {
        let schema = json!({
            "type": "object",
            "additionalProperties": {
                "required": ["value"],
                "properties": {
                    "value": {
                        "anyOf": [
                            { "type": "string" },
                            { "type": "number" }
                        ]
                    }
                }
            }
        });

        let coerced = coerce_object_schema(&schema);

        assert_eq!(coerced["type"], json!("object"));
        assert_eq!(coerced["properties"], json!({}));
        assert_eq!(coerced["additionalProperties"]["type"], json!("object"));
        assert_eq!(
            coerced["additionalProperties"]["required"],
            json!(["value"])
        );
        assert_eq!(
            coerced["additionalProperties"]["properties"]["value"]["type"],
            json!("string")
        );
    }

    #[test]
    fn coerce_object_schema_normalizes_nullable_object_roots() {
        let schema = json!({
            "type": ["null", "object"],
            "required": ["command"]
        });

        let coerced = coerce_object_schema(&schema);

        assert_eq!(coerced["type"], json!("object"));
        assert_eq!(coerced["properties"], json!({}));
    }

    #[test]
    fn coerce_object_schema_resolves_local_refs_and_merges_flatten_like_roots() {
        let schema = json!({
            "$defs": {
                "TaskBase": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string" },
                        "prompt": { "type": "string" }
                    },
                    "required": ["prompt"]
                },
                "TaskMeta": {
                    "type": "object",
                    "properties": {
                        "role": { "type": "string" }
                    }
                }
            },
            "allOf": [
                { "$ref": "#/$defs/TaskBase" },
                { "$ref": "#/$defs/TaskMeta" }
            ]
        });

        let coerced = coerce_object_schema(&schema);

        assert_eq!(coerced["type"], json!("object"));
        assert_eq!(coerced["properties"]["task_id"]["type"], json!("string"));
        assert_eq!(coerced["properties"]["prompt"]["type"], json!("string"));
        assert_eq!(coerced["properties"]["role"]["type"], json!("string"));
        assert_eq!(coerced["required"], json!(["prompt"]));
        assert!(coerced.get("$defs").is_none());
        assert!(coerced.get("allOf").is_none());
    }

    #[test]
    fn coerce_object_schema_normalizes_integer_scalars_to_number() {
        let schema = json!({
            "type": "object",
            "properties": {
                "page": { "type": "integer" }
            }
        });

        let coerced = coerce_object_schema(&schema);

        assert_eq!(coerced["properties"]["page"]["type"], json!("number"));
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
            "custom_diff",
            "Apply a diff",
            ToolFreeformFormat::grammar("lark", "start: \"*** Begin Patch\""),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        );

        let definition = tool_schema(&spec);

        assert_eq!(definition["type"], json!("custom"));
        assert_eq!(definition["name"], json!("custom_diff"));
        assert_eq!(definition["format"]["type"], json!("grammar"));
        assert_eq!(definition["format"]["syntax"], json!("lark"));
    }

    #[test]
    fn tool_schema_can_project_function_tools_as_freeform_when_requested() {
        let spec = ToolSpec::function(
            "patch_files",
            "Apply patch files",
            json!({"type":"object","properties":{"operations":{"type":"array"}}}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_freeform_format(ToolFreeformFormat::grammar("lark", "start: patch"));

        let definition = tool_schema_for_transport(&spec, ProviderToolTransport::Freeform);

        assert_eq!(definition["type"], json!("custom"));
        assert_eq!(definition["name"], json!("patch_files"));
        assert_eq!(definition["format"]["type"], json!("grammar"));
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
