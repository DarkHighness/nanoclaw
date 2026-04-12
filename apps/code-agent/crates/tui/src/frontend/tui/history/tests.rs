use super::{
    format_agent_session_summary_collection, format_live_task_wait_outcome,
    format_session_event_line, format_session_export_result, format_session_operation_outcome,
    summaries::{
        format_agent_session_summary_line, format_session_search_line, format_session_summary_line,
    },
};
use crate::frontend::tui::state::{InspectorAction, InspectorEntry};
use crate::ui::{
    LiveTaskSummary, LiveTaskWaitOutcome, PersistedAgentSessionSummary,
    PersistedSessionSearchMatch, PersistedSessionSummary, ResumeSupport, SessionExportArtifact,
    SessionExportKind, SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot,
};
use agent::types::{
    AgentSessionId, AgentStatus, Message, SessionEventEnvelope, SessionEventKind, SessionId,
    ToolCall, ToolCallId, ToolOrigin, ToolResult,
};
use serde_json::json;
use std::path::PathBuf;

#[test]
fn export_result_includes_kind_path_and_item_count() {
    let lines = format_session_export_result(&SessionExportArtifact {
        kind: SessionExportKind::TranscriptText,
        session_id: SessionId::from("session-1"),
        output_path: PathBuf::from("/workspace/out.txt"),
        item_count: 4,
    });
    let lines = inspector_line_texts(&lines);

    assert!(lines.iter().any(|line| line == "export: transcript text"));
    assert!(lines.iter().any(|line| line == "path: /workspace/out.txt"));
    assert!(lines.iter().any(|line| line == "items: 4"));
}

#[test]
fn session_operation_outcome_uses_shell_style_summary() {
    let lines = format_session_operation_outcome(&SessionOperationOutcome {
        action: SessionOperationAction::Reattached,
        session_ref: "session-1".to_string(),
        active_agent_session_ref: "agent-session-2".to_string(),
        requested_agent_session_ref: Some("agent-session-1".to_string()),
        startup: SessionStartupSnapshot::default(),
        transcript: Vec::new(),
    });
    let lines = inspector_line_texts(&lines);

    assert_eq!(lines[0], "✔ Reattached session");
    assert_eq!(lines[1], "  └ session session-1");
    assert_eq!(lines[2], "  └ agent session agent-session-2");
    assert_eq!(lines[3], "  └ requested agent-session-1");
}

#[test]
fn session_summary_uses_two_line_shell_layout() {
    let line = format_session_summary_line(&PersistedSessionSummary {
        session_ref: "session_12345678".to_string(),
        first_timestamp_ms: 1,
        last_timestamp_ms: 2,
        event_count: 40,
        worker_session_count: 2,
        transcript_message_count: 12,
        session_title: None,
        last_user_prompt: Some("Refine the approval preview".to_string()),
        token_usage: None,
        resume_support: ResumeSupport::AttachedToActiveRuntime,
    });

    assert_eq!(
        line.serialized(),
        "• session_  Refine the approval preview\n  └ 12 messages · 40 events · 2 agent sessions · resume attached"
    );
}

#[test]
fn agent_session_summary_is_kept_to_two_lines() {
    let line = format_agent_session_summary_line(&PersistedAgentSessionSummary {
        agent_session_ref: "agent_session_123456".to_string(),
        session_ref: "session_123456".to_string(),
        label: "planner".to_string(),
        event_count: 14,
        transcript_message_count: 6,
        first_timestamp_ms: 1,
        last_timestamp_ms: 2,
        session_title: None,
        last_user_prompt: Some("Investigate flaky tests".to_string()),
        resume_support: ResumeSupport::AttachedToActiveRuntime,
    });

    assert_eq!(
        line.serialized(),
        "• agent_se  planner\n  └ session session_ · 6 messages · 14 events · resume attached · prompt Investigate flaky tests"
    );
}

#[test]
fn agent_session_collection_exposes_resume_as_alternate_action() {
    let entry = format_agent_session_summary_collection(&PersistedAgentSessionSummary {
        agent_session_ref: "agent_session_123456".to_string(),
        session_ref: "session_123456".to_string(),
        label: "planner".to_string(),
        event_count: 14,
        transcript_message_count: 6,
        first_timestamp_ms: 1,
        last_timestamp_ms: 2,
        session_title: None,
        last_user_prompt: Some("Investigate flaky tests".to_string()),
        resume_support: ResumeSupport::Reattachable,
    });

    match entry {
        InspectorEntry::CollectionItem {
            action,
            alternate_action,
            ..
        } => {
            assert_eq!(
                action,
                Some(InspectorAction::RunCommand(
                    "/agent_session agent_session_123456".to_string()
                ))
            );
            let alternate_action = alternate_action.expect("alternate resume action");
            assert_eq!(alternate_action.key_hint, "r");
            assert_eq!(alternate_action.label, "resume");
            assert_eq!(
                alternate_action.action,
                InspectorAction::RunCommand("/resume agent_session_123456".to_string())
            );
        }
        _ => panic!("expected collection item"),
    }
}

#[test]
fn session_search_summary_stays_compact() {
    let line = format_session_search_line(&PersistedSessionSearchMatch {
        summary: PersistedSessionSummary {
            session_ref: "session_12345678".to_string(),
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            event_count: 40,
            worker_session_count: 2,
            transcript_message_count: 12,
            session_title: None,
            last_user_prompt: Some("Refine the approval preview".to_string()),
            token_usage: None,
            resume_support: ResumeSupport::AttachedToActiveRuntime,
        },
        matched_event_count: 3,
        preview_matches: vec![
            "exec_command approval".to_string(),
            "cargo test".to_string(),
        ],
    });

    assert_eq!(
        line.serialized(),
        "• session_  Refine the approval preview\n  └ 12 messages · 40 events · 2 agent sessions · resume attached · matched 3 event(s) · preview exec_command approval | cargo test"
    );
}

#[test]
fn session_summary_prefers_session_note_title_over_prompt() {
    let line = format_session_summary_line(&PersistedSessionSummary {
        session_ref: "session_12345678".to_string(),
        first_timestamp_ms: 1,
        last_timestamp_ms: 2,
        event_count: 40,
        worker_session_count: 2,
        transcript_message_count: 12,
        session_title: Some("Deploy rollback follow-up".to_string()),
        last_user_prompt: Some("Refine the approval preview".to_string()),
        token_usage: None,
        resume_support: ResumeSupport::AttachedToActiveRuntime,
    });

    assert_eq!(
        line.serialized(),
        "• session_  Deploy rollback follow-up\n  └ 12 messages · 40 events · 2 agent sessions · resume attached"
    );
}

#[test]
fn agent_session_summary_surfaces_parent_session_title() {
    let line = format_agent_session_summary_line(&PersistedAgentSessionSummary {
        agent_session_ref: "agent_session_123456".to_string(),
        session_ref: "session_123456".to_string(),
        label: "planner".to_string(),
        event_count: 14,
        transcript_message_count: 6,
        first_timestamp_ms: 1,
        last_timestamp_ms: 2,
        session_title: Some("Deploy rollback follow-up".to_string()),
        last_user_prompt: Some("Investigate flaky tests".to_string()),
        resume_support: ResumeSupport::AttachedToActiveRuntime,
    });

    assert_eq!(
        line.serialized(),
        "• agent_se  planner\n  └ session session_ · 6 messages · 14 events · resume attached · title Deploy rollback follow-up"
    );
}

#[test]
fn live_task_wait_outcome_uses_terminal_status_marker() {
    let lines = format_live_task_wait_outcome(&LiveTaskWaitOutcome {
        requested_ref: "task_1".to_string(),
        agent_id: "agent_1".to_string(),
        task_id: "task_1".to_string(),
        status: AgentStatus::Completed,
        summary: "Updated planner and wrote tests".to_string(),
        claimed_files: vec!["src/lib.rs".to_string()],
        remaining_live_tasks: vec![LiveTaskSummary {
            agent_id: "agent_2".to_string(),
            task_id: "task_2".to_string(),
            role: "reviewer".to_string(),
            status: AgentStatus::Running,
            session_ref: "session_2".to_string(),
            agent_session_ref: "agent-session-2".to_string(),
        }],
    });
    let lines = inspector_line_texts(&lines);

    assert_eq!(lines[0], "• Finished waiting for task task_1");
    assert_eq!(lines[1], "  └ requested task_1");
    assert_eq!(lines[4], "  └ summary Updated planner and wrote tests");
    assert_eq!(lines[5], "  └ claimed files src/lib.rs");
    assert_eq!(lines[6], "  └ still running task_2 (reviewer, running)");
}

#[test]
fn transcript_event_reuses_shell_transcript_prefixes() {
    let event = SessionEventEnvelope::new(
        SessionId::from("session-1"),
        AgentSessionId::from("agent-session-1"),
        None,
        None,
        SessionEventKind::TranscriptMessage {
            message: Message::user("Explain the failing test"),
        },
    );

    assert_eq!(
        format_session_event_line(&event).serialized(),
        "› Explain the failing test"
    );
}

#[test]
fn interrupt_input_event_mentions_restart_semantics() {
    let event = SessionEventEnvelope::new(
        SessionId::from("session-1"),
        AgentSessionId::from("agent-session-1"),
        None,
        None,
        SessionEventKind::AgentEnvelope {
            envelope: agent::types::AgentEnvelope::new(
                "agent-1".into(),
                None,
                "session-1".into(),
                "agent-session-1".into(),
                agent::types::AgentEnvelopeKind::Input {
                    message: Message::user("focus the latest diff"),
                    delivery: agent::types::AgentInputDelivery::Interrupt,
                },
            ),
        },
    );

    assert_eq!(
        format_session_event_line(&event).serialized(),
        "• Agent interrupt restarted with new input\n  └ content user> focus the latest diff"
    );
}

#[test]
fn tool_approval_event_uses_shell_summary_layout() {
    let call = ToolCall {
        id: ToolCallId::from("tool-call-1"),
        call_id: ToolCallId::from("tool-call-1").into(),
        tool_name: "exec_command".into(),
        arguments: json!({"cmd": "cargo test"}),
        origin: ToolOrigin::Local,
    };
    let event = SessionEventEnvelope::new(
        SessionId::from("session-1"),
        AgentSessionId::from("agent-session-1"),
        None,
        None,
        SessionEventKind::ToolApprovalRequested {
            call,
            reasons: vec!["sandbox policy requires approval".to_string()],
        },
    );

    assert_eq!(
        format_session_event_line(&event).serialized(),
        "• Awaiting approval to run cargo test\n  └ origin local\n  └ $ cargo test\n  └ reason sandbox policy requires approval"
    );
}

#[test]
fn tool_completion_event_includes_shell_summary_details() {
    let call = ToolCall {
        id: ToolCallId::from("tool-call-1"),
        call_id: ToolCallId::from("tool-call-1").into(),
        tool_name: "exec_command".into(),
        arguments: json!({"cmd": "cargo test"}),
        origin: ToolOrigin::Local,
    };
    let output = ToolResult::text(
        ToolCallId::from("tool-call-1"),
        "exec_command",
        "tests passed",
    );
    let event = SessionEventEnvelope::new(
        SessionId::from("session-1"),
        AgentSessionId::from("agent-session-1"),
        None,
        None,
        SessionEventKind::ToolCallCompleted { call, output },
    );

    assert_eq!(
        format_session_event_line(&event).serialized(),
        "• Ran cargo test\n  └ $ cargo test\n  └ output tests passed"
    );
}

#[test]
fn file_tool_completion_event_includes_diff_block() {
    let call = ToolCall {
        id: ToolCallId::from("tool-call-2"),
        call_id: ToolCallId::from("tool-call-2").into(),
        tool_name: "write".into(),
        arguments: json!({"path": "src/lib.rs"}),
        origin: ToolOrigin::Local,
    };
    let output = ToolResult {
        id: ToolCallId::from("tool-call-2"),
        call_id: ToolCallId::from("tool-call-2").into(),
        tool_name: "write".into(),
        parts: vec![agent::types::MessagePart::text(
            "Wrote 18 bytes to src/lib.rs\n[diff_preview]\n--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()",
        )],
        attachments: Vec::new(),
        structured_content: Some(json!({
            "kind": "success",
            "summary": "Wrote 18 bytes to src/lib.rs",
            "snapshot_before": "snap_old",
            "snapshot_after": "snap_new",
            "file_diffs": [{
                "path": "src/lib.rs",
                "preview": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()"
            }]
        })),
        continuation: None,
        metadata: None,
        is_error: false,
    };
    let event = SessionEventEnvelope::new(
        SessionId::from("session-1"),
        AgentSessionId::from("agent-session-1"),
        None,
        None,
        SessionEventKind::ToolCallCompleted { call, output },
    );

    let rendered = format_session_event_line(&event).serialized();
    assert!(rendered.contains("• Updated files"));
    assert!(rendered.contains("  └ files src/lib.rs"));
    assert!(rendered.contains("action [r] review diff"));
}

fn inspector_line_texts(lines: &[InspectorEntry]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| match line {
            InspectorEntry::Section(text)
            | InspectorEntry::Plain(text)
            | InspectorEntry::Muted(text)
            | InspectorEntry::Command(text) => vec![text.clone()],
            InspectorEntry::Field { key, value } => vec![format!("{key}: {value}")],
            InspectorEntry::Transcript(entry) => entry
                .serialized()
                .lines()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
            InspectorEntry::CollectionItem {
                primary, secondary, ..
            } => vec![
                secondary
                    .as_ref()
                    .map(|secondary| format!("{primary}  {secondary}"))
                    .unwrap_or_else(|| primary.clone()),
            ],
            InspectorEntry::Empty => vec![String::new()],
        })
        .collect()
}
