use super::*;
use crate::preview::{PreviewCollapse, collapse_preview_text};
use crate::tool_render::{
    ToolDetail, ToolDetailLabel, ToolReviewItemKind, compact_successful_exploration_details,
    tool_argument_details, tool_arguments_preview_lines, tool_completion_state,
    tool_output_details, tool_review, tool_review_from_details,
};
use agent::tools::{
    SessionReviewItem, SessionReviewItemKind, SessionReviewResult, SessionReviewScope,
};
use agent::types::{SessionEventEnvelope, SessionEventKind};

const REVIEW_START_TOOL_NAME: &str = "review_start";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewBoundary {
    start_index: usize,
    description: Option<String>,
}

impl CodeAgentSession {
    pub(crate) async fn session_review(
        &self,
        scope: SessionReviewScope,
    ) -> agent::tools::Result<SessionReviewResult> {
        let session_id = {
            let runtime = self.runtime.lock().await;
            runtime.session_id()
        };
        let events = self
            .store
            .events(&session_id)
            .await
            .map_err(|error| agent::tools::ToolError::invalid_state(error.to_string()))?;
        build_session_review(&events, scope)
    }
}

fn build_session_review(
    events: &[SessionEventEnvelope],
    scope: SessionReviewScope,
) -> agent::tools::Result<SessionReviewResult> {
    let boundary = resolve_review_boundary(events, scope)?;
    let mut items = Vec::new();
    let mut tool_call_count = 0usize;

    for event in events.iter().skip(boundary.start_index) {
        match &event.event {
            SessionEventKind::ToolCallCompleted { call, output } => {
                if call.tool_name.as_str() == REVIEW_START_TOOL_NAME {
                    continue;
                }
                items.extend(review_items_from_completed_call(call, output));
                tool_call_count += 1;
            }
            SessionEventKind::ToolCallFailed { call, error } => {
                if call.tool_name.as_str() == REVIEW_START_TOOL_NAME {
                    continue;
                }
                items.extend(review_items_from_failed_call(call, error));
                tool_call_count += 1;
            }
            _ => {}
        }
    }

    Ok(SessionReviewResult {
        scope,
        summary: render_review_summary(
            scope,
            boundary.description.as_deref(),
            tool_call_count,
            items.len(),
        ),
        tool_call_count,
        boundary: boundary.description,
        items,
    })
}

fn resolve_review_boundary(
    events: &[SessionEventEnvelope],
    scope: SessionReviewScope,
) -> agent::tools::Result<ReviewBoundary> {
    match scope {
        SessionReviewScope::LatestTurn => {
            let start_index = events
                .iter()
                .rposition(|event| matches!(event.event, SessionEventKind::UserPromptSubmit { .. }))
                .map_or(0, |index| index + 1);
            Ok(ReviewBoundary {
                start_index,
                description: Some("latest user prompt".to_string()),
            })
        }
        SessionReviewScope::SinceCheckpoint => events
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, event)| match &event.event {
                SessionEventKind::CheckpointCreated { checkpoint } => Some(ReviewBoundary {
                    start_index: index + 1,
                    description: Some(format!(
                        "{} ({})",
                        checkpoint.checkpoint_id, checkpoint.summary
                    )),
                }),
                _ => None,
            })
            .ok_or_else(|| {
                agent::tools::ToolError::invalid_state(
                    "review_start with scope=since_checkpoint requires at least one checkpoint in the current session",
                )
            }),
    }
}

fn review_items_from_completed_call(
    call: &agent::types::ToolCall,
    output: &agent::types::ToolResult,
) -> Vec<SessionReviewItem> {
    let argument_preview = tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
    let mut detail_lines = tool_argument_details(&argument_preview);
    let completion =
        tool_completion_state(call.tool_name.as_str(), output.structured_content.as_ref());
    detail_lines.extend(tool_output_details(
        call.tool_name.as_str(),
        &output.text_content(),
        output.structured_content.as_ref(),
    ));
    compact_successful_exploration_details(&mut detail_lines, completion);
    let review = tool_review(call.tool_name.as_str(), output.structured_content.as_ref())
        .or_else(|| tool_review_from_details(&detail_lines));
    review
        .map(|review| {
            review
                .items
                .into_iter()
                .map(|item| SessionReviewItem {
                    title: format!("{} · {}", call.tool_name, item.title),
                    kind: map_review_item_kind(item.preview_kind),
                    preview_lines: item.preview_lines,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn review_items_from_failed_call(
    call: &agent::types::ToolCall,
    error: &str,
) -> Vec<SessionReviewItem> {
    let argument_preview = tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
    let mut items = tool_review_from_details(&tool_argument_details(&argument_preview))
        .map(|review| {
            review
                .items
                .into_iter()
                .map(|item| SessionReviewItem {
                    title: format!("{} · {}", call.tool_name, item.title),
                    kind: map_review_item_kind(item.preview_kind),
                    preview_lines: item.preview_lines,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let detail_lines = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: error.trim().to_string(),
    }];
    if let Some(review) = tool_review_from_details(&detail_lines) {
        items.extend(review.items.into_iter().map(|item| SessionReviewItem {
            title: format!("{} · {}", call.tool_name, item.title),
            kind: SessionReviewItemKind::Stderr,
            preview_lines: collapse_review_lines(&item.preview_lines),
        }));
    } else {
        items.push(SessionReviewItem {
            title: format!("{} · Result", call.tool_name),
            kind: SessionReviewItemKind::Stderr,
            preview_lines: collapse_review_lines(&[error.trim().to_string()]),
        });
    }
    items
}

fn map_review_item_kind(kind: ToolReviewItemKind) -> SessionReviewItemKind {
    match kind {
        ToolReviewItemKind::Neutral => SessionReviewItemKind::Neutral,
        ToolReviewItemKind::Command => SessionReviewItemKind::Command,
        ToolReviewItemKind::Stdout => SessionReviewItemKind::Stdout,
        ToolReviewItemKind::Stderr => SessionReviewItemKind::Stderr,
        ToolReviewItemKind::Diff => SessionReviewItemKind::Diff,
    }
}

fn collapse_review_lines(lines: &[String]) -> Vec<String> {
    let body = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if body.is_empty() {
        return Vec::new();
    }
    collapse_preview_text(&body.join("\n"), 48, 120, PreviewCollapse::HeadTail)
}

fn render_review_summary(
    scope: SessionReviewScope,
    boundary: Option<&str>,
    tool_call_count: usize,
    item_count: usize,
) -> String {
    let scope_text = match scope {
        SessionReviewScope::LatestTurn => "latest turn",
        SessionReviewScope::SinceCheckpoint => "checkpoint boundary",
    };
    match (
        tool_call_count,
        item_count,
        boundary.filter(|value| !value.trim().is_empty()),
    ) {
        (0, _, Some(boundary)) => {
            format!("No completed tool activity found since {boundary}.")
        }
        (0, _, None) => {
            format!("No completed tool activity found in the {scope_text}.")
        }
        (_, _, Some(boundary)) => format!(
            "Reviewed {tool_call_count} completed tool call(s) and surfaced {item_count} section(s) since {boundary}."
        ),
        _ => format!(
            "Reviewed {tool_call_count} completed tool call(s) and surfaced {item_count} section(s) from the {scope_text}."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_session_review, resolve_review_boundary};
    use agent::tools::{SessionReviewItemKind, SessionReviewScope};
    use agent::types::{
        AgentSessionId, CallId, CheckpointId, CheckpointOrigin, CheckpointRecord, CheckpointScope,
        EventId, SessionEventEnvelope, SessionEventKind, SessionId, SubmittedPromptSnapshot,
        ToolCall, ToolCallId, ToolName, ToolOrigin, ToolResult,
    };
    use serde_json::json;

    fn envelope(event: SessionEventKind, index: u64) -> SessionEventEnvelope {
        SessionEventEnvelope {
            id: EventId::new(),
            timestamp_ms: index as u128,
            session_id: SessionId::from("session_1"),
            agent_session_id: AgentSessionId::from("agent_1"),
            turn_id: None,
            tool_call_id: None,
            event,
        }
    }

    fn prompt(index: u64, text: &str) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::UserPromptSubmit {
                prompt: SubmittedPromptSnapshot {
                    text: text.to_string(),
                    attachments: Vec::new(),
                },
            },
            index,
        )
    }

    fn checkpoint(index: u64, summary: &str) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::CheckpointCreated {
                checkpoint: CheckpointRecord {
                    checkpoint_id: CheckpointId::from("checkpoint_1"),
                    session_id: SessionId::from("session_1"),
                    agent_session_id: AgentSessionId::from("agent_1"),
                    scope: CheckpointScope::Both,
                    origin: CheckpointOrigin::FileTool {
                        tool_name: ToolName::from("write"),
                    },
                    summary: summary.to_string(),
                    created_at_unix_s: index,
                    rollback_message_id: None,
                    prompt_message_id: None,
                    changed_files: Vec::new(),
                },
            },
            index,
        )
    }

    fn completed(index: u64, tool_name: &str, output: ToolResult) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::ToolCallCompleted {
                call: ToolCall {
                    id: ToolCallId::from(format!("tool_{index}")),
                    call_id: CallId::from(format!("call_{index}")),
                    tool_name: ToolName::from(tool_name),
                    arguments: json!({"cmd": "cargo test"}),
                    origin: ToolOrigin::Local,
                },
                output,
            },
            index,
        )
    }

    fn failed(index: u64, tool_name: &str, error: &str) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::ToolCallFailed {
                call: ToolCall {
                    id: ToolCallId::from(format!("tool_fail_{index}")),
                    call_id: CallId::from(format!("call_fail_{index}")),
                    tool_name: ToolName::from(tool_name),
                    arguments: json!({"path": "src/lib.rs"}),
                    origin: ToolOrigin::Local,
                },
                error: error.to_string(),
            },
            index,
        )
    }

    #[test]
    fn latest_turn_boundary_uses_last_prompt() {
        let events = vec![
            prompt(1, "first"),
            completed(
                2,
                "exec_command",
                ToolResult::text(ToolCallId::from("result_1"), "exec_command", "ok"),
            ),
            prompt(3, "second"),
        ];
        let boundary = resolve_review_boundary(&events, SessionReviewScope::LatestTurn)
            .expect("boundary should resolve");
        assert_eq!(boundary.start_index, 3);
    }

    #[test]
    fn since_checkpoint_requires_boundary() {
        let events = vec![prompt(1, "first")];
        let error = resolve_review_boundary(&events, SessionReviewScope::SinceCheckpoint)
            .expect_err("boundary should be required");
        assert!(
            error
                .to_string()
                .contains("requires at least one checkpoint")
        );
    }

    #[test]
    fn session_review_collects_terminal_tool_activity_after_boundary() {
        let file_output = ToolResult::text(ToolCallId::from("result_write"), "write", "updated")
            .with_structured_content(json!({
                "summary": "Updated src/lib.rs",
                "file_diffs": [
                    {
                        "path": "src/lib.rs",
                        "preview": "@@ -1 +1 @@\n-old\n+new"
                    }
                ]
            }));
        let events = vec![
            prompt(1, "first"),
            completed(
                2,
                "exec_command",
                ToolResult::text(ToolCallId::from("result_old"), "exec_command", "old"),
            ),
            checkpoint(3, "before write"),
            completed(4, "write", file_output),
            failed(5, "exec_command", "boom"),
        ];

        let review = build_session_review(&events, SessionReviewScope::SinceCheckpoint)
            .expect("review should succeed");

        assert_eq!(review.tool_call_count, 2);
        assert_eq!(
            review.boundary.as_deref(),
            Some("checkpoint_1 (before write)")
        );
        assert!(
            review
                .items
                .iter()
                .any(|item| item.kind == SessionReviewItemKind::Diff)
        );
        assert!(
            review
                .items
                .iter()
                .any(|item| item.title == "exec_command · Result")
        );
    }
}
