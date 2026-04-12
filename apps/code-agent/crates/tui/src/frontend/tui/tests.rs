use super::build_history_rollback_candidates;
use super::build_startup_inspector;
use super::commands::command_palette_lines;
use super::state::{
    ComposerAttachmentEditSummary, ComposerDraftAttachmentKind, ComposerRowAttachmentPreview,
    InspectorEntry, SessionSummary,
};
use super::{
    PlainInputSubmitAction, attachment_preview_status_label,
    external_editor_attachment_status_suffix, live_task_wait_toast_message,
    looks_like_local_image_path, merge_interrupt_steers, plain_input_submit_action,
};
use crate::interaction::SessionPermissionMode;
use crate::ui::{HistoryRollbackRound, LiveTaskSummary, LiveTaskWaitOutcome};
use agent::types::{Message, MessageId, MessagePart, MessageRole, TaskId, TaskOrigin, TaskStatus};
use crossterm::event::KeyCode;
use std::path::{Path, PathBuf};

#[test]
fn startup_inspector_surfaces_backend_boot_snapshot() {
    let lines = build_startup_inspector(&SessionSummary {
        workspace_name: "nanoclaw".to_string(),
        active_session_ref: "session_123".to_string(),
        root_agent_session_id: "session_123".to_string(),
        provider_label: "openai".to_string(),
        model: "gpt-5.4".to_string(),
        model_reasoning_effort: Some("high".to_string()),
        supported_model_reasoning_efforts: vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ],
        supports_image_input: true,
        workspace_root: PathBuf::from("/workspace"),
        git: Default::default(),
        tool_names: vec!["read".to_string(), "write".to_string()],
        skills: Vec::new(),
        store_label: "file /workspace/.nanoclaw/store".to_string(),
        store_warning: Some("falling back soon".to_string()),
        stored_session_count: 12,
        default_sandbox_summary: "workspace-write".to_string(),
        sandbox_summary: "enforced via seatbelt".to_string(),
        permission_mode: SessionPermissionMode::Default,
        host_process_surfaces_allowed: true,
        startup_diagnostics: Default::default(),
        queued_commands: 0,
        token_ledger: Default::default(),
        statusline: Default::default(),
    });
    let lines = inspector_line_texts(&lines);

    assert!(
        lines
            .iter()
            .any(|line| line == "store: file /workspace/.nanoclaw/store (12 sessions)")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "sandbox: enforced via seatbelt")
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("warning: falling back soon"))
    );
    assert!(lines.iter().any(|line| line == "image input: enabled"));
    assert!(
        lines
            .iter()
            .any(|line| line == "/statusline  choose footer items")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/thinking [level]  pick or set model effort")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/theme [name]  pick or set tui theme")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/details  toggle tool details")
    );
}

fn inspector_line_texts(lines: &[InspectorEntry]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line {
            InspectorEntry::Section(text) => format!("## {text}"),
            InspectorEntry::Plain(text)
            | InspectorEntry::Muted(text)
            | InspectorEntry::Command(text) => text.clone(),
            InspectorEntry::Field { key, value } => format!("{key}: {value}"),
            InspectorEntry::Transcript(entry) => entry.serialized(),
            InspectorEntry::CollectionItem {
                primary, secondary, ..
            } => secondary
                .as_ref()
                .map(|secondary| format!("{primary}  {secondary}"))
                .unwrap_or_else(|| primary.clone()),
            InspectorEntry::Empty => String::new(),
        })
        .collect()
}

#[test]
fn command_palette_groups_operator_commands() {
    let lines = inspector_line_texts(&command_palette_lines());

    assert!(lines.iter().any(|line| line == "## Session"));
    assert!(lines.iter().any(|line| line == "## Agents"));
    assert!(lines.iter().any(|line| line == "## History"));
    assert!(
        lines
            .iter()
            .any(|line| { line.starts_with("/live_tasks  list live child agents") })
    );
}

#[test]
fn running_enter_targets_active_turn_steer() {
    assert_eq!(
        plain_input_submit_action("tighten the plan", true, false, true, KeyCode::Enter),
        Some(PlainInputSubmitAction::SteerActiveTurn)
    );
}

#[test]
fn running_tab_queues_prompt() {
    assert_eq!(
        plain_input_submit_action("write a regression test", true, false, true, KeyCode::Tab),
        Some(PlainInputSubmitAction::QueuePrompt)
    );
}

#[test]
fn idle_enter_starts_prompt() {
    assert_eq!(
        plain_input_submit_action(
            "write a regression test",
            true,
            false,
            false,
            KeyCode::Enter
        ),
        Some(PlainInputSubmitAction::StartPrompt)
    );
}

#[test]
fn slash_input_keeps_command_flow() {
    assert_eq!(
        plain_input_submit_action("/help", true, false, true, KeyCode::Enter),
        None
    );
}

#[test]
fn attachment_only_idle_enter_starts_prompt() {
    assert_eq!(
        plain_input_submit_action("", true, true, false, KeyCode::Enter),
        Some(PlainInputSubmitAction::StartPrompt)
    );
}

#[test]
fn attachment_prompt_running_enter_queues_follow_up_instead_of_steer() {
    assert_eq!(
        plain_input_submit_action("review this", true, true, true, KeyCode::Enter),
        Some(PlainInputSubmitAction::QueuePrompt)
    );
}

#[test]
fn live_task_wait_toast_mentions_remaining_running_tasks() {
    let outcome = LiveTaskWaitOutcome {
        requested_ref: "task_123".to_string(),
        task_id: TaskId::from("task_123"),
        status: TaskStatus::Completed,
        summary: "done".to_string(),
        agent_id: "agent_123".to_string(),
        claimed_files: Vec::new(),
        remaining_live_tasks: vec![LiveTaskSummary {
            agent_id: "agent_456".to_string(),
            task_id: TaskId::from("task_456"),
            role: "reviewer".to_string(),
            origin: TaskOrigin::ChildAgentBacked,
            status: TaskStatus::Running,
            session_ref: "session_456".to_string(),
            agent_session_ref: "agent-session-456".to_string(),
            worktree_id: None,
            worktree_root: None,
        }],
    };

    assert_eq!(
        live_task_wait_toast_message(&outcome, true),
        "task task_123 completed · done · 1 still running · enter steer / tab queue / /task inspect"
    );
    assert_eq!(
        live_task_wait_toast_message(&outcome, false),
        "task task_123 completed · done · 1 still running · model follow-up queued / /task inspect"
    );
}

#[test]
fn local_image_path_detection_accepts_known_image_extensions() {
    assert!(looks_like_local_image_path(Path::new(
        "fixtures/failure.png"
    )));
    assert!(looks_like_local_image_path(Path::new(
        "fixtures/failure.JPEG"
    )));
    assert!(!looks_like_local_image_path(Path::new(
        "fixtures/failure.txt"
    )));
}

#[test]
fn interrupt_merge_keeps_steer_order() {
    assert_eq!(
        merge_interrupt_steers(vec![
            "first pending steer".to_string(),
            "second pending steer".to_string(),
        ]),
        Some("first pending steer\nsecond pending steer".to_string())
    );
}

#[test]
fn interrupt_merge_ignores_empty_steer_list() {
    assert_eq!(merge_interrupt_steers(Vec::new()), None);
}

#[test]
fn attachment_preview_status_label_formats_numbered_summary() {
    assert_eq!(
        attachment_preview_status_label(&ComposerRowAttachmentPreview {
            index: 2,
            summary: "file · run.pdf".to_string(),
            detail: "reports/run.pdf".to_string(),
        }),
        "attachment #2 · file · run.pdf"
    );
}

#[test]
fn external_editor_attachment_status_suffix_reports_detached_attachment() {
    assert_eq!(
        external_editor_attachment_status_suffix(&ComposerAttachmentEditSummary {
            detached: vec![ComposerRowAttachmentPreview {
                index: 1,
                summary: "image · failure.png".to_string(),
                detail: "https://example.com/assets/failure.png".to_string(),
            }],
            reordered: true,
        }),
        " · detached attachment #1 · image · failure.png and reordered remaining"
    );
}

#[test]
fn history_rollback_candidates_track_turn_slice_and_removed_counts() {
    let first_prompt = Message::user("first").with_message_id(MessageId::from("msg-1"));
    let first_answer = Message::assistant("answer one").with_message_id(MessageId::from("msg-2"));
    let second_prompt = Message::user("second").with_message_id(MessageId::from("msg-3"));
    let second_answer = Message::assistant("answer two").with_message_id(MessageId::from("msg-4"));
    let rounds = vec![
        HistoryRollbackRound {
            rollback_message_id: first_prompt.message_id.clone(),
            prompt_message: first_prompt.clone(),
            round_messages: vec![first_prompt.clone(), first_answer.clone()],
            removed_turn_count: 2,
            removed_message_count: 4,
        },
        HistoryRollbackRound {
            rollback_message_id: second_prompt.message_id.clone(),
            prompt_message: second_prompt.clone(),
            round_messages: vec![second_prompt.clone(), second_answer.clone()],
            removed_turn_count: 1,
            removed_message_count: 2,
        },
    ];

    let candidates = build_history_rollback_candidates(&rounds);

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].message_id, MessageId::from("msg-1"));
    assert_eq!(candidates[0].removed_turn_count, 2);
    assert_eq!(candidates[0].removed_message_count, 4);
    assert_eq!(
        candidates[0].turn_preview_lines,
        vec!["› first".into(), "• answer one".into()]
    );

    assert_eq!(candidates[1].message_id, MessageId::from("msg-3"));
    assert_eq!(candidates[1].removed_turn_count, 1);
    assert_eq!(candidates[1].removed_message_count, 2);
    assert_eq!(
        candidates[1].turn_preview_lines,
        vec!["› second".into(), "• answer two".into()]
    );
}

#[test]
fn history_rollback_candidates_restore_latest_user_prompt_from_request_round_snapshot() {
    let steer = Message::system("prefer terse answers").with_message_id(MessageId::from("msg-1"));
    let recall =
        Message::user("recalled workspace memory").with_message_id(MessageId::from("msg-2"));
    let prompt = Message::user("real user prompt").with_message_id(MessageId::from("msg-3"));
    let reply =
        Message::assistant("latest assistant reply").with_message_id(MessageId::from("msg-4"));
    let rounds = vec![HistoryRollbackRound {
        rollback_message_id: steer.message_id.clone(),
        prompt_message: prompt.clone(),
        round_messages: vec![steer, recall, prompt.clone(), reply],
        removed_turn_count: 1,
        removed_message_count: 4,
    }];

    let candidates = build_history_rollback_candidates(&rounds);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].message_id, MessageId::from("msg-1"));
    assert_eq!(candidates[0].prompt, "real user prompt");
    assert_eq!(candidates[0].draft.text, "real user prompt");
    assert_eq!(
        candidates[0].turn_preview_lines,
        vec![
            "• prefer terse answers".into(),
            "› recalled workspace memory".into(),
            "› real user prompt".into(),
            "• latest assistant reply".into(),
        ]
    );
}

#[test]
fn history_rollback_candidates_keep_operator_visible_attachment_summaries() {
    let prompt = Message::new(
        MessageRole::User,
        vec![MessagePart::ImageUrl {
            url: "https://example.com/diagram.png".to_string(),
            mime_type: Some("image/png".to_string()),
        }],
    )
    .with_message_id(MessageId::from("msg-1"));
    let rounds = vec![HistoryRollbackRound {
        rollback_message_id: prompt.message_id.clone(),
        prompt_message: prompt.clone(),
        round_messages: vec![prompt],
        removed_turn_count: 1,
        removed_message_count: 1,
    }];

    let candidates = build_history_rollback_candidates(&rounds);

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].prompt,
        "[image_url:https://example.com/diagram.png image/png]"
    );
    assert_eq!(candidates[0].draft.text, "");
    assert_eq!(candidates[0].draft.draft_attachments.len(), 1);
    assert!(matches!(
        &candidates[0].draft.draft_attachments[0].kind,
        ComposerDraftAttachmentKind::RemoteImage { requested_url, .. }
            if requested_url == "https://example.com/diagram.png"
    ));
}

#[test]
fn history_rollback_candidates_restore_text_and_inline_attachments_into_draft() {
    let prompt = Message::new(
        MessageRole::User,
        vec![
            MessagePart::ImageUrl {
                url: "https://example.com/diagram.png".to_string(),
                mime_type: Some("image/png".to_string()),
            },
            MessagePart::File {
                file_name: Some("run.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                data_base64: Some("cGRm".to_string()),
                uri: Some("reports/run.pdf".to_string()),
            },
            MessagePart::inline_text(" summarize the artifact"),
        ],
    )
    .with_message_id(MessageId::from("msg-1"));
    let rounds = vec![HistoryRollbackRound {
        rollback_message_id: prompt.message_id.clone(),
        prompt_message: prompt.clone(),
        round_messages: vec![prompt],
        removed_turn_count: 1,
        removed_message_count: 1,
    }];

    let candidates = build_history_rollback_candidates(&rounds);

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].prompt,
        "[image_url:https://example.com/diagram.png image/png]\n[file:run.pdf application/pdf reports/run.pdf]\n summarize the artifact"
    );
    assert_eq!(candidates[0].draft.text, "[File #1] summarize the artifact");
    assert_eq!(candidates[0].draft.draft_attachments.len(), 2);
    assert!(matches!(
        &candidates[0].draft.draft_attachments[0].kind,
        ComposerDraftAttachmentKind::RemoteImage { requested_url, .. }
            if requested_url == "https://example.com/diagram.png"
    ));
    assert!(matches!(
        &candidates[0].draft.draft_attachments[1].kind,
        ComposerDraftAttachmentKind::LocalFile { requested_path, .. }
            if requested_path == "reports/run.pdf"
    ));
}

#[test]
fn history_rollback_candidates_restore_large_paste_placeholders_into_draft() {
    let prompt = Message::new(
        MessageRole::User,
        vec![
            MessagePart::inline_text("before "),
            MessagePart::paste("[Paste #1]", "pasted body"),
            MessagePart::inline_text(" after"),
        ],
    )
    .with_message_id(MessageId::from("msg-1"));
    let rounds = vec![HistoryRollbackRound {
        rollback_message_id: prompt.message_id.clone(),
        prompt_message: prompt.clone(),
        round_messages: vec![prompt],
        removed_turn_count: 1,
        removed_message_count: 1,
    }];

    let candidates = build_history_rollback_candidates(&rounds);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].prompt, "before pasted body after");
    assert_eq!(candidates[0].draft.text, "before [Paste #1] after");
    assert_eq!(candidates[0].draft.draft_attachments.len(), 1);
    assert!(matches!(
        &candidates[0].draft.draft_attachments[0].kind,
        ComposerDraftAttachmentKind::LargePaste { payload } if payload == "pasted body"
    ));
    assert_eq!(
        candidates[0].draft.draft_attachments[0]
            .placeholder
            .as_deref(),
        Some("[Paste #1]")
    );
}
