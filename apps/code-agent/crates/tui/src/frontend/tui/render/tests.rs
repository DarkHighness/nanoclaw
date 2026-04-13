use super::chrome::{
    approval_preview_lines, build_approval_text, build_composer_line, build_composer_text,
    build_user_input_text, composer_cursor_position, composer_height, should_render_side_rail,
};
use super::history_rollback_overlay::{
    build_history_rollback_list_text, build_history_rollback_preview_text,
};
use super::main_pane_viewport_height;
use super::picker::build_composer_hint_text;
use super::statusline::{format_footer_context, format_toast_line, toast_height};
use super::theme::palette;
use super::tool_review_overlay::{build_tool_review_list_text, build_tool_review_preview_text};
use super::transcript::TranscriptEntryKind;
use super::transcript::build_transcript_lines;
use super::transcript::build_transcript_lines_for_width;
use super::transcript::transcript_content_area;
use super::transcript_markdown::render_markdown_body;
use super::transcript_shell::{
    animated_progress_text_spans, live_progress_lines, render_shell_summary_body,
};
use super::view::{
    build_collection_text, build_command_palette_text, build_key_value_text,
    build_statusline_picker_text, build_theme_picker_text, should_render_view_title,
};
use super::welcome::build_welcome_lines;
use crate::frontend::tui::UserInputView;
use crate::frontend::tui::approval::ApprovalPrompt;
use crate::frontend::tui::commands::{
    ComposerCompletionHint, SkillInvocationHint, SkillInvocationSpec, SlashCommandArgumentHint,
    SlashCommandArgumentSpec, SlashCommandArgumentValue, SlashCommandHint, SlashCommandSpec,
    SlashInvocationSpec,
};
use crate::frontend::tui::state::{
    ActiveToolCell, ComposerContextHint, ComposerDraftAttachmentKind, ComposerDraftAttachmentState,
    ComposerDraftState, HistoryRollbackCandidate, InspectorAction, InspectorEntry, MainPaneMode,
    StatusLinePickerState, ThemePickerState, ToastTone, ToolSelectionTarget, TrackedTaskSummary,
    TranscriptEntry, TranscriptShellDetail, TranscriptToolStatus, TuiState,
};
use crate::interaction::{
    ApprovalContent, ApprovalContentKind, ApprovalOrigin, PendingControlKind, PendingControlReason,
    PendingControlSummary, PermissionProfile, PermissionRequestPrompt, UserInputAnswer,
    UserInputOption, UserInputQuestion,
};
use crate::theme::ThemeSummary;
use crate::tool_render::{
    ToolCommand, ToolCompletionState, ToolDetail, ToolDetailBlockKind, ToolDetailLabel, ToolReview,
    ToolReviewItem, ToolReviewItemKind, ToolReviewKind,
};
use agent::types::{MessageId, MessagePart, TaskId, TaskOrigin, TaskStatus};
use ratatui::layout::Rect;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn builtin_slash(spec: SlashCommandSpec) -> SlashInvocationSpec {
    SlashInvocationSpec::Builtin(spec)
}

#[test]
fn key_value_text_renders_section_headers_without_treating_them_as_pairs() {
    let rendered = build_key_value_text(&[
        section_entry("Session"),
        field_entry("session ref", "abc123"),
        command_entry("/sessions [query]"),
    ]);
    let lines = rendered.lines;
    assert_eq!(lines[0].spans[0].content.as_ref(), "Session");
    assert_eq!(lines[1].spans[0].content.as_ref(), "session ref:");
    assert_eq!(lines[2].spans[0].content.as_ref(), "/sessions [query]");
}

#[test]
fn key_value_text_preserves_prefixed_summary_blocks() {
    let rendered = build_key_value_text(&[success_summary_entry(
        "Exported transcript text",
        vec![
            raw_detail("session-1"),
            continuation_detail("Wrote 4 items to /workspace/out.txt"),
        ],
    )]);
    let lines = rendered.lines;
    assert_eq!(lines[0].spans[0].content.as_ref(), "✔");
    assert_eq!(
        lines[0].spans[2].content.as_ref(),
        "Exported transcript text"
    );
    let raw_line = lines
        .iter()
        .find(|line| line_text_for(line).contains("session-1"))
        .expect("expected summary raw detail");
    assert_eq!(raw_line.spans[0].content.as_ref(), "  └ ");
    let continuation = lines
        .iter()
        .find(|line| line_text_for(line).contains("Wrote 4 items to /workspace/out.txt"))
        .expect("expected summary continuation detail");
    assert_eq!(continuation.spans[0].content.as_ref(), "    ");
}

#[test]
fn key_value_text_reuses_transcript_rendering_for_shell_summary_lines() {
    let rendered = build_key_value_text(&[shell_summary_entry(
        "Reattached session",
        vec![raw_detail("session session-1")],
    )]);
    let headline = rendered
        .lines
        .iter()
        .find(|line| line_text_for(line).contains("Reattached session"))
        .expect("expected shell summary headline");
    assert_eq!(headline.spans[0].content.as_ref(), "•");
    assert_eq!(headline.spans[2].content.as_ref(), "Reattached session");
    let detail = rendered
        .lines
        .iter()
        .find(|line| line_text_for(line).contains("session session-1"))
        .expect("expected shell summary detail");
    assert_eq!(detail.spans[0].content.as_ref(), "  └ ");
    assert_eq!(detail.spans[1].content.as_ref(), "session session-1");
}

#[test]
fn transcript_entries_render_with_codex_like_prefixes() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![transcript_entry("• hello world")];

    let lines = build_transcript_lines(&state);

    assert_eq!(lines[0].spans[0].content.as_ref(), "•");
    assert_eq!(lines[0].spans[2].content.as_ref(), "hello world");
}

#[test]
fn transcript_inserts_turn_dividers_between_user_turns() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![
        transcript_entry("› first"),
        transcript_entry("• reply"),
        transcript_entry("› second"),
    ];

    let rendered = build_transcript_lines_for_width(&state, 24);
    let divider = rendered
        .iter()
        .find(|line| line_text_for(line).contains('─'))
        .expect("expected turn divider");

    let divider_text = line_text_for(divider);
    assert_eq!(divider_text.chars().count(), 24);
    assert!(divider_text.chars().all(|ch| ch == '─'));
}

#[test]
fn transcript_content_area_reserves_top_breathing_room() {
    let area = Rect::new(0, 0, 80, 20);

    let content = transcript_content_area(area);

    assert_eq!(content.y, 1);
    assert_eq!(content.width, 80);
    assert_eq!(content.height, 19);
}

#[test]
fn transcript_separates_assistant_and_tool_entries_with_breathing_room() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![
        transcript_entry("• assistant reply"),
        running_tool_transcript_entry(),
        transcript_entry("› next prompt"),
    ];

    let rendered = build_transcript_lines(&state);

    assert_eq!(line_text_for(&rendered[0]), "• assistant reply");
    assert!(line_text_for(&rendered[1]).is_empty());
    assert_eq!(line_text_for(&rendered[2]), "• Running cargo test");
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("cargo test"))
    );
}

#[test]
fn transcript_collapses_tool_details_by_default() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![finished_tool_transcript_entry()];

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Ran cargo test"))
    );
    assert!(rendered.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains("hidden line"))
    }));
    assert!(rendered.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains("exit 0"))
    }));
    assert!(!rendered.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains("ok"))
    }));
}

#[test]
fn transcript_expands_tool_details_when_enabled() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        show_tool_details: true,
        ..TuiState::default()
    };
    state.transcript = vec![finished_tool_transcript_entry()];

    let rendered = build_transcript_lines(&state);

    assert!(rendered.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains("exit 0"))
    }));
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("ok"))
    );
}

#[test]
fn transcript_renders_exec_commands_with_highlighted_action_verb_and_shell_subject() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "exec_command",
        vec![command_tool_detail("cargo test -- --nocapture")],
    )];

    let rendered = build_transcript_lines(&state);
    let line = rendered
        .iter()
        .find(|line| line_text_for(line).contains("Ran cargo test -- --nocapture"))
        .expect("expected command headline");

    assert_eq!(line_text_for(line), "• Ran cargo test -- --nocapture");
    assert!(line.spans.iter().any(|span| {
        span.content.as_ref() == "Ran"
            && span.style.fg == Some(palette().assistant)
            && span
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
    }),);
    assert!(line.spans.iter().any(|span| {
        span.content.as_ref() == "cargo"
            && span.style.fg == Some(palette().header)
            && span
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
    }),);
    assert!(
        line.spans
            .iter()
            .any(|span| span.content.as_ref() == "--nocapture"
                && span.style.fg == Some(palette().accent)),
    );
}

#[test]
fn transcript_renders_exploration_commands_with_summary_detail() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "exec_command",
        vec![command_tool_detail("sed -n '1,200p' src/lib.rs tests.rs")],
    )];

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Explored"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Read lib.rs, tests.rs"))
    );
}

#[test]
fn transcript_keeps_piped_shell_commands_as_ran_entries() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "exec_command",
        vec![command_tool_detail(
            "find /tmp -maxdepth 2 -type d 2>/dev/null | sed -n '1,80p'",
        )],
    )];

    let rendered = build_transcript_lines(&state);

    assert!(rendered.iter().any(|line| {
        line_text_for(line).contains("Ran find /tmp -maxdepth 2 -type d 2>/dev/null | sed -n")
    }));
    assert!(
        !rendered
            .iter()
            .any(|line| line_text_for(line).contains("Explored"))
    );
}

#[test]
fn transcript_colors_completed_command_marker_from_typed_completion_state() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool_with_completion(
        TranscriptToolStatus::Finished,
        "exec_command",
        vec![command_tool_detail("cargo test -- --nocapture")],
        ToolCompletionState::Failure,
    )];

    let rendered = build_transcript_lines(&state);
    let line = rendered
        .iter()
        .find(|line| line_text_for(line).contains("Ran cargo test -- --nocapture"))
        .expect("expected completed command headline");

    assert_eq!(line.spans[0].content.as_ref(), "•");
    assert_eq!(line.spans[0].style.fg, Some(palette().error));
}

#[test]
fn selected_tool_entry_surfaces_review_action_in_collapsed_mode() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        tool_selection: Some(ToolSelectionTarget::Transcript(0)),
        ..TuiState::default()
    };
    state.transcript = vec![reviewable_tool_transcript_entry()];

    let rendered = build_transcript_lines(&state);

    assert!(rendered.iter().any(|line| {
        let text = line_text_for(line);
        text.contains("review diff") && text.contains("src/lib.rs")
    }));
}

#[test]
fn selected_transcript_cells_use_focus_rail_and_elevated_surface() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        tool_selection: Some(ToolSelectionTarget::Transcript(0)),
        ..TuiState::default()
    };
    state.transcript = vec![finished_tool_transcript_entry()];

    let rendered = build_transcript_lines(&state);
    let first_visible = rendered
        .iter()
        .find(|line| !line_text_for(line).trim().is_empty())
        .expect("expected rendered transcript content");

    assert!(line_text_for(first_visible).starts_with("▌ "));
    for line in &rendered {
        for span in &line.spans {
            assert_eq!(span.style.bg, Some(palette().elevated_surface()));
        }
    }
}

#[test]
fn transcript_renders_resume_summary_above_history() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        inspector_title: "Resume".to_string(),
        inspector: vec![success_summary_entry(
            "Reattached session",
            vec![raw_detail("session session-1")],
        )],
        ..TuiState::default()
    };
    state.transcript = vec![transcript_entry("• done")];

    let rendered = build_transcript_lines(&state);

    assert_eq!(rendered[0].spans[0].content.as_ref(), "Resume");
    assert_eq!(rendered[2].spans[0].content.as_ref(), "✔");
    assert_eq!(rendered[2].spans[2].content.as_ref(), "Reattached session");
}

#[test]
fn composer_line_describes_armed_history_rollback() {
    let mut state = TuiState::default();
    state.prime_history_rollback();

    let line = build_composer_line(&state);

    assert!(line_text_for(&line).contains("history rollback armed"));
    assert!(line_text_for(&line).contains("esc choose turn"));
}

#[test]
fn history_rollback_overlay_renders_selection_list_and_preview() {
    let mut state = TuiState::default();
    let _ = state.open_history_rollback_overlay(vec![
        HistoryRollbackCandidate {
            message_id: MessageId::from("msg-1"),
            prompt: "first prompt".to_string(),
            draft: ComposerDraftState::from_text("first prompt"),
            turn_preview_lines: vec![
                transcript_entry("› first prompt"),
                transcript_entry("• first answer"),
            ],
            removed_turn_count: 2,
            removed_message_count: 4,
        },
        HistoryRollbackCandidate {
            message_id: MessageId::from("msg-2"),
            prompt: "second prompt".to_string(),
            draft: ComposerDraftState::from_text("second prompt"),
            turn_preview_lines: vec![
                transcript_entry("› second prompt"),
                transcript_entry("• second answer"),
            ],
            removed_turn_count: 1,
            removed_message_count: 2,
        },
    ]);

    let list = build_history_rollback_list_text(&state);
    let preview = build_history_rollback_preview_text(&state);

    assert!(
        text_lines(&list)
            .iter()
            .any(|line| line.contains("first prompt"))
    );
    assert!(
        text_lines(&list)
            .iter()
            .any(|line| line.contains("second prompt"))
    );
    assert!(
        text_lines(&preview)
            .iter()
            .any(|line| line.contains("Prompt"))
    );
    assert!(
        text_lines(&preview)
            .iter()
            .any(|line| line.contains("second answer"))
    );
}

#[test]
fn history_rollback_overlay_uses_attachment_aware_preview_labels() {
    let mut state = TuiState::default();
    let _ = state.open_history_rollback_overlay(vec![HistoryRollbackCandidate {
        message_id: MessageId::from("msg-1"),
        prompt: "[image_url:https://example.com/assets/failure.png image/png]".to_string(),
        draft: ComposerDraftState {
            text: String::new(),
            cursor: 0,
            draft_attachments: vec![ComposerDraftAttachmentState {
                placeholder: None,
                kind: ComposerDraftAttachmentKind::RemoteImage {
                    requested_url: "https://example.com/assets/failure.png".to_string(),
                    part: MessagePart::ImageUrl {
                        url: "https://example.com/assets/failure.png".to_string(),
                        mime_type: Some("image/png".to_string()),
                    },
                },
            }],
        },
        turn_preview_lines: vec![transcript_entry("› restore attachments")],
        removed_turn_count: 1,
        removed_message_count: 1,
    }]);

    let list = build_history_rollback_list_text(&state);
    let preview = build_history_rollback_preview_text(&state);

    assert!(
        text_lines(&list)
            .iter()
            .any(|line| line.contains("#1 image · failure.png"))
    );
    assert!(
        text_lines(&preview)
            .iter()
            .any(|line| line.contains("#1 image · failure.png"))
    );
}

#[test]
fn welcome_lines_keep_the_start_screen_sparse() {
    let mut state = TuiState::default();
    state.session.workspace_name = "nanoclaw".to_string();
    state.session.model = "gpt-5.4".to_string();
    state.session.model_reasoning_effort = Some("high".to_string());

    let lines = build_welcome_lines(&state, 140, 28);

    assert!(lines.iter().any(|line| {
        line_text_for(line).contains("▄▄     ▄▄▄    ▄▄       ▄▄     ▄▄▄")
    }));
    assert!(lines.iter().any(|line| {
        line_text_for(line).contains("▀██▀    ██  ▀██▀  ▀█▄█")
    }));
    assert!(
        lines
            .iter()
            .any(|line| { line_text_for(line).contains("Terminal shell for focused coding work") })
    );
    assert!(lines.iter().any(|line| {
        line_text_for(line).contains("workspace nanoclaw  ·  model gpt-5.4 · high")
    }));
    assert!(lines.iter().any(|line| {
        line_text_for(line).contains("tools 0  ·  mcp 0  ·  skills 0  ·  plugins 0/0")
    }));
    assert!(lines.iter().any(|line| {
        line_text_for(line)
            .contains("Ask for a change, start with $skill_name or /skill_name for an explicit skill, review history, or run /help.")
    }));
}

#[test]
fn welcome_lines_switch_to_the_compact_logo_on_narrow_viewports() {
    let mut state = TuiState::default();
    state.session.workspace_name = "nanoclaw".to_string();
    state.session.model = "gpt-5.4".to_string();

    let lines = build_welcome_lines(&state, 80, 28);

    assert!(lines.iter().any(|line| {
        line_text_for(line).contains("███  ██ ▄████▄ ███  ██ ▄████▄")
    }));
    assert!(
        lines
            .iter()
            .any(|line| { line_text_for(line).contains("Terminal shell for coding work") })
    );
}

#[test]
fn pending_control_band_surfaces_selected_prompt_and_editing_state() {
    let mut state = TuiState::default();
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "write a regression test".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Steer,
            preview: "keep the diff small".to_string(),
            reason: Some(PendingControlReason::InlineEnter),
        },
    ];
    let _ = state.open_pending_control_picker(true);

    let text = super::picker::build_pending_control_text(&state);

    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("Queued Follow-ups"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("write a regression test"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("keep the diff small"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("› Queued Steer · latest draft"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line) == "  • prompt write a regression test")
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line) == "› Queued Steer · latest draft")
    );
    let prompt_row = text
        .lines
        .iter()
        .position(|line| line_text_for(line).contains("write a regression test"))
        .expect("expected prompt row");
    let selected_row = text
        .lines
        .iter()
        .position(|line| line_text_for(line).contains("keep the diff small"))
        .expect("expected selected steer row");
    assert!(selected_row > prompt_row);
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("after 1 older item(s)"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("from Enter while running"))
    );
    assert!(
        text.lines
            .iter()
            .all(|line| !line_text_for(line).contains("enter edit"))
    );

    let selected = state.begin_pending_control_edit().unwrap();
    assert_eq!(selected.id, "cmd_2");
    let text = super::picker::build_pending_control_text(&state);
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("Editing Queued Steer"))
    );
}

#[test]
fn composer_line_surfaces_pending_edit_shortcuts() {
    let mut state = TuiState::default();
    state.input = "keep the diff small".to_string();
    state.editing_pending_control = Some(crate::frontend::tui::state::PendingControlEditorState {
        id: "cmd_2".to_string(),
        kind: PendingControlKind::Steer,
    });

    let line = build_composer_line(&state);
    let text = line_text_for(&line);

    assert!(text.contains("edit queued steer"));
    assert!(text.contains("enter/tab save"));
    assert!(text.contains("esc cancel"));
}

#[test]
fn composer_line_surfaces_pending_picker_shortcuts() {
    let mut state = TuiState::default();
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "write a regression test".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Steer,
            preview: "keep the diff small".to_string(),
            reason: Some(PendingControlReason::InlineEnter),
        },
    ];
    let _ = state.open_pending_control_picker(true);

    let line = build_composer_line(&state);
    let text = line_text_for(&line);

    assert!(text.contains("selected steer"));
    assert!(text.contains("latest draft"));
    assert!(text.contains("enter/alt+t edit"));
    assert!(text.contains("del withdraw"));
    assert!(text.contains("esc close"));
}

#[test]
fn composer_line_surfaces_live_task_hint_while_turn_running() {
    let mut state = TuiState::default();
    state.turn_running = true;
    state.composer_context_hint = Some(ComposerContextHint::LiveTaskFinished {
        task_id: TaskId::from("task_123456"),
        status: TaskStatus::Completed,
    });

    let line = build_composer_line(&state);
    let text = line_text_for(&line);

    assert!(text.contains("task task_"));
    assert!(text.contains("completed"));
    assert!(text.contains("enter steer"));
    assert!(text.contains("tab queue"));
    assert!(text.contains("/task inspect"));
}

#[test]
fn composer_line_surfaces_live_task_hint_while_idle() {
    let mut state = TuiState::default();
    state.composer_context_hint = Some(ComposerContextHint::LiveTaskFinished {
        task_id: TaskId::from("task_123456"),
        status: TaskStatus::Failed,
    });

    let line = build_composer_line(&state);
    let text = line_text_for(&line);

    assert!(text.contains("task task_"));
    assert!(text.contains("failed"));
    assert!(text.contains("type follow-up"));
    assert!(text.contains("/task inspect"));
    assert!(!text.contains("enter steer"));
    assert!(!text.contains("tab queue"));
}

#[test]
fn multiline_composer_text_keeps_followup_lines_and_shortcuts_visible() {
    let mut state = TuiState::default();
    state.input = "first line\nsecond line".to_string();
    state.editing_pending_control = Some(crate::frontend::tui::state::PendingControlEditorState {
        id: "cmd_2".to_string(),
        kind: PendingControlKind::Steer,
    });

    let text = build_composer_text(&state, None);
    let lines = text_lines(&text);

    assert_eq!(lines[0], "› edit queued steer · first line");
    assert_eq!(lines[1], "  second line");
    assert!(lines[2].contains("enter/tab save"));
    assert_eq!(composer_height(&state, None), 5);
}

#[test]
fn composer_cursor_position_reserves_top_padding() {
    let state = TuiState::default();
    let position = composer_cursor_position(
        Rect::new(0, 20, 80, composer_height(&state, None)),
        &state,
        None,
    );

    assert_eq!(position.y, 21);
}

#[test]
fn multiline_composer_text_keeps_local_attachment_placeholders_inline() {
    let mut state = TuiState::default();
    state.draft_attachments = vec![
        ComposerDraftAttachmentState {
            placeholder: Some("[Image #1]".to_string()),
            kind: ComposerDraftAttachmentKind::LocalImage {
                requested_path: "artifacts/failure.png".to_string(),
                mime_type: Some("image/png".to_string()),
                part: Some(MessagePart::Image {
                    mime_type: "image/png".to_string(),
                    data_base64: "png-data".to_string(),
                }),
            },
        },
        ComposerDraftAttachmentState {
            placeholder: Some("[File #1]".to_string()),
            kind: ComposerDraftAttachmentKind::LocalFile {
                requested_path: "reports/run.pdf".to_string(),
                file_name: Some("run.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                part: Some(MessagePart::File {
                    file_name: Some("run.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    data_base64: Some("pdf-data".to_string()),
                    uri: Some("reports/run.pdf".to_string()),
                }),
            },
        },
    ];
    state.input = "[Image #1] [File #1]\ndescribe the failure".to_string();

    let text = build_composer_text(&state, None);
    let lines = text_lines(&text);

    assert_eq!(lines[0], "› [Image #1] [File #1]");
    assert_eq!(lines[1], "  describe the failure");
}

#[test]
fn multiline_composer_text_renders_remote_attachment_rows_above_prompt() {
    let mut state = TuiState::default();
    state.draft_attachments = vec![
        ComposerDraftAttachmentState {
            placeholder: None,
            kind: ComposerDraftAttachmentKind::RemoteImage {
                requested_url: "https://example.com/assets/failure.png".to_string(),
                part: MessagePart::ImageUrl {
                    url: "https://example.com/assets/failure.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                },
            },
        },
        ComposerDraftAttachmentState {
            placeholder: None,
            kind: ComposerDraftAttachmentKind::RemoteFile {
                requested_url: "https://example.com/reports/run.pdf".to_string(),
                part: MessagePart::File {
                    file_name: Some("run.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    data_base64: None,
                    uri: Some("https://example.com/reports/run.pdf".to_string()),
                },
            },
        },
    ];
    state.input = "summarize the remote artifacts".to_string();

    let text = build_composer_text(&state, None);
    let lines = text_lines(&text);

    assert_eq!(
        lines[0],
        "· #1 image · failure.png · https://example.com/assets/failure.png"
    );
    assert_eq!(
        lines[1],
        "· #2 file · run.pdf · https://example.com/reports/run.pdf"
    );
    assert_eq!(lines[2], "› summarize the remote artifacts");
}

#[test]
fn multiline_composer_text_highlights_selected_attachment_row() {
    let mut state = TuiState::default();
    state.draft_attachments = vec![
        ComposerDraftAttachmentState {
            placeholder: None,
            kind: ComposerDraftAttachmentKind::RemoteImage {
                requested_url: "https://example.com/assets/failure.png".to_string(),
                part: MessagePart::ImageUrl {
                    url: "https://example.com/assets/failure.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                },
            },
        },
        ComposerDraftAttachmentState {
            placeholder: None,
            kind: ComposerDraftAttachmentKind::RemoteFile {
                requested_url: "https://example.com/reports/run.pdf".to_string(),
                part: MessagePart::File {
                    file_name: Some("run.pdf".to_string()),
                    mime_type: Some("application/pdf".to_string()),
                    data_base64: None,
                    uri: Some("https://example.com/reports/run.pdf".to_string()),
                },
            },
        },
    ];
    state.selected_row_attachment = Some(1);
    state.input = "summarize the remote artifacts".to_string();

    let text = build_composer_text(&state, None);
    let lines = text_lines(&text);

    assert_eq!(
        lines[1],
        "› #2 file · run.pdf · https://example.com/reports/run.pdf"
    );
    assert!(lines[3].contains("delete detach"));
}

#[test]
fn user_input_band_renders_progress_and_other_shortcut() {
    let prompt = crate::interaction::UserInputPrompt {
        prompt_id: "prompt_1".to_string(),
        questions: vec![
            UserInputQuestion {
                id: "scope_choice".to_string(),
                header: "Scope".to_string(),
                question: "Which scope should I target?".to_string(),
                options: vec![
                    UserInputOption {
                        label: "Runtime".to_string(),
                        description: "Touches substrate code.".to_string(),
                    },
                    UserInputOption {
                        label: "Host".to_string(),
                        description: "Touches app code.".to_string(),
                    },
                ],
            },
            UserInputQuestion {
                id: "risk_choice".to_string(),
                header: "Risk".to_string(),
                question: "Should I keep the change narrow?".to_string(),
                options: vec![
                    UserInputOption {
                        label: "Yes".to_string(),
                        description: "Avoid broader cleanup.".to_string(),
                    },
                    UserInputOption {
                        label: "No".to_string(),
                        description: "Broader cleanup is acceptable.".to_string(),
                    },
                ],
            },
        ],
    };
    let flow = crate::frontend::tui::ActiveUserInputState {
        prompt_id: prompt.prompt_id.clone(),
        current_question: 1,
        answers: BTreeMap::from([(
            "scope_choice".to_string(),
            UserInputAnswer {
                answers: vec!["Runtime".to_string()],
            },
        )]),
        collecting_other_note: false,
    };
    let text = build_user_input_text(&UserInputView {
        prompt: &prompt,
        flow: Some(&flow),
        input: "",
    });

    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("Question 2/2"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("1 answered"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("0 Other"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("1-9 choose"))
    );
}

#[test]
fn user_input_band_renders_other_note_mode() {
    let prompt = crate::interaction::UserInputPrompt {
        prompt_id: "prompt_1".to_string(),
        questions: vec![UserInputQuestion {
            id: "scope_choice".to_string(),
            header: "Scope".to_string(),
            question: "Which scope should I target?".to_string(),
            options: vec![
                UserInputOption {
                    label: "Runtime".to_string(),
                    description: "Touches substrate code.".to_string(),
                },
                UserInputOption {
                    label: "Host".to_string(),
                    description: "Touches app code.".to_string(),
                },
            ],
        }],
    };
    let flow = crate::frontend::tui::ActiveUserInputState {
        prompt_id: prompt.prompt_id.clone(),
        current_question: 0,
        answers: BTreeMap::new(),
        collecting_other_note: true,
    };
    let text = build_user_input_text(&UserInputView {
        prompt: &prompt,
        flow: Some(&flow),
        input: "Something else",
    });

    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("Type the alternate answer"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("Something else"))
    );
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("esc back to options"))
    );
}

#[test]
fn animated_progress_text_preserves_the_full_status_label() {
    let spans = animated_progress_text_spans("Working · exec_command", 225);
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(text, "Working · exec_command");
    assert!(spans.len() > 4);
}

#[test]
fn shell_summary_highlights_requested_running_and_finished_status_phrases() {
    for headline in [
        "Requested exec_command",
        "Queued follow-ups · 2",
        "Running exec_command",
        "Finished exec_command",
    ] {
        let rendered =
            render_shell_summary_body(headline, "•", TranscriptEntryKind::ShellSummary, Some(225));
        assert_eq!(line_text_for(&rendered[0]), headline);
        assert!(rendered[0].spans.len() > 2);
    }
}

#[test]
fn collection_text_renders_shell_summary_blocks_for_history_rows() {
    let rendered = build_collection_text(
        "Sessions",
        &[
            section_entry("Sessions"),
            actionable_collection_entry(
                "sess_123  no prompt yet",
                "12 messages · 40 events · 2 agent sessions · resume attached",
            ),
        ],
        Some(0),
    );

    assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
    assert_eq!(
        rendered.lines[1].spans[2].content.as_ref(),
        "sess_123  no prompt yet"
    );
    assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "  ");
    assert_eq!(
        rendered.lines[2].spans[1].content.as_ref(),
        "12 messages · 40 events · 2 agent sessions · resume attached"
    );
}

#[test]
fn collection_text_keeps_history_rows_compact() {
    let rendered = build_collection_text(
        "Sessions",
        &[
            actionable_collection_entry("sess_123  no prompt yet", "12 messages · 40 events"),
            actionable_collection_entry("sess_456  resume prompt", "4 messages · 9 events"),
        ],
        Some(1),
    );

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "·");
    assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "›");
    assert_eq!(
        rendered.lines[2].spans[2].content.as_ref(),
        "sess_456  resume prompt"
    );
}

#[test]
fn statusline_picker_text_renders_checked_rows() {
    let rendered = build_statusline_picker_text(
        &TuiState::default().session.statusline,
        &StatusLinePickerState { selected: 1 },
    );

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "status line");
    assert!(
        rendered
            .lines
            .iter()
            .any(|line| line_text_for(line).contains("› [x] model"))
    );
    assert!(
        rendered
            .lines
            .iter()
            .any(|line| line_text_for(line).contains("[ ] session"))
    );
}

#[test]
fn theme_picker_text_renders_available_themes() {
    let rendered = build_theme_picker_text(
        "fjord",
        &[
            ThemeSummary {
                id: "graphite".to_string(),
                summary: "cool dark slate".to_string(),
            },
            ThemeSummary {
                id: "fjord".to_string(),
                summary: "deep blue with brighter cyan accents".to_string(),
            },
        ],
        &ThemePickerState {
            selected: 1,
            original_theme: "fjord".to_string(),
        },
    );

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "theme");
    assert!(
        rendered
            .lines
            .iter()
            .any(|line| line_text_for(line).contains("› [x] fjord"))
    );
    assert!(
        rendered
            .lines
            .iter()
            .any(|line| line_text_for(line).contains("[ ] graphite"))
    );
    assert!(
        rendered
            .lines
            .iter()
            .any(|line| line_text_for(line).contains("enter save"))
    );
    assert!(
        rendered
            .lines
            .iter()
            .any(|line| line_text_for(line).contains("esc restore"))
    );
}

#[test]
fn command_palette_text_matches_picker_style() {
    let rendered = build_command_palette_text(
        &[
            section_entry("Session"),
            actionable_collection_entry("/help [query]", "browse commands"),
            actionable_collection_entry("/sessions [query]", "browse persisted sessions"),
        ],
        Some(0),
    );

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "section");
    assert_eq!(rendered.lines[0].spans[2].content.as_ref(), "Session");
    assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
    assert_eq!(rendered.lines[1].spans[2].content.as_ref(), "/help [query]");
    assert_eq!(
        rendered.lines[2].spans[1].content.as_ref(),
        "browse commands"
    );
    assert_eq!(
        rendered.lines[3].spans[2].content.as_ref(),
        "/sessions [query]"
    );
}

#[test]
fn transcript_renders_compact_live_progress_line() {
    let state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working (2)".to_string(),
        turn_started_at: Some(Instant::now() - Duration::from_secs(128)),
        ..TuiState::default()
    };

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Working (2)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| { line_text_for(line).contains("2m 08s · esc to interrupt") })
    );
    assert!(!rendered.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains("$ cargo test"))
    }));
}

#[test]
fn live_progress_hides_queue_count_while_pending_picker_is_open() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        turn_started_at: Some(Instant::now() - Duration::from_secs(128)),
        active_tool_cells: vec![active_tool_entry(
            "call-1",
            "exec_command",
            TranscriptToolStatus::Running,
        )],
        ..TuiState::default()
    };
    state.pending_controls = vec![PendingControlSummary {
        id: "cmd_1".to_string(),
        kind: PendingControlKind::Prompt,
        preview: "write a regression test".to_string(),
        reason: None,
    }];
    state.session.queued_commands = state.pending_controls.len();
    let _ = state.open_pending_control_picker(true);

    let rendered = live_progress_lines(&state);
    let text = line_text_for(&rendered[0]);

    assert!(text.contains("Working · Running cargo test"));
    assert!(text.contains("2m 08s · esc to interrupt"));
    assert!(!text.contains("queued behind current tool"));
}

#[test]
fn transcript_hides_progress_line_while_tool_cell_is_active() {
    let state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        turn_started_at: Some(Instant::now() - Duration::from_secs(128)),
        active_tool_cells: vec![active_tool_entry(
            "call-1",
            "exec_command",
            TranscriptToolStatus::Running,
        )],
        ..TuiState::default()
    };

    let rendered = build_transcript_lines(&state);

    let running_count = rendered
        .iter()
        .filter(|line| line_text_for(line) == "• Running cargo test")
        .count();
    assert_eq!(running_count, 1);
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Working"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Working · Running cargo test"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Worked 2m 08s"))
    );
}

#[test]
fn live_progress_hides_idle_queue_summary_when_empty() {
    let state = TuiState::default();

    let rendered = live_progress_lines(&state);

    assert!(rendered.is_empty());
}

#[test]
fn transcript_merges_pending_controls_into_the_active_tool_timeline_cell() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        active_tool_cells: vec![active_tool_entry(
            "call-1",
            "exec_command",
            TranscriptToolStatus::Running,
        )],
        ..TuiState::default()
    };
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "write a regression test".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Steer,
            preview: "keep the diff small".to_string(),
            reason: Some(PendingControlReason::InlineEnter),
        },
    ];

    let rendered = build_transcript_lines(&state);

    let running_count = rendered
        .iter()
        .filter(|line| line_text_for(line) == "• Running cargo test")
        .count();
    assert_eq!(running_count, 1);
    let queued_headline = rendered
        .iter()
        .find(|line| line_text_for(line).contains("Queued Follow-ups · 2"))
        .expect("expected embedded queued follow-ups headline");
    assert!(queued_headline.spans.len() > 3);
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("latest Queued Steer"))
    );
    let queued_prompt_line = rendered
        .iter()
        .find(|line| line_text_for(line).contains("  └ older Queued Prompt"))
        .expect("expected embedded queued prompt continuation");
    assert!(
        queued_prompt_line
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "Queued Prompt")
    );
}

#[test]
fn transcript_bridges_pending_picker_into_the_active_tool_timeline() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        active_tool_cells: vec![active_tool_entry(
            "call-1",
            "exec_command",
            TranscriptToolStatus::Running,
        )],
        ..TuiState::default()
    };
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "write a regression test".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Steer,
            preview: "keep the diff small".to_string(),
            reason: Some(PendingControlReason::InlineEnter),
        },
    ];
    state.session.queued_commands = state.pending_controls.len();
    let _ = state.open_pending_control_picker(true);

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Queued follow-ups below"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("selected steer"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("latest draft"))
    );
    assert!(
        !rendered
            .iter()
            .any(|line| { line_text_for(line).contains("latest pending steer") })
    );
    assert!(
        !rendered
            .iter()
            .any(|line| { line_text_for(line).contains("queued behind current tool") })
    );
}

#[test]
fn transcript_surfaces_pending_picker_bridge_without_an_active_tool() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.pending_controls = vec![PendingControlSummary {
        id: "cmd_1".to_string(),
        kind: PendingControlKind::Prompt,
        preview: "write a regression test".to_string(),
        reason: None,
    }];
    state.session.queued_commands = state.pending_controls.len();
    let _ = state.open_pending_control_picker(true);

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Queued follow-ups below"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("selected prompt"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("only item"))
    );
}

#[test]
fn transcript_surfaces_pending_control_timeline_summary() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        ..TuiState::default()
    };
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "write a regression test".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Steer,
            preview: "keep the diff small".to_string(),
            reason: Some(PendingControlReason::InlineEnter),
        },
    ];

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Queued Follow-ups · 2"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("older Queued Prompt"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("write a regression test"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("latest Queued Steer"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("keep the diff small"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("from Enter while running"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Working"))
    );
}

#[test]
fn transcript_collapses_older_pending_controls_into_a_summary_line() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        ..TuiState::default()
    };
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "first".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "second".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_3".to_string(),
            kind: PendingControlKind::Steer,
            preview: "third".to_string(),
            reason: Some(PendingControlReason::ManualCommand),
        },
    ];

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("1 older pending"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("latest Queued Steer"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("from /steer"))
    );
}

#[test]
fn transcript_keeps_an_older_editing_pending_control_visible() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        turn_running: true,
        status: "Working".to_string(),
        ..TuiState::default()
    };
    state.pending_controls = vec![
        PendingControlSummary {
            id: "cmd_1".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "rewrite the summary".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_2".to_string(),
            kind: PendingControlKind::Prompt,
            preview: "second".to_string(),
            reason: None,
        },
        PendingControlSummary {
            id: "cmd_3".to_string(),
            kind: PendingControlKind::Steer,
            preview: "latest steer".to_string(),
            reason: Some(PendingControlReason::ManualCommand),
        },
    ];
    state.editing_pending_control = Some(crate::frontend::tui::state::PendingControlEditorState {
        id: "cmd_1".to_string(),
        kind: PendingControlKind::Prompt,
    });

    let rendered = build_transcript_lines(&state);

    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Editing Queued Prompt"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("rewrite the summary"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("latest Queued Steer"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("latest steer"))
    );
    assert!(
        rendered
            .iter()
            .all(|line| !line_text_for(line).contains("second"))
    );
}

#[test]
fn transcript_renders_markdown_blocks_without_fence_noise() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![transcript_entry(concat!(
        "• # Plan\n",
        "- inspect output\n",
        "1. rerun tests\n",
        "> keep the diff readable\n",
        "Use `rg` for search\n",
        "```diff\n",
        "+ added line\n",
        "- removed line\n",
        "@@ hunk\n",
        "```"
    ))];

    let rendered = build_transcript_lines(&state);
    assert_eq!(rendered[0].spans[0].content.as_ref(), "•");
    assert_eq!(rendered[0].spans[2].content.as_ref(), "Plan");
    assert!(rendered.iter().all(|line| {
        line.spans
            .iter()
            .all(|span| !span.content.as_ref().contains("```"))
    }));
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("inspect output"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("rerun tests"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("keep the diff readable"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| { line.spans.iter().any(|span| span.content.as_ref() == "rg") })
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("+ added line"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("- removed line"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("@@ hunk"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("··· diff"))
    );
}

#[test]
fn markdown_ordered_lists_keep_the_marker_and_first_word_on_one_line() {
    let rendered = render_markdown_body("1. rerun tests", TranscriptEntryKind::AssistantMessage);

    assert_eq!(line_text_for(&rendered[0]), "1.\u{00A0}rerun tests");
}

#[test]
fn transcript_motion_preserves_theme_span_accents() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    let index = state.push_transcript(TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "exec_command",
        vec![command_tool_detail("cargo test -- --nocapture")],
    ));
    let motion = state
        .transcript_motion
        .get_mut(index)
        .expect("expected transcript motion state");
    motion.settled_at = Some(Instant::now());

    let rendered = build_transcript_lines(&state);
    let line = rendered
        .iter()
        .find(|line| line_text_for(line).contains("Ran cargo test -- --nocapture"))
        .expect("expected rendered command headline line");

    assert!(line.spans.iter().any(|span| {
        span.content.as_ref() == "Ran" && span.style.fg == Some(palette().assistant)
    }));
    assert!(line.spans.iter().any(|span| {
        span.content.as_ref() == "cargo" && span.style.fg == Some(palette().header)
    }));
}

#[test]
fn transcript_keeps_fenced_block_label_as_first_visible_line() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![transcript_entry("• ```rust\nfn main() {}\n```")];

    let rendered = build_transcript_lines(&state);

    let first_visible = rendered
        .iter()
        .find(|line| !line_text_for(line).trim().is_empty())
        .expect("expected visible transcript line");
    assert_eq!(line_text_for(first_visible), "• ··· rust");
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("fn main() {}"))
    );
}

#[test]
fn transcript_preserves_span_level_syntax_highlighting_for_fenced_code() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![transcript_entry(concat!(
        "• ```rust\n",
        "fn main() {\n",
        "    println!(\"hi\");\n",
        "}\n",
        "```"
    ))];

    let rendered = build_transcript_lines(&state);
    let code_line = rendered
        .iter()
        .find(|line| line_text_for(line).contains("fn main() {"))
        .expect("expected fenced rust code line");

    assert!(
        code_line
            .spans
            .iter()
            .any(|span| { !span.content.as_ref().trim().is_empty() && span.style.fg.is_some() }),
        "expected fenced code spans to keep explicit syntax colors"
    );
}

#[test]
fn transcript_renders_shell_text_blocks_as_markdown_sections() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        show_tool_details: true,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::shell_summary_details(
        "Session note",
        vec![TranscriptShellDetail::TextBlock(vec![
            "# Findings".to_string(),
            "- inspect output".to_string(),
            "> keep the diff readable".to_string(),
            "```diff".to_string(),
            "+ added line".to_string(),
            "```".to_string(),
        ])],
    )];

    let rendered = build_transcript_lines(&state);
    let rendered_text = rendered.iter().map(line_text_for).collect::<Vec<_>>();

    assert!(
        rendered.iter().any(|line| {
            let text = line_text_for(line);
            text.contains("Findings") && text.contains("└")
        }),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("inspect output")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("keep the diff readable")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("··· diff")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("+ added line")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered.iter().all(|line| {
            line.spans
                .iter()
                .all(|span| !span.content.as_ref().contains("```"))
        }),
        "lines: {rendered_text:?}"
    );
}

#[test]
fn transcript_renders_tool_text_blocks_as_markdown_output() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        show_tool_details: true,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "browser_snapshot",
        vec![ToolDetail::TextBlock(vec![
            "# Snapshot".to_string(),
            "- primary button".to_string(),
            "Use `button.primary`".to_string(),
            "```diff".to_string(),
            "+ aria-label".to_string(),
            "```".to_string(),
        ])],
    )];

    let rendered = build_transcript_lines(&state);
    let rendered_text = rendered.iter().map(line_text_for).collect::<Vec<_>>();

    assert!(
        rendered.iter().any(|line| {
            let text = line_text_for(line);
            text.contains("Output") && text.contains("Snapshot")
        }),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("primary button")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "button.primary")
        }),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("··· diff")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("+ aria-label")),
        "lines: {rendered_text:?}"
    );
    assert!(
        rendered.iter().all(|line| {
            line.spans
                .iter()
                .all(|span| !span.content.as_ref().contains("```"))
        }),
        "lines: {rendered_text:?}"
    );
}

fn line_text_for(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn transcript_entry(line: &str) -> TranscriptEntry {
    line.into()
}

fn running_tool_transcript_entry() -> TranscriptEntry {
    TranscriptEntry::tool(
        TranscriptToolStatus::Running,
        "exec_command",
        vec![command_tool_detail("cargo test")],
    )
}

fn active_tool_entry(
    call_id: &str,
    tool_name: &str,
    status: TranscriptToolStatus,
) -> ActiveToolCell {
    ActiveToolCell::new(
        call_id,
        crate::frontend::tui::state::TranscriptToolEntry::new(
            status,
            tool_name,
            vec![command_tool_detail("cargo test")],
        ),
    )
}

fn finished_tool_transcript_entry() -> TranscriptEntry {
    TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "exec_command",
        vec![
            command_tool_detail("cargo test"),
            ToolDetail::Meta("exit 0".to_string()),
            ToolDetail::TextBlock(vec!["ok".to_string()]),
        ],
    )
}

fn command_tool_detail(command: &str) -> ToolDetail {
    ToolDetail::Command(ToolCommand::from_preview(&format!("$ {command}")))
}

fn reviewable_tool_transcript_entry() -> TranscriptEntry {
    TranscriptEntry::tool_with_review(
        TranscriptToolStatus::Finished,
        "write",
        vec![
            ToolDetail::LabeledValue {
                label: ToolDetailLabel::Effect,
                value: "Updated src/lib.rs".to_string(),
            },
            ToolDetail::LabeledValue {
                label: ToolDetailLabel::Files,
                value: "src/lib.rs".to_string(),
            },
            ToolDetail::ActionHint {
                key_hint: "r".to_string(),
                label: "review diff".to_string(),
                detail: Some("src/lib.rs".to_string()),
            },
        ],
        Some(ToolReview {
            kind: ToolReviewKind::FileDiff,
            summary: Some("Updated src/lib.rs".to_string()),
            items: vec![ToolReviewItem {
                title: "src/lib.rs".to_string(),
                preview_kind: ToolReviewItemKind::Diff,
                preview_lines: vec![
                    "--- src/lib.rs".to_string(),
                    "+++ src/lib.rs".to_string(),
                    "@@ -1,1 +1,1 @@".to_string(),
                    "-old()".to_string(),
                    "+new()".to_string(),
                ],
            }],
        }),
    )
}

#[test]
fn transcript_renders_task_tracking_cells() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::shell_summary_details(
        "Tracked Task task_123 (running)",
        vec![
            raw_detail("role reviewer"),
            raw_detail("summary inspect transcript renderer"),
            raw_detail("origin agent-created"),
        ],
    )];

    let rendered = build_transcript_lines(&state);
    assert_eq!(rendered[0].spans[0].content.as_ref(), "•");
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("Tracked Task task_123 (running)"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line_text_for(line).contains("summary inspect transcript renderer"))
    );
}

fn text_lines(text: &ratatui::text::Text<'_>) -> Vec<String> {
    text.lines.iter().map(line_text_for).collect()
}

#[test]
fn side_rail_stays_disabled_even_when_transcript_has_live_context() {
    let mut state = TuiState::default();
    state.main_pane = MainPaneMode::Transcript;
    state.session.tool_names = vec!["code_symbol_search".to_string()];
    state.tracked_tasks = vec![TrackedTaskSummary {
        task_id: TaskId::from("task_1"),
        role: "reviewer".to_string(),
        origin: TaskOrigin::AgentCreated,
        status: TaskStatus::Running,
        summary: Some("Refine transcript".to_string()),
        parent_agent_id: None,
        child_agent_id: None,
    }];

    assert!(!should_render_side_rail(
        &state,
        Rect {
            x: 0,
            y: 0,
            width: 140,
            height: 20,
        }
    ));
}

#[test]
fn tool_review_overlay_renders_file_list_and_preview() {
    let mut state = TuiState {
        tool_selection: Some(ToolSelectionTarget::Transcript(0)),
        ..TuiState::default()
    };
    state.transcript = vec![reviewable_tool_transcript_entry()];

    assert!(state.open_selected_tool_review_overlay());

    let list = build_tool_review_list_text(&state);
    let preview = build_tool_review_preview_text(&state);

    assert!(list.lines.iter().any(|line| {
        let text = line_text_for(line);
        text.contains("src/lib.rs")
    }));
    assert!(preview.lines.iter().any(|line| {
        let text = line_text_for(line);
        text.contains("+new()")
    }));
}

#[test]
fn tool_review_overlay_renders_structured_sections_for_non_diff_tools() {
    let mut state = TuiState {
        tool_selection: Some(ToolSelectionTarget::Transcript(0)),
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool(
        TranscriptToolStatus::Failed,
        "exec_command",
        vec![
            command_tool_detail("cargo test -- --nocapture"),
            ToolDetail::LabeledValue {
                label: ToolDetailLabel::Result,
                value: "exit 101".to_string(),
            },
            ToolDetail::NamedBlock {
                label: "Stderr".to_string(),
                kind: ToolDetailBlockKind::Stderr,
                lines: vec!["test failed".to_string()],
            },
        ],
    )];

    assert!(state.open_selected_tool_review_overlay());

    let list = build_tool_review_list_text(&state);
    let preview = build_tool_review_preview_text(&state);

    assert!(
        text_lines(&list)
            .iter()
            .any(|line| line.contains("Command"))
    );
    assert!(
        text_lines(&list)
            .iter()
            .any(|line| line.contains("cargo test -- --nocapture"))
    );
    assert!(
        text_lines(&preview)
            .iter()
            .any(|line| line.contains("cargo test -- --nocapture"))
    );
    assert!(
        text_lines(&preview)
            .iter()
            .any(|line| line.contains("Section Preview"))
    );
}

#[test]
fn approval_modal_uses_structured_command_preview() {
    let text = build_approval_text(&ApprovalPrompt {
        tool_name: "exec_command".to_string(),
        origin: ApprovalOrigin::Local,
        mode: Some("run".to_string()),
        working_directory: Some("/workspace/apps/code-agent".to_string()),
        content: ApprovalContent {
            kind: ApprovalContentKind::Command,
            preview: vec!["$ cargo test".to_string()],
        },
        reasons: vec!["sandbox policy requires approval".to_string()],
    });

    assert!(line_text_for(&text.lines[0]).contains("Approval Required"));
    assert!(line_text_for(&text.lines[0]).contains("exec_command"));
    assert!(line_text_for(&text.lines[1]).contains("Context"));
    assert!(line_text_for(&text.lines[1]).contains("/workspace/apps/code-agent"));
    assert!(line_text_for(&text.lines[1]).contains("run"));
    assert!(!line_text_for(&text.lines[1]).contains("local"));
    assert!(line_text_for(&text.lines[2]).contains("command"));
    assert!(line_text_for(&text.lines[3]).contains("Reason"));
    assert!(
        text.lines
            .iter()
            .any(|line| line_text_for(line).contains("Keys"))
    );
    assert!(text.lines.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains("$ cargo test"))
    }));
    assert!(text.lines.iter().any(|line| {
        line.spans.iter().any(|span| {
            span.content
                .as_ref()
                .contains("sandbox policy requires approval")
        })
    }));
}

#[test]
fn approval_modal_hides_local_origin_when_it_adds_no_operator_value() {
    let text = build_approval_text(&ApprovalPrompt {
        tool_name: "write".to_string(),
        origin: ApprovalOrigin::Local,
        mode: None,
        working_directory: None,
        content: ApprovalContent {
            kind: ApprovalContentKind::Arguments,
            preview: vec!["src/main.rs".to_string()],
        },
        reasons: vec!["needs approval".to_string()],
    });

    assert!(line_text_for(&text.lines[0]).contains("Approval Required"));
    assert!(line_text_for(&text.lines[0]).contains("write"));
    assert!(line_text_for(&text.lines[1]).contains("arguments"));
    assert!(
        text.lines
            .iter()
            .all(|line| !line_text_for(line).contains("local"))
    );
}

#[test]
fn approval_preview_lines_collapse_long_argument_blocks() {
    let lines = approval_preview_lines(&[
        "one".to_string(),
        "two".to_string(),
        "three".to_string(),
        "four".to_string(),
        "five".to_string(),
    ]);

    assert_eq!(lines, vec!["one", "two", "three", "… +2 lines"]);
}

#[test]
fn permission_request_modal_does_not_shrink_main_pane_viewport() {
    let state = TuiState::default();
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };
    let prompt = PermissionRequestPrompt {
        prompt_id: "perm-1".to_string(),
        reason: Some("need write access".to_string()),
        requested: PermissionProfile {
            read_roots: Vec::new(),
            write_roots: vec!["/workspace".to_string()],
            network_full: false,
            network_domains: Vec::new(),
        },
        current_turn: PermissionProfile::default(),
        current_session: PermissionProfile::default(),
    };

    let viewport = main_pane_viewport_height(area, &state, None, Some(&prompt), None);

    assert_eq!(viewport, 30 - composer_height(&state, None) - 1);
}

#[test]
fn command_hint_text_surfaces_selected_usage_and_matches() {
    let rendered = build_composer_hint_text(&ComposerCompletionHint::Slash(SlashCommandHint {
        selected: builtin_slash(SlashCommandSpec {
            section: "History",
            name: "sessions",
            usage: "sessions [query]",
            summary: "browse persisted sessions",
        }),
        matches: vec![
            builtin_slash(SlashCommandSpec {
                section: "History",
                name: "sessions",
                usage: "sessions [query]",
                summary: "browse persisted sessions",
            }),
            builtin_slash(SlashCommandSpec {
                section: "History",
                name: "session",
                usage: "session <session-ref>",
                summary: "open persisted session",
            }),
        ],
        selected_match_index: 0,
        arguments: None,
        exact: false,
    }));

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "Commands");
    assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
    assert_eq!(
        rendered.lines[1].spans[2].content.as_ref(),
        "/sessions [query]"
    );
    assert_eq!(
        rendered.lines[1].spans[4].content.as_ref(),
        "browse persisted sessions"
    );
    assert_eq!(
        rendered.lines[2].spans[1].content.as_ref(),
        "/session <session-ref>"
    );
    assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "Tab Complete");
    assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "Enter Accept");
}

#[test]
fn command_hint_text_surfaces_argument_progress() {
    let rendered = build_composer_hint_text(&ComposerCompletionHint::Slash(SlashCommandHint {
        selected: builtin_slash(SlashCommandSpec {
            section: "Export",
            name: "export_session",
            usage: "export_session <session-ref> <path>",
            summary: "write session export",
        }),
        matches: vec![builtin_slash(SlashCommandSpec {
            section: "Export",
            name: "export_session",
            usage: "export_session <session-ref> <path>",
            summary: "write session export",
        })],
        selected_match_index: 0,
        arguments: Some(SlashCommandArgumentHint {
            provided: vec![SlashCommandArgumentValue {
                placeholder: "<session-ref>",
                value: "session_123".to_string(),
            }],
            next: Some(SlashCommandArgumentSpec {
                placeholder: "<path>",
                required: true,
            }),
        }),
        exact: true,
    }));

    assert_eq!(rendered.lines[2].spans[1].content.as_ref(), "<session-ref>");
    assert_eq!(rendered.lines[2].spans[3].content.as_ref(), "session_123");
    assert!(
        rendered.lines[2]
            .spans
            .iter()
            .any(|span| span.content.as_ref().contains("<path>"))
    );
    assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "Keep Typing");
    assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "Keep Typing");
}

#[test]
fn command_hint_text_keeps_enter_run_for_optional_arguments() {
    let rendered = build_composer_hint_text(&ComposerCompletionHint::Slash(SlashCommandHint {
        selected: builtin_slash(SlashCommandSpec {
            section: "Session",
            name: "help",
            usage: "help [query]",
            summary: "browse commands",
        }),
        matches: vec![builtin_slash(SlashCommandSpec {
            section: "Session",
            name: "help",
            usage: "help [query]",
            summary: "browse commands",
        })],
        selected_match_index: 0,
        arguments: Some(SlashCommandArgumentHint {
            provided: Vec::new(),
            next: Some(SlashCommandArgumentSpec {
                placeholder: "[query]",
                required: false,
            }),
        }),
        exact: true,
    }));

    assert_eq!(rendered.lines[2].spans[1].content.as_ref(), "[query]");
    assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "Enter Run");
    assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "Enter Run");
}

#[test]
fn command_hint_text_shows_browse_window_ellipsis() {
    let rendered = build_composer_hint_text(&ComposerCompletionHint::Slash(SlashCommandHint {
        selected: builtin_slash(SlashCommandSpec {
            section: "History",
            name: "resume",
            usage: "resume <agent-session-ref>",
            summary: "reattach agent session",
        }),
        matches: vec![
            builtin_slash(SlashCommandSpec {
                section: "Session",
                name: "help",
                usage: "help",
                summary: "browse commands",
            }),
            builtin_slash(SlashCommandSpec {
                section: "Session",
                name: "status",
                usage: "status",
                summary: "session overview",
            }),
            builtin_slash(SlashCommandSpec {
                section: "Session",
                name: "new",
                usage: "new",
                summary: "fresh top-level session",
            }),
            builtin_slash(SlashCommandSpec {
                section: "History",
                name: "sessions",
                usage: "sessions [query]",
                summary: "browse persisted sessions",
            }),
            builtin_slash(SlashCommandSpec {
                section: "History",
                name: "session",
                usage: "session <session-ref>",
                summary: "open persisted session",
            }),
            builtin_slash(SlashCommandSpec {
                section: "History",
                name: "resume",
                usage: "resume <agent-session-ref>",
                summary: "reattach agent session",
            }),
            builtin_slash(SlashCommandSpec {
                section: "Agents",
                name: "live_tasks",
                usage: "live_tasks",
                summary: "list live child agents",
            }),
        ],
        selected_match_index: 5,
        arguments: None,
        exact: false,
    }));

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "Commands");
    assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "… 2 earlier");
    assert_eq!(
        rendered.lines[5].spans[2].content.as_ref(),
        "/resume <agent-session-ref>"
    );
    assert_eq!(rendered.lines[6].spans[0].content.as_ref(), "… 1 more");
}

#[test]
fn skill_hint_text_surfaces_aliases_and_tags() {
    let rendered = build_composer_hint_text(&ComposerCompletionHint::Skill(SkillInvocationHint {
        selected: SkillInvocationSpec {
            name: "openai-docs".to_string(),
            description: "Use official OpenAI docs".to_string(),
            aliases: vec!["docs".to_string()],
            tags: vec!["api".to_string()],
        },
        matches: vec![
            SkillInvocationSpec {
                name: "openai-docs".to_string(),
                description: "Use official OpenAI docs".to_string(),
                aliases: vec!["docs".to_string()],
                tags: vec!["api".to_string()],
            },
            SkillInvocationSpec {
                name: "frontend-design".to_string(),
                description: "Build polished interfaces".to_string(),
                aliases: vec!["ui".to_string()],
                tags: vec!["design".to_string()],
            },
        ],
        selected_match_index: 0,
        exact: false,
    }));

    assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "Skills");
    assert_eq!(rendered.lines[1].spans[2].content.as_ref(), "$openai-docs");
    assert!(
        rendered.lines[2]
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "$docs")
    );
    assert!(
        rendered.lines[3]
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "api")
    );
    assert_eq!(rendered.lines[5].spans[3].content.as_ref(), "Tab Use");
    assert_eq!(rendered.lines[5].spans[7].content.as_ref(), "Enter Use");
}

#[test]
fn footer_context_renders_configured_status_items() {
    let mut state = TuiState::default();
    state.status = "Ready".to_string();
    state.session.workspace_name = "nanoclaw".to_string();
    state.session.model = "gpt-5.4".to_string();
    state.session.model_reasoning_effort = Some("high".to_string());
    state.session.active_session_ref = "session_123456".to_string();
    state.session.git.available = true;
    state.session.git.repo_name = "nanoclaw-repo".to_string();
    state.session.git.branch = "main".to_string();
    state.session.statusline.clock = false;
    state.session.statusline.session = true;

    let footer = format_footer_context(&state);

    assert_eq!(footer.spans[0].content.as_ref(), "•");
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("Ready") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("nanoclaw") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("gpt-5.4 (high)") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("nanoclaw-repo") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("main") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("ctx --") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("in 0") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("out 0") })
    );
    assert!(
        footer
            .spans
            .iter()
            .all(|span| { !span.content.as_ref().contains("queue 0") })
    );
    assert!(
        footer
            .spans
            .iter()
            .any(|span| { span.content.as_ref().contains("sid session_123456") })
    );
}

#[test]
fn main_pane_viewport_height_does_not_reserve_top_header_space() {
    let state = TuiState::default();
    let area = Rect::new(0, 0, 120, 30);

    let viewport = main_pane_viewport_height(area, &state, None, None, None);

    assert_eq!(viewport, 30 - composer_height(&state, None) - 1);
}

#[test]
fn main_pane_viewport_height_does_not_shrink_for_approval_modal() {
    let state = TuiState::default();
    let area = Rect::new(0, 0, 120, 30);
    let approval = ApprovalPrompt {
        tool_name: "exec_command".to_string(),
        origin: ApprovalOrigin::Local,
        mode: None,
        working_directory: None,
        content: ApprovalContent {
            kind: ApprovalContentKind::Command,
            preview: vec!["$ cargo test".to_string()],
        },
        reasons: vec!["sandbox policy requires approval".to_string()],
    };

    let viewport = main_pane_viewport_height(area, &state, Some(&approval), None, None);

    assert_eq!(viewport, 30 - composer_height(&state, None) - 1);
}

#[test]
fn footer_context_window_includes_units_and_percent() {
    let mut state = TuiState::default();
    state.session.token_ledger.context_window = Some(agent::types::ContextWindowUsage {
        used_tokens: 30_000,
        max_tokens: 400_000,
    });

    let footer = format_footer_context(&state);

    assert!(
        footer
            .spans
            .iter()
            .any(|span| span.content.as_ref().contains("ctx 30k / 400k tok (7%)"))
    );
}

#[test]
fn toast_band_is_hidden_without_a_visible_toast() {
    let state = TuiState::default();

    assert_eq!(toast_height(&state), None);
    assert!(line_text_for(&format_toast_line(&state)).is_empty());
}

#[test]
fn toast_band_renders_tone_and_message_preview() {
    let mut state = TuiState::default();
    state.show_toast(
        ToastTone::Warning,
        "task task_123 failed · inspect with /task",
    );

    let line = format_toast_line(&state);
    let text = line_text_for(&line);

    assert_eq!(toast_height(&state), Some(1));
    assert!(text.contains("notice"));
    assert!(text.contains("task task_123 failed"));
    assert!(text.contains("/task"));
}

#[test]
fn composer_cursor_width_accounts_for_wide_characters() {
    assert_eq!(super::shared::composer_cursor_width("hello"), 5);
    assert_eq!(super::shared::composer_cursor_width("你好"), 4);
    assert_eq!(super::shared::composer_cursor_width("A你B"), 4);
}

#[test]
fn view_title_is_suppressed_when_the_collection_already_has_one() {
    assert!(!should_render_view_title(
        "Sessions",
        &[
            section_entry("Sessions"),
            shell_summary_entry("sess_123  prompt", Vec::new())
        ]
    ));
    assert!(should_render_view_title(
        "Export",
        &[section_entry("Session"), field_entry("path", "out.txt")]
    ));
}

fn section_entry(title: &str) -> InspectorEntry {
    InspectorEntry::section(title)
}

fn field_entry(key: &str, value: &str) -> InspectorEntry {
    InspectorEntry::field(key, value)
}

fn command_entry(command: &str) -> InspectorEntry {
    InspectorEntry::Command(command.to_string())
}

fn actionable_collection_entry(primary: &str, secondary: &str) -> InspectorEntry {
    InspectorEntry::actionable_collection(
        primary,
        Some(secondary),
        InspectorAction::RunCommand(primary.to_string()),
    )
}

fn shell_summary_entry(headline: &str, details: Vec<TranscriptShellDetail>) -> InspectorEntry {
    InspectorEntry::transcript(TranscriptEntry::shell_summary_details(headline, details))
}

fn success_summary_entry(headline: &str, details: Vec<TranscriptShellDetail>) -> InspectorEntry {
    InspectorEntry::transcript(TranscriptEntry::success_summary_details(headline, details))
}

fn raw_detail(text: &str) -> TranscriptShellDetail {
    TranscriptShellDetail::Raw {
        text: text.to_string(),
        continuation: false,
    }
}

fn continuation_detail(text: &str) -> TranscriptShellDetail {
    TranscriptShellDetail::Raw {
        text: text.to_string(),
        continuation: true,
    }
}
