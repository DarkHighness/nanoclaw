use serde_json::{Value, json};
use std::collections::BTreeMap;

#[must_use]
pub fn mcp_tool_annotations(
    title: impl Into<String>,
    read_only_hint: bool,
    destructive_hint: bool,
    idempotent_hint: bool,
    open_world_hint: bool,
) -> BTreeMap<String, Value> {
    let title = title.into();
    BTreeMap::from([
        ("title".to_string(), Value::String(title.clone())),
        (
            "mcp_annotations".to_string(),
            json!({
                "title": title,
                "readOnlyHint": read_only_hint,
                "destructiveHint": destructive_hint,
                "idempotentHint": idempotent_hint,
                "openWorldHint": open_world_hint,
            }),
        ),
    ])
}
