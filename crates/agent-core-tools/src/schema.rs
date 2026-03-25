use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaTarget {
    OpenAi,
    Anthropic,
    GeminiLike,
    XAiLike,
}

#[must_use]
pub fn normalize_schema_for_target(schema: &Value, target: SchemaTarget) -> Value {
    let mut normalized = schema.clone();
    if matches!(target, SchemaTarget::OpenAi)
        && normalized.get("type").is_none()
        && normalized.get("properties").is_some()
    {
        if let Some(object) = normalized.as_object_mut() {
            object.insert("type".to_string(), Value::String("object".to_string()));
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{SchemaTarget, normalize_schema_for_target};

    #[test]
    fn openai_normalization_adds_object_type() {
        let schema = serde_json::json!({"properties":{"path":{"type":"string"}}});
        let normalized = normalize_schema_for_target(&schema, SchemaTarget::OpenAi);
        assert_eq!(normalized["type"], "object");
    }
}
