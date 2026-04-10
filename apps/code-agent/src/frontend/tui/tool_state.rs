use super::state::{ExecutionEntry, PlanEntry, TranscriptEntry};
use agent::types::{SessionEventEnvelope, SessionEventKind};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestoredToolPanels {
    pub(crate) plan_items: Vec<PlanEntry>,
    pub(crate) execution: Option<ExecutionEntry>,
}

pub(crate) fn plan_items_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<Vec<PlanEntry>> {
    plan_payload_from_tool_output(tool_name, structured_output_preview).map(|(_, _, items)| items)
}

pub(crate) fn plan_update_entry_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<TranscriptEntry> {
    let (explanation, warnings, items) =
        plan_payload_from_tool_output(tool_name, structured_output_preview)?;
    Some(TranscriptEntry::plan_update(explanation, warnings, items))
}

pub(crate) fn execution_state_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<Option<ExecutionEntry>> {
    execution_payload_from_tool_output(tool_name, structured_output_preview).map(|(_, state)| state)
}

pub(crate) fn execution_update_entry_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<TranscriptEntry> {
    let (headline, state) =
        execution_payload_from_tool_output(tool_name, structured_output_preview)?;
    Some(TranscriptEntry::execution_update(headline, state))
}

pub(crate) fn restore_tool_panels(events: &[SessionEventEnvelope]) -> RestoredToolPanels {
    let mut restored = RestoredToolPanels::default();
    for event in events {
        let SessionEventKind::ToolCallCompleted { call, output } = &event.event else {
            continue;
        };
        let structured = output
            .structured_content
            .as_ref()
            .and_then(|value| serde_json::to_string(value).ok());
        if let Some(plan_items) =
            plan_items_from_tool_output(call.tool_name.as_str(), structured.as_deref())
        {
            restored.plan_items = plan_items;
        }
        if let Some(execution) =
            execution_state_from_tool_output(call.tool_name.as_str(), structured.as_deref())
        {
            restored.execution = execution;
        }
    }
    restored
}

fn plan_payload_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<(Option<String>, Vec<String>, Vec<PlanEntry>)> {
    if tool_name != "update_plan" {
        return None;
    }
    let value = serde_json::from_str::<Value>(structured_output_preview?).ok()?;
    if value.get("kind")?.as_str()? != "success" {
        return None;
    }
    let items = value.get("items")?.as_array()?;
    let explanation = value
        .get("explanation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let warnings = value
        .get("warnings")
        .and_then(Value::as_array)
        .map(|warnings| {
            warnings
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some((
        explanation,
        warnings,
        items
            .iter()
            .filter_map(|item| {
                let step = item.get("step")?.as_str()?.to_string();
                Some(PlanEntry {
                    id: step.clone(),
                    content: step,
                    status: item.get("status")?.as_str()?.to_string(),
                })
            })
            .collect(),
    ))
}

fn execution_payload_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<(String, Option<ExecutionEntry>)> {
    if tool_name != "update_execution" {
        return None;
    }
    let value = serde_json::from_str::<Value>(structured_output_preview?).ok()?;
    if value.get("kind")?.as_str()? != "success" {
        return None;
    }
    let action = value.get("action")?.as_str()?;
    let headline = match action {
        "get" => "Execution Snapshot",
        "clear" => "Cleared Execution",
        _ => "Updated Execution",
    }
    .to_string();
    let scope_label = value.pointer("/scope/label")?.as_str()?.to_string();
    let state = value.get("state").and_then(parse_execution_state_payload);
    let state = state.map(|mut entry| {
        entry.scope_label = scope_label;
        entry
    });
    Some((headline, state))
}

fn parse_execution_state_payload(value: &Value) -> Option<ExecutionEntry> {
    value.as_object()?;
    Some(ExecutionEntry {
        scope_label: String::new(),
        status: value.get("status")?.as_str()?.to_string(),
        summary: value.get("summary")?.as_str()?.to_string(),
        next_action: value
            .get("next_action")
            .and_then(Value::as_str)
            .map(str::to_string),
        verification: value
            .get("verification")
            .and_then(Value::as_str)
            .map(str::to_string),
        blocker: value
            .get("blocker")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::{execution_state_from_tool_output, restore_tool_panels};
    use agent::types::{
        AgentSessionId, CallId, MessagePart, SessionEventEnvelope, SessionEventKind, SessionId,
        ToolCall, ToolCallId, ToolOrigin, ToolResult,
    };
    use serde_json::json;

    #[test]
    fn update_execution_payload_supports_clear_and_set() {
        let set = execution_state_from_tool_output(
            "update_execution",
            Some(
                &json!({
                    "kind": "success",
                    "action": "set",
                    "scope": {"label": "session root"},
                    "state": {
                        "status": "active",
                        "summary": "Patch the observer",
                        "next_action": "Wire the side rail"
                    }
                })
                .to_string(),
            ),
        )
        .expect("set payload");
        assert_eq!(set.as_ref().unwrap().scope_label, "session root");
        assert_eq!(set.as_ref().unwrap().status, "active");

        let clear = execution_state_from_tool_output(
            "update_execution",
            Some(
                &json!({
                    "kind": "success",
                    "action": "clear",
                    "scope": {"label": "session root"},
                    "state": null
                })
                .to_string(),
            ),
        )
        .expect("clear payload");
        assert!(clear.is_none());
    }

    #[test]
    fn restore_tool_panels_replays_latest_snapshots() {
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-1");
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::ToolCallCompleted {
                    call: ToolCall {
                        id: ToolCallId::new(),
                        call_id: CallId::new(),
                        tool_name: "update_plan".into(),
                        arguments: json!({}),
                        origin: ToolOrigin::Local,
                    },
                    output: ToolResult {
                        id: ToolCallId::new(),
                        call_id: CallId::new(),
                        tool_name: "update_plan".into(),
                        parts: vec![MessagePart::text("ok")],
                        attachments: Vec::new(),
                        structured_content: Some(json!({
                            "kind": "success",
                            "items": [{"step": "Inspect", "status": "completed"}]
                        })),
                        continuation: None,
                        metadata: None,
                        is_error: false,
                    },
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                None,
                None,
                SessionEventKind::ToolCallCompleted {
                    call: ToolCall {
                        id: ToolCallId::new(),
                        call_id: CallId::new(),
                        tool_name: "update_execution".into(),
                        arguments: json!({}),
                        origin: ToolOrigin::Local,
                    },
                    output: ToolResult {
                        id: ToolCallId::new(),
                        call_id: CallId::new(),
                        tool_name: "update_execution".into(),
                        parts: vec![MessagePart::text("ok")],
                        attachments: Vec::new(),
                        structured_content: Some(json!({
                            "kind": "success",
                            "action": "set",
                            "scope": {"label": "session root"},
                            "state": {
                                "status": "verifying",
                                "summary": "Run focused tests",
                                "verification": "cargo test"
                            }
                        })),
                        continuation: None,
                        metadata: None,
                        is_error: false,
                    },
                },
            ),
        ];

        let restored = restore_tool_panels(&events);
        assert_eq!(restored.plan_items.len(), 1);
        assert_eq!(restored.execution.as_ref().unwrap().status, "verifying");
    }
}
