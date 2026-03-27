use serde_json::{Value, json};
use std::collections::BTreeMap;
use types::{
    CallId, MessageId, Reasoning, ReasoningContent, ReasoningId, ToolCall, ToolCallId, ToolName,
    ToolOrigin,
};

pub(super) fn parse_openai_tool_call_item(
    item: &Value,
    tool_origins: &BTreeMap<ToolName, ToolOrigin>,
) -> Option<ToolCall> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let tool_name: ToolName = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .into();
    let origin = tool_origins
        .get(tool_name.as_str())
        .cloned()
        .unwrap_or_else(|| ToolOrigin::Provider {
            provider: "openai".to_string(),
        });
    Some(ToolCall {
        id: ToolCallId::from(item.get("id").and_then(Value::as_str).unwrap_or_default()),
        call_id: item
            .get("call_id")
            .and_then(Value::as_str)
            .map(CallId::from)
            .unwrap_or_else(CallId::new),
        tool_name,
        arguments: parse_openai_arguments(item.get("arguments")),
        origin,
    })
}

pub(super) fn parse_openai_reasoning_item(item: &Value) -> Option<Reasoning> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let content = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str))
        .map(|text| ReasoningContent::Summary(text.to_string()))
        .collect::<Vec<_>>();
    let mut content = content;
    if let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) {
        content.push(ReasoningContent::Encrypted(encrypted.to_string()));
    }
    Some(Reasoning {
        id: item
            .get("id")
            .and_then(Value::as_str)
            .map(ReasoningId::from),
        content,
    })
}

pub(super) fn parse_openai_message_id(item: &Value) -> Option<MessageId> {
    if item.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    item.get("id").and_then(Value::as_str).map(MessageId::from)
}

pub(super) fn parse_openai_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        Some(value) => value.clone(),
        None => json!({}),
    }
}
