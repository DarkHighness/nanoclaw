use super::state::{
    PlanEntry, PlanEntryStatus, PlanFocusEntry, PlanFocusStatus, TranscriptEntry,
    TranscriptPlanFocusChange,
};
use agent::types::{SessionEventEnvelope, SessionEventKind};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RestoredToolPanels {
    pub(crate) plan_items: Vec<PlanEntry>,
    pub(crate) focus: Option<PlanFocusEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedPlanToolOutput {
    explanation: Option<String>,
    warnings: Vec<String>,
    plan_changed: bool,
    focus_change: TranscriptPlanFocusChange,
    items: Vec<PlanEntry>,
    focus: Option<PlanFocusEntry>,
}

pub(crate) fn plan_items_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<Vec<PlanEntry>> {
    plan_payload_from_tool_output(tool_name, structured_output_preview).map(|parsed| parsed.items)
}

pub(crate) fn focus_state_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<Option<PlanFocusEntry>> {
    plan_payload_from_tool_output(tool_name, structured_output_preview).map(|parsed| parsed.focus)
}

pub(crate) fn plan_update_entry_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<TranscriptEntry> {
    let parsed = plan_payload_from_tool_output(tool_name, structured_output_preview)?;
    Some(TranscriptEntry::plan_update(
        parsed.plan_changed,
        parsed.focus_change,
        parsed.explanation,
        parsed.warnings,
        parsed.items,
        parsed.focus,
    ))
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
        if let Some(parsed) =
            plan_payload_from_tool_output(call.tool_name.as_str(), structured.as_deref())
        {
            restored.plan_items = parsed.items;
            restored.focus = parsed.focus;
        }
    }
    restored
}

fn plan_payload_from_tool_output(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<ParsedPlanToolOutput> {
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
    let items = items
        .iter()
        .filter_map(|item| {
            let step = item.get("step")?.as_str()?.to_string();
            Some(PlanEntry {
                id: step.clone(),
                content: step,
                status: PlanEntryStatus::from_wire(item.get("status")?.as_str()?),
            })
        })
        .collect::<Vec<_>>();
    let focus = value.get("focus").and_then(parse_focus_payload);
    let focus_change = if value
        .get("focus_updated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if focus.is_some() {
            TranscriptPlanFocusChange::Updated
        } else {
            TranscriptPlanFocusChange::Cleared
        }
    } else {
        TranscriptPlanFocusChange::Unchanged
    };
    Some(ParsedPlanToolOutput {
        explanation,
        warnings,
        plan_changed: value
            .get("plan_updated")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        focus_change,
        items,
        focus,
    })
}

fn parse_focus_payload(value: &Value) -> Option<PlanFocusEntry> {
    value.as_object()?;
    Some(PlanFocusEntry {
        scope_label: value.get("scope_label")?.as_str()?.to_string(),
        status: PlanFocusStatus::from_wire(value.get("status")?.as_str()?),
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
    use super::{PlanFocusStatus, focus_state_from_tool_output, restore_tool_panels};
    use agent::types::{
        AgentSessionId, CallId, MessagePart, SessionEventEnvelope, SessionEventKind, SessionId,
        ToolCall, ToolCallId, ToolOrigin, ToolResult,
    };
    use serde_json::json;

    #[test]
    fn update_plan_payload_supports_focus_updates() {
        let focus = focus_state_from_tool_output(
            "update_plan",
            Some(
                &json!({
                    "kind": "success",
                    "plan_updated": false,
                    "focus_updated": true,
                    "items": [],
                    "focus": {
                        "scope_label": "session root",
                        "status": "active",
                        "summary": "Patch the observer",
                        "next_action": "Wire the side rail"
                    }
                })
                .to_string(),
            ),
        )
        .expect("focus payload");
        assert_eq!(focus.as_ref().unwrap().scope_label, "session root");
        assert_eq!(focus.as_ref().unwrap().status, PlanFocusStatus::Active);

        let cleared = focus_state_from_tool_output(
            "update_plan",
            Some(
                &json!({
                    "kind": "success",
                    "plan_updated": false,
                    "focus_updated": true,
                    "items": [],
                    "focus": null
                })
                .to_string(),
            ),
        )
        .expect("clear payload");
        assert!(cleared.is_none());
    }

    #[test]
    fn restore_tool_panels_replays_latest_snapshots() {
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-1");
        let events = vec![SessionEventEnvelope::new(
            session_id,
            agent_session_id,
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
                        "plan_updated": true,
                        "focus_updated": true,
                        "items": [{"step": "Inspect", "status": "completed"}],
                        "focus": {
                            "scope_label": "session root",
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
        )];

        let restored = restore_tool_panels(&events);
        assert_eq!(restored.plan_items.len(), 1);
        assert_eq!(
            restored.focus.as_ref().unwrap().status,
            PlanFocusStatus::Verifying
        );
    }
}
