use super::{
    ActiveToolCell, ComposerDraftAttachmentKind, ComposerDraftAttachmentState, ComposerDraftState,
    ComposerKillBufferState, ComposerRowAttachmentPreview, GitPorcelainEntry, GitPorcelainState,
    HistoryRollbackCandidate, InspectorAction, InspectorEntry, MainPaneMode, SharedUiState,
    ToastState, ToastTone, ToolSelectionTarget, TranscriptEntry, TranscriptToolEntry,
    TranscriptToolStatus, TuiState, composer_draft_from_messages, composer_draft_from_parts,
    draft_preview_text, git_snapshot, page_scroll_amount,
};
use crate::frontend::tui::input_history::{ComposerHistoryKind, PersistedComposerHistoryEntry};
use crate::theme::ThemeSummary;
use crate::tool_render::{
    ToolCommand, ToolCompletionState, ToolDetail, ToolDetailLabel, ToolReview, ToolReviewFile,
};
use agent::types::{
    Message, MessageId, MessagePart, MessageRole, SubmittedPromptAttachment,
    SubmittedPromptAttachmentKind, SubmittedPromptSnapshot,
};
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn git_snapshot_skips_host_process_when_disabled() {
    let dir = tempdir().unwrap();
    let snapshot = git_snapshot(dir.path(), false);

    assert!(!snapshot.available);
    assert!(snapshot.repo_name.is_empty());
    assert!(snapshot.branch.is_empty());
}

#[test]
fn git_porcelain_parser_distinguishes_branch_tracked_and_untracked_entries() {
    assert_eq!(
        GitPorcelainEntry::parse("## main...origin/main"),
        Some(GitPorcelainEntry::BranchHeader(
            "main...origin/main".to_string()
        ))
    );
    assert_eq!(
        GitPorcelainEntry::parse("M  src/lib.rs"),
        Some(GitPorcelainEntry::Tracked {
            index: GitPorcelainState::Changed,
            worktree: GitPorcelainState::Unmodified,
        })
    );
    assert_eq!(
        GitPorcelainEntry::parse(" M src/lib.rs"),
        Some(GitPorcelainEntry::Tracked {
            index: GitPorcelainState::Unmodified,
            worktree: GitPorcelainState::Changed,
        })
    );
    assert_eq!(
        GitPorcelainEntry::parse("?? scratch.txt"),
        Some(GitPorcelainEntry::Untracked)
    );
    assert_eq!(
        GitPorcelainEntry::parse("!! target/"),
        Some(GitPorcelainEntry::Ignored)
    );
}

#[test]
fn transcript_push_keeps_manual_scroll_position_until_follow_is_restored() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        follow_transcript: true,
        ..TuiState::default()
    };

    state.push_transcript("first");
    assert_eq!(state.transcript_scroll, u16::MAX);

    state.scroll_focused(-2);
    assert!(!state.follow_transcript);

    state.push_transcript("second");
    assert_eq!(state.transcript_scroll, u16::MAX.saturating_sub(2));

    state.scroll_focused_end();
    assert!(state.follow_transcript);

    state.push_transcript("third");
    assert_eq!(state.transcript_scroll, u16::MAX);
}

#[test]
fn transcript_push_merges_finished_exploration_entries() {
    let mut state = TuiState::default();

    assert_eq!(
        state.push_transcript(TranscriptEntry::tool_with_completion(
            TranscriptToolStatus::Finished,
            "exec_command",
            vec![command_tool_detail("rg shimmer_spans")],
            ToolCompletionState::Success,
        )),
        0
    );
    assert_eq!(
        state.push_transcript(TranscriptEntry::tool_with_completion(
            TranscriptToolStatus::Finished,
            "exec_command",
            vec![command_tool_detail("cat shimmer.rs")],
            ToolCompletionState::Success,
        )),
        0
    );
    assert_eq!(
        state.push_transcript(TranscriptEntry::tool_with_completion(
            TranscriptToolStatus::Finished,
            "exec_command",
            vec![command_tool_detail("cat status_indicator_widget.rs")],
            ToolCompletionState::Success,
        )),
        0
    );

    assert_eq!(state.transcript.len(), 1);
    assert_eq!(
        state.transcript[0].serialized(),
        "• Explored\n  └ Search shimmer_spans\n    Read shimmer.rs, status_indicator_widget.rs"
    );
}

#[test]
fn transcript_entry_from_string_round_trips_prefixed_summary_blocks() {
    let raw = "✔ Exported transcript\n  └ session session-1\n    wrote /tmp/out.txt".to_string();
    let entry = TranscriptEntry::from(raw.clone());

    assert_eq!(entry.serialized(), raw);
}

#[test]
fn code_diagnostics_tool_entry_uses_diagnostics_headline() {
    let notebook_entry = TranscriptEntry::tool_with_completion(
        TranscriptToolStatus::Finished,
        "notebook_read",
        vec![ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: "2 cell(s)".to_string(),
        }],
        ToolCompletionState::Success,
    );
    assert_eq!(
        notebook_entry.serialized(),
        "• Read notebook\n  └ Result 2 cell(s)"
    );

    let search_entry = TranscriptEntry::tool_with_completion(
        TranscriptToolStatus::Finished,
        "code_search",
        vec![ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: "3 match(es)".to_string(),
        }],
        ToolCompletionState::Success,
    );
    assert_eq!(
        search_entry.serialized(),
        "• Searched code\n  └ Result 3 match(es)"
    );

    let entry = TranscriptEntry::tool_with_completion(
        TranscriptToolStatus::Finished,
        "code_diagnostics",
        vec![ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: "2 diagnostic(s)".to_string(),
        }],
        ToolCompletionState::Success,
    );

    assert_eq!(
        entry.serialized(),
        "• Inspected diagnostics\n  └ Result 2 diagnostic(s)"
    );
}

#[test]
fn expired_toast_is_cleared_on_next_tick() {
    let mut state = TuiState::default();
    state.toast = Some(ToastState {
        message: "background wait done".to_string(),
        tone: ToastTone::Info,
        expires_at: Instant::now() - Duration::from_secs(1),
    });

    assert!(state.expire_toast_if_due());
    assert!(state.toast.is_none());
}

#[test]
fn transcript_home_disables_follow_until_end_is_requested() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        follow_transcript: true,
        ..TuiState::default()
    };

    state.scroll_focused_home();
    assert_eq!(state.transcript_scroll, 0);
    assert!(!state.follow_transcript);

    state.scroll_focused_end();
    assert_eq!(state.transcript_scroll, u16::MAX);
    assert!(state.follow_transcript);
}

#[test]
fn page_scroll_amount_keeps_overlap_and_supports_half_pages() {
    assert_eq!(page_scroll_amount(20, false), 18);
    assert_eq!(page_scroll_amount(20, true), 9);
    assert_eq!(page_scroll_amount(1, false), 1);
    assert_eq!(page_scroll_amount(2, true), 1);
}

#[test]
fn transcript_page_scroll_uses_viewport_height() {
    let mut state = TuiState {
        main_pane: MainPaneMode::Transcript,
        follow_transcript: true,
        transcript_scroll: 40,
        ..TuiState::default()
    };

    state.scroll_focused_page(20, true, true);
    assert_eq!(state.transcript_scroll, 31);
    assert!(!state.follow_transcript);

    state.scroll_focused_page(20, false, false);
    assert_eq!(state.transcript_scroll, 49);
}

#[test]
fn history_rollback_overlay_opens_on_latest_candidate_and_wraps_navigation() {
    let mut state = TuiState::default();
    let candidates = vec![
        HistoryRollbackCandidate {
            message_id: MessageId::from("msg-1"),
            prompt: "first".to_string(),
            draft: ComposerDraftState::from_text("first"),
            turn_preview_lines: vec!["› first".into()],
            removed_turn_count: 2,
            removed_message_count: 4,
        },
        HistoryRollbackCandidate {
            message_id: MessageId::from("msg-2"),
            prompt: "second".to_string(),
            draft: ComposerDraftState::from_text("second"),
            turn_preview_lines: vec!["› second".into()],
            removed_turn_count: 1,
            removed_message_count: 2,
        },
    ];

    assert!(state.open_history_rollback_overlay(candidates));
    assert_eq!(
        state
            .selected_history_rollback_candidate()
            .map(|candidate| candidate.prompt.as_str()),
        Some("second")
    );

    assert!(state.move_history_rollback_selection(true));
    assert_eq!(
        state
            .selected_history_rollback_candidate()
            .map(|candidate| candidate.prompt.as_str()),
        Some("first")
    );

    assert!(state.move_history_rollback_selection(true));
    assert_eq!(
        state
            .selected_history_rollback_candidate()
            .map(|candidate| candidate.prompt.as_str()),
        Some("second")
    );
}

#[test]
fn transcript_selection_moves_between_tool_entries_only() {
    let mut state = TuiState::default();
    state.transcript = vec![
        TranscriptEntry::AssistantMessage("assistant".to_string()),
        TranscriptEntry::tool(
            TranscriptToolStatus::Running,
            "exec_command",
            vec![command_tool_detail("cargo test")],
        ),
        TranscriptEntry::UserPrompt("prompt".to_string()),
        TranscriptEntry::tool(
            TranscriptToolStatus::Finished,
            "write",
            vec![ToolDetail::LabeledValue {
                label: ToolDetailLabel::Effect,
                value: "Updated src/lib.rs".to_string(),
            }],
        ),
    ];

    assert!(state.move_tool_selection(false));
    assert_eq!(
        state.tool_selection,
        Some(ToolSelectionTarget::Transcript(1))
    );

    assert!(state.move_tool_selection(false));
    assert_eq!(
        state.tool_selection,
        Some(ToolSelectionTarget::Transcript(3))
    );

    assert!(state.move_tool_selection(false));
    assert_eq!(
        state.tool_selection,
        Some(ToolSelectionTarget::Transcript(1))
    );
}

#[test]
fn selected_tool_review_overlay_uses_review_from_selected_entry() {
    let mut state = TuiState {
        tool_selection: Some(ToolSelectionTarget::Transcript(0)),
        ..TuiState::default()
    };
    state.transcript = vec![TranscriptEntry::tool_with_review(
        TranscriptToolStatus::Finished,
        "write",
        vec![ToolDetail::LabeledValue {
            label: ToolDetailLabel::Effect,
            value: "Updated src/lib.rs".to_string(),
        }],
        Some(ToolReview {
            summary: Some("Updated src/lib.rs".to_string()),
            files: vec![ToolReviewFile {
                path: "src/lib.rs".to_string(),
                preview_lines: vec!["+new()".to_string()],
            }],
        }),
    )];

    assert!(state.open_selected_tool_review_overlay());
    assert_eq!(
        state
            .selected_tool_review_file()
            .map(|file| file.path.as_str()),
        Some("src/lib.rs")
    );
}

#[test]
fn tool_selection_cycles_across_committed_and_live_tools() {
    let mut state = TuiState::default();
    state.transcript = vec![TranscriptEntry::tool(
        TranscriptToolStatus::Finished,
        "write",
        vec![ToolDetail::LabeledValue {
            label: ToolDetailLabel::Effect,
            value: "Updated src/lib.rs".to_string(),
        }],
    )];
    state.active_tool_cells = vec![ActiveToolCell::new(
        "call-1",
        TranscriptToolEntry::new(
            TranscriptToolStatus::Running,
            "exec_command",
            vec![command_tool_detail("cargo test")],
        ),
    )];

    assert!(state.move_tool_selection(false));
    assert_eq!(
        state.tool_selection,
        Some(ToolSelectionTarget::Transcript(0))
    );

    assert!(state.move_tool_selection(false));
    assert_eq!(
        state.tool_selection,
        Some(ToolSelectionTarget::LiveCell("call-1".to_string()))
    );
}

fn command_tool_detail(command: &str) -> ToolDetail {
    ToolDetail::Command(ToolCommand::from_preview(&format!("$ {command}")))
}

#[test]
fn selected_tool_review_overlay_can_open_from_live_tool_selection() {
    let mut state = TuiState {
        tool_selection: Some(ToolSelectionTarget::LiveCell("call-1".to_string())),
        ..TuiState::default()
    };
    state.active_tool_cells = vec![ActiveToolCell::new(
        "call-1",
        TranscriptToolEntry::new_with_review(
            TranscriptToolStatus::Running,
            "write",
            vec![ToolDetail::LabeledValue {
                label: ToolDetailLabel::Effect,
                value: "Updating src/lib.rs".to_string(),
            }],
            Some(ToolReview {
                summary: Some("Updating src/lib.rs".to_string()),
                files: vec![ToolReviewFile {
                    path: "src/lib.rs".to_string(),
                    preview_lines: vec!["+new()".to_string()],
                }],
            }),
        ),
    )];

    assert!(state.open_selected_tool_review_overlay());
    assert_eq!(
        state
            .selected_tool_review_file()
            .map(|file| file.path.as_str()),
        Some("src/lib.rs")
    );
}

#[test]
fn composer_input_history_recalls_entries_and_restores_draft() {
    let mut state = TuiState {
        input: "current draft".to_string(),
        input_cursor: "current draft".len(),
        input_history: vec![
            SubmittedPromptSnapshot::from_text("first prompt"),
            SubmittedPromptSnapshot::from_text("second prompt"),
        ],
        ..TuiState::default()
    };

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "second prompt");
    assert_eq!(state.input_cursor(), "second prompt".len());

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "first prompt");

    assert!(state.browse_input_history(false));
    assert_eq!(state.input, "second prompt");

    assert!(state.browse_input_history(false));
    assert_eq!(state.input, "current draft");
    assert_eq!(state.input_cursor(), "current draft".len());

    assert!(!state.browse_input_history(false));
}

#[test]
fn open_theme_picker_tracks_original_theme_for_restore() {
    let mut state = TuiState {
        theme: "fjord".to_string(),
        themes: vec![
            ThemeSummary {
                id: "graphite".to_string(),
                summary: "dark slate".to_string(),
            },
            ThemeSummary {
                id: "fjord".to_string(),
                summary: "deep blue".to_string(),
            },
        ],
        ..TuiState::default()
    };

    state.open_theme_picker();

    let picker = state.theme_picker.as_ref().unwrap();
    assert_eq!(picker.selected, 1);
    assert_eq!(picker.original_theme, "fjord");
    assert_eq!(state.original_theme().as_deref(), Some("fjord"));
}

#[test]
fn show_main_view_selects_the_first_actionable_collection_item() {
    let mut state = TuiState::default();
    state.show_main_view(
        "Sessions",
        [
            InspectorEntry::section("Sessions"),
            InspectorEntry::actionable_collection(
                "session_1  first",
                Some("open the first session"),
                InspectorAction::RunCommand("/session session_1".to_string()),
            ),
            InspectorEntry::actionable_collection(
                "session_2  second",
                Some("open the second session"),
                InspectorAction::RunCommand("/session session_2".to_string()),
            ),
        ],
    );

    assert_eq!(
        state
            .collection_picker
            .as_ref()
            .map(|picker| picker.selected),
        Some(0)
    );
    assert!(matches!(
        state.selected_collection_entry(),
        Some(InspectorEntry::CollectionItem { primary, .. }) if primary == "session_1  first"
    ));

    assert!(state.move_collection_picker(false));
    assert!(matches!(
        state.selected_collection_entry(),
        Some(InspectorEntry::CollectionItem { primary, .. }) if primary == "session_2  second"
    ));
}

#[test]
fn editing_after_history_recall_resets_navigation_cursor() {
    let mut state = TuiState {
        input_history: vec![
            SubmittedPromptSnapshot::from_text("prompt one"),
            SubmittedPromptSnapshot::from_text("prompt two"),
        ],
        ..TuiState::default()
    };

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "prompt two");

    state.push_input_char('!');
    assert_eq!(state.input, "prompt two!");

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "prompt two");
}

#[test]
fn history_recall_requires_cursor_at_buffer_boundary() {
    let mut state = TuiState {
        input: "current draft".to_string(),
        input_cursor: 4,
        input_history: vec![
            SubmittedPromptSnapshot::from_text("first prompt"),
            SubmittedPromptSnapshot::from_text("second prompt"),
        ],
        ..TuiState::default()
    };

    assert!(!state.browse_input_history(true));

    assert!(state.move_input_cursor_end());
    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "second prompt");
}

#[test]
fn local_history_overrides_persistent_suffix_with_richer_drafts() {
    let mut state = TuiState {
        input_history: vec![
            SubmittedPromptSnapshot::from_text("older"),
            SubmittedPromptSnapshot::from_text("recent"),
        ],
        local_input_history: vec![ComposerDraftState {
            text: "recent".to_string(),
            cursor: 2,
            draft_attachments: Vec::new(),
        }],
        ..TuiState::default()
    };

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "recent");
    assert_eq!(state.input_cursor(), 2);

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "older");
}

#[test]
fn slash_input_history_recalls_command_entries_without_mixing_prompt_history() {
    let mut state = TuiState {
        input: "/".to_string(),
        input_cursor: 1,
        input_history: vec![SubmittedPromptSnapshot::from_text("regular prompt")],
        command_history: vec![
            SubmittedPromptSnapshot::from_text("/help"),
            SubmittedPromptSnapshot::from_text("/sessions recent"),
        ],
        ..TuiState::default()
    };

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "/sessions recent");

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "/help");
}

#[test]
fn empty_input_history_prefers_prompt_entries_over_command_history() {
    let mut state = TuiState {
        input_history: vec![
            SubmittedPromptSnapshot::from_text("first prompt"),
            SubmittedPromptSnapshot::from_text("second prompt"),
        ],
        command_history: vec![SubmittedPromptSnapshot::from_text("/help")],
        ..TuiState::default()
    };

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "second prompt");
}

#[test]
fn non_slash_history_recall_uses_persisted_cross_type_order() {
    let mut state = TuiState {
        persisted_history_entries: vec![
            PersistedComposerHistoryEntry {
                kind: ComposerHistoryKind::Prompt,
                prompt: SubmittedPromptSnapshot::from_text("first prompt"),
            },
            PersistedComposerHistoryEntry {
                kind: ComposerHistoryKind::Command,
                prompt: SubmittedPromptSnapshot::from_text("/help"),
            },
            PersistedComposerHistoryEntry {
                kind: ComposerHistoryKind::Prompt,
                prompt: SubmittedPromptSnapshot::from_text("second prompt"),
            },
        ],
        ..TuiState::default()
    };

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "second prompt");

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "/help");

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "first prompt");
}

#[test]
fn inserting_and_deleting_respect_the_cursor_position() {
    let mut state = TuiState {
        input: "helo".to_string(),
        input_cursor: 3,
        ..TuiState::default()
    };

    state.push_input_char('l');
    assert_eq!(state.input, "hello");
    assert_eq!(state.input_cursor(), 4);

    state.pop_input_char();
    assert_eq!(state.input, "helo");
    assert_eq!(state.input_cursor(), 3);
}

#[test]
fn inserting_a_pasted_string_respects_the_cursor_position() {
    let mut state = TuiState {
        input: "helo".to_string(),
        input_cursor: 2,
        ..TuiState::default()
    };

    state.push_input_str("l\n");
    assert_eq!(state.input, "hel\nlo");
    assert_eq!(state.input_cursor(), 4);
}

#[test]
fn vertical_cursor_motion_moves_between_lines_without_triggering_history() {
    let mut state = TuiState {
        input: "alpha\nbeta\ngamma".to_string(),
        input_cursor: "alpha\nbe".len(),
        ..TuiState::default()
    };

    assert!(state.move_input_cursor_vertical(false));
    assert_eq!(state.input_cursor(), "alpha\nbeta\nga".len());

    assert!(state.move_input_cursor_vertical(true));
    assert_eq!(state.input_cursor(), "alpha\nbe".len());
}

#[test]
fn vertical_cursor_motion_keeps_the_desired_column_across_short_lines() {
    let mut state = TuiState {
        input: "wide line\nx\nwide tail".to_string(),
        input_cursor: "wide li".len(),
        ..TuiState::default()
    };

    assert!(state.move_input_cursor_vertical(false));
    assert_eq!(state.input_cursor(), "wide line\nx".len());

    assert!(state.move_input_cursor_vertical(false));
    assert_eq!(state.input_cursor(), "wide line\nx\nwide ta".len());
}

#[test]
fn stashing_current_input_draft_preserves_exact_text_and_cursor() {
    let mut state = TuiState {
        input: "draft  \nline two".to_string(),
        input_cursor: 7,
        ..TuiState::default()
    };

    assert!(state.stash_current_input_draft());
    assert_eq!(
        state.local_input_history,
        vec![ComposerDraftState {
            text: "draft  \nline two".to_string(),
            cursor: 7,
            draft_attachments: Vec::new(),
        }]
    );
}

#[test]
fn stashed_draft_can_be_recalled_with_up_after_clearing_input() {
    let mut state = TuiState {
        input: "draft  \nline two".to_string(),
        input_cursor: 7,
        ..TuiState::default()
    };

    assert!(state.stash_current_input_draft());
    state.clear_input();

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "draft  \nline two");
    assert_eq!(state.input_cursor(), 7);
}

#[test]
fn large_paste_creates_placeholder_and_tracks_payload() {
    let mut state = TuiState::default();

    let placeholder = state.push_large_paste("pasted body");

    assert_eq!(placeholder, "[Paste #1]");
    assert_eq!(state.input, "[Paste #1]");
    assert_eq!(
        state.draft_attachments,
        vec![large_paste_attachment("pasted body")]
    );
}

#[test]
fn take_input_expands_pending_paste_placeholders() {
    let ui_state = SharedUiState::new();
    ui_state.mutate(|state| {
        let _ = state.push_large_paste("pasted body");
    });

    assert_eq!(ui_state.take_input(), "pasted body");

    let snapshot = ui_state.snapshot();
    assert!(snapshot.draft_attachments.is_empty());
    assert!(snapshot.input.is_empty());
}

#[test]
fn take_submission_keeps_large_paste_as_typed_part_and_preserves_local_draft() {
    let mut state = TuiState::default();
    state.push_input_str("prefix ");
    let _ = state.push_large_paste("pasted body");
    state.push_input_str(" suffix");

    let submission = state.take_submission();

    assert_eq!(
        submission.prompt_snapshot,
        SubmittedPromptSnapshot {
            text: "prefix [Paste #1] suffix".to_string(),
            attachments: vec![SubmittedPromptAttachment {
                placeholder: Some("[Paste #1]".to_string()),
                kind: SubmittedPromptAttachmentKind::Paste {
                    text: "pasted body".to_string(),
                },
            }],
        }
    );
    assert_eq!(
        submission.local_history_draft,
        ComposerDraftState {
            text: "prefix [Paste #1] suffix".to_string(),
            cursor: "prefix [Paste #1] suffix".len(),
            draft_attachments: vec![large_paste_attachment("pasted body")],
        }
    );
}

#[test]
fn take_submission_keeps_local_attachment_placeholders_as_first_class_parts() {
    let mut state = TuiState::default();
    assert!(state.push_inline_attachment(local_image_attachment("artifacts/failure.png")));
    state.push_input_str(" ");
    assert!(state.push_inline_attachment(local_file_attachment("reports/run.pdf")));
    state.push_input_str("\ndescribe the failure");

    let submission = state.take_submission();

    assert_eq!(
        submission.prompt_snapshot,
        SubmittedPromptSnapshot {
            text: "[Image #1] [File #1]\ndescribe the failure".to_string(),
            attachments: vec![
                SubmittedPromptAttachment {
                    placeholder: Some("[Image #1]".to_string()),
                    kind: SubmittedPromptAttachmentKind::LocalImage {
                        requested_path: "artifacts/failure.png".to_string(),
                        mime_type: Some("image/png".to_string()),
                    },
                },
                SubmittedPromptAttachment {
                    placeholder: Some("[File #1]".to_string()),
                    kind: SubmittedPromptAttachmentKind::LocalFile {
                        requested_path: "reports/run.pdf".to_string(),
                        file_name: Some("run.pdf".to_string()),
                        mime_type: Some("application/pdf".to_string()),
                    },
                },
            ],
        }
    );
    assert_eq!(
        submission.local_history_draft,
        ComposerDraftState {
            text: "[Image #1] [File #1]\ndescribe the failure".to_string(),
            cursor: "[Image #1] [File #1]\ndescribe the failure".len(),
            draft_attachments: vec![
                local_image_attachment("artifacts/failure.png"),
                local_file_attachment("reports/run.pdf"),
            ],
        }
    );
}

#[test]
fn take_submission_keeps_remote_row_attachments_as_first_class_parts() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(remote_image_attachment(
        "https://example.com/assets/failure.png"
    )));
    assert!(state.push_row_attachment(remote_file_attachment(
        "https://example.com/reports/run.pdf"
    )));
    state.push_input_str("summarize the remote artifacts");

    let submission = state.take_submission();

    assert_eq!(
        submission.prompt_snapshot,
        SubmittedPromptSnapshot {
            text: "summarize the remote artifacts".to_string(),
            attachments: vec![
                SubmittedPromptAttachment {
                    placeholder: None,
                    kind: SubmittedPromptAttachmentKind::RemoteImage {
                        requested_url: "https://example.com/assets/failure.png".to_string(),
                        mime_type: Some("image/png".to_string()),
                    },
                },
                SubmittedPromptAttachment {
                    placeholder: None,
                    kind: SubmittedPromptAttachmentKind::RemoteFile {
                        requested_url: "https://example.com/reports/run.pdf".to_string(),
                        file_name: Some("run.pdf".to_string()),
                        mime_type: Some("application/pdf".to_string()),
                    },
                },
            ],
        }
    );
    assert_eq!(
        submission.local_history_draft,
        ComposerDraftState {
            text: "summarize the remote artifacts".to_string(),
            cursor: "summarize the remote artifacts".len(),
            draft_attachments: vec![
                remote_image_attachment("https://example.com/assets/failure.png"),
                remote_file_attachment("https://example.com/reports/run.pdf"),
            ],
        }
    );
}

#[test]
fn row_attachment_summaries_list_only_visible_attachment_rows() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    let _ = state.push_large_paste("pasted body");
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));

    assert_eq!(
        state.row_attachment_summaries(),
        vec![
            (
                1,
                "image · failure.png".to_string(),
                "artifacts/failure.png".to_string(),
            ),
            (
                2,
                "file · run.pdf".to_string(),
                "reports/run.pdf".to_string(),
            ),
        ]
    );
}

#[test]
fn row_attachment_summaries_keep_remote_urls_visible() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(remote_image_attachment(
        "https://example.com/assets/failure.png"
    )));
    assert!(state.push_row_attachment(remote_file_attachment(
        "https://example.com/reports/run.pdf"
    )));

    assert_eq!(
        state.row_attachment_summaries(),
        vec![
            (
                1,
                "image · failure.png".to_string(),
                "https://example.com/assets/failure.png".to_string(),
            ),
            (
                2,
                "file · run.pdf".to_string(),
                "https://example.com/reports/run.pdf".to_string(),
            ),
        ]
    );
}

#[test]
fn remove_row_attachment_defaults_to_latest_visible_row() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));

    let removed = state.remove_row_attachment(None);

    assert_eq!(removed, Some(local_file_row_attachment("reports/run.pdf")));
    assert_eq!(
        state.row_attachment_summaries(),
        vec![(
            1,
            "image · failure.png".to_string(),
            "artifacts/failure.png".to_string(),
        )]
    );
}

#[test]
fn move_row_attachment_reorders_visible_rows() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));

    assert!(state.move_row_attachment(2, 1));
    assert_eq!(
        state.row_attachment_summaries(),
        vec![
            (
                1,
                "file · run.pdf".to_string(),
                "reports/run.pdf".to_string(),
            ),
            (
                2,
                "image · failure.png".to_string(),
                "artifacts/failure.png".to_string(),
            ),
        ]
    );
    assert_eq!(
        state.selected_row_attachment_summary(),
        Some((
            1,
            "file · run.pdf".to_string(),
            "reports/run.pdf".to_string(),
        ))
    );
}

#[test]
fn row_attachment_selection_moves_and_deletes_rows() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(remote_image_attachment(
        "https://example.com/assets/failure.png"
    )));
    assert!(state.push_row_attachment(remote_file_attachment(
        "https://example.com/reports/run.pdf"
    )));

    assert!(state.select_previous_row_attachment());
    assert_eq!(
        state.selected_row_attachment_summary(),
        Some((
            2,
            "file · run.pdf".to_string(),
            "https://example.com/reports/run.pdf".to_string(),
        ))
    );

    let removed = state.remove_selected_row_attachment();
    assert_eq!(
        removed,
        Some(remote_file_attachment(
            "https://example.com/reports/run.pdf"
        ))
    );
    assert_eq!(
        state.selected_row_attachment_summary(),
        Some((
            1,
            "image · failure.png".to_string(),
            "https://example.com/assets/failure.png".to_string(),
        ))
    );
}

#[test]
fn stash_current_input_draft_keeps_attachment_only_drafts() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(remote_image_attachment(
        "https://example.com/assets/failure.png"
    )));

    assert!(state.stash_current_input_draft());
    state.clear_input();

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "");
    assert_eq!(
        state.draft_attachments,
        vec![remote_image_attachment(
            "https://example.com/assets/failure.png"
        )]
    );
}

#[test]
fn apply_external_edit_rebases_large_paste_placeholders_by_text_order() {
    let mut state = TuiState::default();
    let first = state.push_large_paste("first payload");
    state.push_input_str(" and ");
    let second = state.push_large_paste("second payload");

    state.apply_external_edit(format!("{second} before {first}"));

    assert_eq!(state.input, "[Paste #1] before [Paste #2]");
    assert_eq!(
        state.draft_attachments,
        vec![
            ComposerDraftAttachmentState {
                placeholder: Some("[Paste #1]".to_string()),
                kind: ComposerDraftAttachmentKind::LargePaste {
                    payload: "second payload".to_string(),
                },
            },
            ComposerDraftAttachmentState {
                placeholder: Some("[Paste #2]".to_string()),
                kind: ComposerDraftAttachmentKind::LargePaste {
                    payload: "first payload".to_string(),
                },
            },
        ]
    );
}

#[test]
fn apply_external_edit_drops_missing_inline_attachments_and_keeps_rows() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));
    let placeholder = state.push_large_paste("pasted body");

    state.apply_external_edit(format!("keep rows but not {placeholder}"));
    state.apply_external_edit("keep rows only");

    assert_eq!(state.input, "keep rows only");
    assert_eq!(
        state.draft_attachments,
        vec![local_file_row_attachment("reports/run.pdf")]
    );
}

#[test]
fn external_editor_seed_text_surfaces_attachment_section_before_prompt() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    assert!(state.push_row_attachment(remote_file_attachment(
        "https://example.com/reports/run.pdf"
    )));
    state.push_input_str("summarize the artifacts");

    assert_eq!(
        state.external_editor_seed_text(),
        "[Attachments]\n[Image #1] artifacts/failure.png\n[File #2] https://example.com/reports/run.pdf\n\n[Prompt]\nsummarize the artifacts"
    );
}

#[test]
fn apply_external_edit_reorders_and_drops_rows_from_attachment_section() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));
    state.push_input_str("summarize the artifacts");

    let summary = state.apply_external_edit(
        "[Attachments]\n[File #2] reports/run.pdf\n\n[Prompt]\nupdated prompt".to_string(),
    );

    assert_eq!(state.input, "updated prompt");
    assert_eq!(
        state.draft_attachments,
        vec![local_file_row_attachment("reports/run.pdf")]
    );
    assert_eq!(
        summary.detached,
        vec![ComposerRowAttachmentPreview {
            index: 1,
            summary: "image · failure.png".to_string(),
            detail: "artifacts/failure.png".to_string(),
        }]
    );
    assert!(!summary.reordered);
}

#[test]
fn apply_external_edit_can_clear_all_rows_from_attachment_section() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));

    let summary = state.apply_external_edit("[Attachments]\n\n[Prompt]\ntext only".to_string());

    assert_eq!(state.input, "text only");
    assert!(state.draft_attachments.is_empty());
    assert_eq!(summary.detached.len(), 2);
    assert!(!summary.reordered);
}

#[test]
fn apply_external_edit_reports_reordered_rows() {
    let mut state = TuiState::default();
    assert!(state.push_row_attachment(local_image_row_attachment("artifacts/failure.png")));
    assert!(state.push_row_attachment(local_file_row_attachment("reports/run.pdf")));
    assert!(state.push_row_attachment(remote_image_attachment(
        "https://example.com/assets/overview.png",
    )));

    let summary = state.apply_external_edit(
        "[Attachments]\n[Image #3] https://example.com/assets/overview.png\n[Image #1] artifacts/failure.png\n[File #2] reports/run.pdf\n\n[Prompt]\ntext only"
            .to_string(),
    );

    assert!(summary.detached.is_empty());
    assert!(summary.reordered);
}

#[test]
fn composer_draft_from_messages_keeps_text_breaks_and_row_attachments() {
    let draft = composer_draft_from_messages(&[
        Message::new(
            MessageRole::User,
            vec![MessagePart::ImageUrl {
                url: "https://example.com/failure.png".to_string(),
                mime_type: Some("image/png".to_string()),
            }],
        ),
        Message::new(
            MessageRole::User,
            vec![MessagePart::inline_text("summarize the failure")],
        ),
    ]);

    assert_eq!(draft.text, "summarize the failure");
    assert_eq!(
        draft.draft_attachments,
        vec![remote_image_attachment("https://example.com/failure.png")]
    );
}

#[test]
fn composer_draft_from_parts_restores_inline_paste_and_local_attachment_placeholders() {
    let draft = composer_draft_from_parts(&[
        MessagePart::File {
            file_name: Some("run.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
            data_base64: Some("pdf-data".to_string()),
            uri: Some("reports/run.pdf".to_string()),
        },
        MessagePart::inline_text(" review "),
        MessagePart::paste("[Paste #1]", "body"),
    ]);

    assert_eq!(draft.text, "[File #1] review [Paste #1]");
    assert_eq!(
        draft.draft_attachments,
        vec![
            local_file_attachment("reports/run.pdf"),
            large_paste_attachment("body")
        ]
    );
}

#[test]
fn draft_preview_text_prefers_attachment_summaries_over_raw_markers() {
    let draft = ComposerDraftState {
        text: "summarize the artifact".to_string(),
        cursor: "summarize the artifact".len(),
        draft_attachments: vec![remote_image_attachment(
            "https://example.com/assets/failure.png",
        )],
    };

    assert_eq!(
        draft_preview_text(
            &draft,
            "[image_url:https://example.com/assets/failure.png image/png]",
            80,
        ),
        "#1 image · failure.png · summarize the artifact"
    );
}

#[test]
fn stashed_draft_recall_restores_pending_paste_payloads() {
    let mut state = TuiState::default();
    let _ = state.push_large_paste("pasted body");

    assert!(state.stash_current_input_draft());
    state.clear_input();

    assert!(state.browse_input_history(true));
    assert_eq!(state.input, "[Paste #1]");
    assert_eq!(
        state.draft_attachments,
        vec![large_paste_attachment("pasted body")]
    );
}

#[test]
fn ctrl_k_kills_the_tail_and_tracks_large_paste_payloads() {
    let mut state = TuiState {
        input: "prefix [Paste #1] suffix".to_string(),
        input_cursor: 7,
        draft_attachments: vec![large_paste_attachment("pasted body")],
        ..TuiState::default()
    };

    assert!(state.kill_input_to_end());
    assert_eq!(state.input, "prefix ");
    assert!(state.draft_attachments.is_empty());
    assert_eq!(
        state.kill_buffer,
        Some(ComposerKillBufferState {
            text: "[Paste #1] suffix".to_string(),
            draft_attachments: vec![large_paste_attachment("pasted body")],
        })
    );
}

#[test]
fn ctrl_y_reinserts_the_latest_killed_tail_with_payloads() {
    let mut state = TuiState {
        input: "prefix [Paste #1] suffix".to_string(),
        input_cursor: 7,
        draft_attachments: vec![large_paste_attachment("pasted body")],
        ..TuiState::default()
    };

    assert!(state.kill_input_to_end());
    assert!(state.yank_kill_buffer());
    assert_eq!(state.input, "prefix [Paste #1] suffix");
    assert_eq!(
        state.draft_attachments,
        vec![large_paste_attachment("pasted body")]
    );
}

#[test]
fn kill_buffer_survives_submission_clears_and_expands_draft_attachments_on_yank() {
    let ui_state = SharedUiState::new();
    ui_state.mutate(|state| {
        state.replace_input("prefix [Paste #1]");
        state
            .draft_attachments
            .push(large_paste_attachment("pasted body"));
        state.input_cursor = "prefix ".len();
        let _ = state.kill_input_to_end();
    });

    assert_eq!(ui_state.take_input(), "prefix ");

    ui_state.mutate(|state| {
        assert!(state.yank_kill_buffer());
    });
    assert_eq!(ui_state.take_input(), "pasted body");
}

fn large_paste_attachment(payload: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: Some("[Paste #1]".to_string()),
        kind: ComposerDraftAttachmentKind::LargePaste {
            payload: payload.to_string(),
        },
    }
}

fn local_image_attachment(path: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: Some("[Image #1]".to_string()),
        kind: ComposerDraftAttachmentKind::LocalImage {
            requested_path: path.to_string(),
            mime_type: Some("image/png".to_string()),
            part: Some(MessagePart::Image {
                mime_type: "image/png".to_string(),
                data_base64: "png-data".to_string(),
            }),
        },
    }
}

fn local_file_attachment(path: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: Some("[File #1]".to_string()),
        kind: ComposerDraftAttachmentKind::LocalFile {
            requested_path: path.to_string(),
            file_name: Some("run.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
            part: Some(MessagePart::File {
                file_name: Some("run.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                data_base64: Some("pdf-data".to_string()),
                uri: Some(path.to_string()),
            }),
        },
    }
}

fn local_image_row_attachment(path: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: None,
        kind: ComposerDraftAttachmentKind::LocalImage {
            requested_path: path.to_string(),
            mime_type: Some("image/png".to_string()),
            part: Some(MessagePart::Image {
                mime_type: "image/png".to_string(),
                data_base64: "png-data".to_string(),
            }),
        },
    }
}

fn local_file_row_attachment(path: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: None,
        kind: ComposerDraftAttachmentKind::LocalFile {
            requested_path: path.to_string(),
            file_name: Some("run.pdf".to_string()),
            mime_type: Some("application/pdf".to_string()),
            part: Some(MessagePart::File {
                file_name: Some("run.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                data_base64: Some("pdf-data".to_string()),
                uri: Some(path.to_string()),
            }),
        },
    }
}

fn remote_image_attachment(url: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: None,
        kind: ComposerDraftAttachmentKind::RemoteImage {
            requested_url: url.to_string(),
            part: MessagePart::ImageUrl {
                url: url.to_string(),
                mime_type: Some("image/png".to_string()),
            },
        },
    }
}

fn remote_file_attachment(url: &str) -> ComposerDraftAttachmentState {
    ComposerDraftAttachmentState {
        placeholder: None,
        kind: ComposerDraftAttachmentKind::RemoteFile {
            requested_url: url.to_string(),
            part: MessagePart::File {
                file_name: Some("run.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                data_base64: None,
                uri: Some(url.to_string()),
            },
        },
    }
}
