use crate::interaction::{PendingControlKind, PendingControlSummary, SessionPermissionMode};
use crate::statusline::{StatusLineConfig, StatusLineField, status_line_fields};
use crate::theme::ThemeSummary;
use crate::tool_render::{
    ToolDetail, ToolDetailBlockKind, preview_tool_details, serialize_tool_details,
};
use crate::ui::StartupDiagnosticsSnapshot;
use agent::types::{AgentStatus, SubmittedPromptSnapshot, TokenLedgerSnapshot};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

mod composer;
mod picker;
mod transcript;

pub(crate) use composer::{
    ComposerAttachmentEditSummary, ComposerDraftAttachmentKind, ComposerDraftAttachmentState,
    ComposerDraftState, ComposerHistoryNavigationState, ComposerKillBufferState,
    ComposerRowAttachmentPreview, ComposerSubmission, composer_draft_from_message,
    composer_draft_from_messages, composer_draft_from_parts, draft_preview_text,
};
pub(crate) use picker::{
    HistoryRollbackCandidate, HistoryRollbackState, PendingControlEditorState,
    PendingControlPickerState, StatusLinePickerState, ThemePickerState, ThinkingEffortPickerState,
};
pub(crate) use transcript::{
    InspectorEntry, TranscriptEntry, TranscriptExecutionEntry, TranscriptPlanEntry,
    TranscriptShellBlockKind, TranscriptShellDetail, TranscriptShellEntry, TranscriptToolEntry,
    TranscriptToolStatus,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct GitSnapshot {
    pub(crate) available: bool,
    pub(crate) repo_name: String,
    pub(crate) branch: String,
    pub(crate) staged: usize,
    pub(crate) modified: usize,
    pub(crate) untracked: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionSummary {
    pub(crate) workspace_name: String,
    pub(crate) active_session_ref: String,
    pub(crate) root_agent_session_id: String,
    pub(crate) provider_label: String,
    pub(crate) model: String,
    pub(crate) model_reasoning_effort: Option<String>,
    pub(crate) supported_model_reasoning_efforts: Vec<String>,
    pub(crate) supports_image_input: bool,
    pub(crate) workspace_root: PathBuf,
    pub(crate) git: GitSnapshot,
    pub(crate) tool_names: Vec<String>,
    pub(crate) store_label: String,
    pub(crate) store_warning: Option<String>,
    pub(crate) stored_session_count: usize,
    pub(crate) default_sandbox_summary: String,
    pub(crate) sandbox_summary: String,
    pub(crate) permission_mode: SessionPermissionMode,
    pub(crate) host_process_surfaces_allowed: bool,
    pub(crate) startup_diagnostics: StartupDiagnosticsSnapshot,
    pub(crate) queued_commands: usize,
    pub(crate) token_ledger: TokenLedgerSnapshot,
    pub(crate) statusline: StatusLineConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MainPaneMode {
    #[default]
    Transcript,
    View,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlanEntry {
    pub(crate) id: String,
    pub(crate) content: String,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExecutionEntry {
    pub(crate) scope_label: String,
    pub(crate) status: String,
    pub(crate) summary: String,
    pub(crate) next_action: Option<String>,
    pub(crate) verification: Option<String>,
    pub(crate) blocker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposerContextHint {
    LiveTaskFinished {
        task_id: String,
        status: AgentStatus,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ToastTone {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToastState {
    pub(crate) message: String,
    pub(crate) tone: ToastTone,
    pub(crate) expires_at: Instant,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TuiState {
    pub(crate) session: SessionSummary,
    pub(crate) theme: String,
    pub(crate) themes: Vec<ThemeSummary>,
    pub(crate) main_pane: MainPaneMode,
    pub(crate) show_tool_details: bool,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) input_vertical_column: Option<usize>,
    pub(crate) draft_attachments: Vec<ComposerDraftAttachmentState>,
    pub(crate) selected_row_attachment: Option<usize>,
    // Keep kill/yank state outside the visible draft so Ctrl+Y can restore the
    // latest killed tail even after submit/clear transitions.
    pub(crate) kill_buffer: Option<ComposerKillBufferState>,
    pub(crate) input_history: Vec<SubmittedPromptSnapshot>,
    pub(crate) local_input_history: Vec<ComposerDraftState>,
    pub(crate) input_history_navigation: Option<ComposerHistoryNavigationState>,
    pub(crate) command_completion_index: usize,
    pub(crate) composer_context_hint: Option<ComposerContextHint>,
    pub(crate) toast: Option<ToastState>,
    pub(crate) transcript: Vec<TranscriptEntry>,
    pub(crate) transcript_scroll: u16,
    pub(crate) follow_transcript: bool,
    pub(crate) inspector_title: String,
    pub(crate) inspector: Vec<InspectorEntry>,
    pub(crate) inspector_scroll: u16,
    pub(crate) activity: Vec<String>,
    pub(crate) activity_scroll: u16,
    pub(crate) status: String,
    pub(crate) turn_running: bool,
    pub(crate) turn_started_at: Option<Instant>,
    pub(crate) active_tool_label: Option<String>,
    pub(crate) plan_items: Vec<PlanEntry>,
    pub(crate) execution: Option<ExecutionEntry>,
    pub(crate) pending_controls: Vec<PendingControlSummary>,
    pub(crate) pending_control_picker: Option<PendingControlPickerState>,
    pub(crate) editing_pending_control: Option<PendingControlEditorState>,
    pub(crate) statusline_picker: Option<StatusLinePickerState>,
    pub(crate) thinking_effort_picker: Option<ThinkingEffortPickerState>,
    pub(crate) theme_picker: Option<ThemePickerState>,
    pub(crate) history_rollback: Option<HistoryRollbackState>,
}

impl TuiState {
    pub(crate) fn push_activity(&mut self, line: impl Into<String>) {
        self.activity.push(line.into());
        self.activity_scroll = u16::MAX;
        if self.activity.len() > 128 {
            let overflow = self.activity.len() - 128;
            self.activity.drain(0..overflow);
        }
    }

    pub(crate) fn push_transcript(&mut self, entry: impl Into<TranscriptEntry>) {
        self.transcript.push(entry.into());
        self.mark_transcript_follow();
    }

    pub(crate) fn replace_transcript(
        &mut self,
        index: usize,
        entry: impl Into<TranscriptEntry>,
    ) -> bool {
        let Some(slot) = self.transcript.get_mut(index) else {
            return false;
        };
        *slot = entry.into();
        self.mark_transcript_follow();
        true
    }

    pub(crate) fn append_transcript_text(&mut self, index: usize, delta: &str) -> bool {
        let Some(entry) = self.transcript.get_mut(index) else {
            return false;
        };
        let appended = entry.append_text(delta);
        if appended {
            self.mark_transcript_follow();
        }
        appended
    }

    fn mark_transcript_follow(&mut self) {
        if self.follow_transcript {
            self.transcript_scroll = u16::MAX;
        }
    }

    pub(crate) fn show_toast(&mut self, tone: ToastTone, message: impl Into<String>) {
        self.toast = Some(ToastState {
            message: message.into(),
            tone,
            expires_at: Instant::now() + Duration::from_secs(4),
        });
    }

    pub(crate) fn clear_toast(&mut self) {
        self.toast = None;
    }

    pub(crate) fn expire_toast_if_due(&mut self) -> bool {
        let expired = self
            .toast
            .as_ref()
            .is_some_and(|toast| Instant::now() >= toast.expires_at);
        if expired {
            self.toast = None;
        }
        expired
    }

    pub(crate) fn scroll_focused(&mut self, delta: i16) {
        match self.main_pane {
            MainPaneMode::Transcript => {
                // Manual transcript scrolling detaches the viewport from live
                // follow mode until the operator explicitly jumps back to end.
                self.follow_transcript = false;
                bump_scroll(&mut self.transcript_scroll, delta);
            }
            MainPaneMode::View => bump_scroll(&mut self.inspector_scroll, delta),
        }
    }

    pub(crate) fn scroll_focused_page(
        &mut self,
        viewport_height: u16,
        half_page: bool,
        backwards: bool,
    ) {
        let amount = page_scroll_amount(viewport_height, half_page);
        let delta = if backwards { -amount } else { amount };
        self.scroll_focused(delta);
    }

    pub(crate) fn scroll_focused_home(&mut self) {
        match self.main_pane {
            MainPaneMode::Transcript => {
                self.follow_transcript = false;
                self.transcript_scroll = 0;
            }
            MainPaneMode::View => self.inspector_scroll = 0,
        }
    }

    pub(crate) fn scroll_focused_end(&mut self) {
        match self.main_pane {
            MainPaneMode::Transcript => {
                self.follow_transcript = true;
                self.transcript_scroll = u16::MAX;
            }
            MainPaneMode::View => self.inspector_scroll = u16::MAX,
        }
    }
}

fn bump_scroll(value: &mut u16, delta: i16) {
    if delta >= 0 {
        *value = value.saturating_add(delta as u16);
    } else {
        *value = value.saturating_sub(delta.unsigned_abs());
    }
}

fn page_scroll_amount(viewport_height: u16, half_page: bool) -> i16 {
    let page = viewport_height.saturating_sub(2).max(1);
    let amount = if half_page { (page / 2).max(1) } else { page };
    amount.min(i16::MAX as u16) as i16
}

#[derive(Clone, Default)]
pub struct SharedUiState(Arc<RwLock<TuiState>>);

impl SharedUiState {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot(&self) -> TuiState {
        self.0.read().unwrap().clone()
    }

    pub(crate) fn replace(&self, state: TuiState) {
        *self.0.write().unwrap() = state;
    }

    pub(crate) fn mutate<F>(&self, f: F)
    where
        F: FnOnce(&mut TuiState),
    {
        f(&mut self.0.write().unwrap());
    }

    pub(crate) fn take_input(&self) -> String {
        let mut state = self.0.write().unwrap();
        state.take_submission_input()
    }

    pub(crate) fn take_submission(&self) -> ComposerSubmission {
        let mut state = self.0.write().unwrap();
        state.take_submission()
    }
}

pub(crate) fn preview_text(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "<empty>".to_string();
    }
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        format!(
            "{}...",
            collapsed
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>()
        )
    }
}

pub(crate) fn git_snapshot(
    workspace_root: &Path,
    host_process_surfaces_allowed: bool,
) -> GitSnapshot {
    // The TUI git snapshot is a convenience-only host subprocess. When the
    // operator continues without sandbox enforcement, keep the UI on the same
    // fail-closed boundary as the runtime tool and hook surfaces.
    if !host_process_surfaces_allowed {
        return GitSnapshot::default();
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("status")
        .arg("--short")
        .arg("--branch")
        .output();
    let Ok(output) = output else {
        return GitSnapshot::default();
    };
    if !output.status.success() {
        return GitSnapshot::default();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let branch = lines
        .next()
        .map(|line| line.trim_start_matches("## ").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let repo_name = git_repo_name(workspace_root).unwrap_or_default();
    let mut staged = 0;
    let mut modified = 0;
    let mut untracked = 0;
    for line in lines {
        if line.starts_with("??") {
            untracked += 1;
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.first().copied().unwrap_or(b' ') != b' ' {
            staged += 1;
        }
        if bytes.get(1).copied().unwrap_or(b' ') != b' ' {
            modified += 1;
        }
    }
    GitSnapshot {
        available: true,
        repo_name,
        branch,
        staged,
        modified,
        untracked,
    }
}

fn git_repo_name(workspace_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Path::new(&root)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{
        ComposerDraftAttachmentKind, ComposerDraftAttachmentState, ComposerDraftState,
        ComposerKillBufferState, ComposerRowAttachmentPreview, HistoryRollbackCandidate,
        MainPaneMode, SharedUiState, ToastState, ToastTone, TuiState, composer_draft_from_messages,
        composer_draft_from_parts, draft_preview_text, git_snapshot, page_scroll_amount,
    };
    use crate::theme::ThemeSummary;
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
}
