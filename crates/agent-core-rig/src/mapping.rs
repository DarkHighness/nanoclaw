use agent_core_types::ToolSpec;
use rig::completion::ToolDefinition;
use serde_json::Value;

#[must_use]
pub fn to_rig_tool_definition(spec: &ToolSpec) -> ToolDefinition {
    ToolDefinition {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: coerce_object_schema(&spec.input_schema),
    }
}

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

#[cfg(test)]
mod tests {
    use super::{coerce_object_schema, to_rig_tool_definition};
    use agent_core_types::{ToolOrigin, ToolOutputMode, ToolSpec};
    use serde_json::json;
    use std::collections::BTreeMap;

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
    fn to_rig_tool_definition_uses_coerced_schema() {
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

        let definition = to_rig_tool_definition(&spec);

        assert_eq!(definition.name, "read");
        assert_eq!(definition.parameters["type"], json!("object"));
    }
}
