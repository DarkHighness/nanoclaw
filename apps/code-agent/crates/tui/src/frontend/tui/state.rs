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
mod tests;
