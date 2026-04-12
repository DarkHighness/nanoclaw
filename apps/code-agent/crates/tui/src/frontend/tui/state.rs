use super::input_history::PersistedComposerHistoryEntry;
use crate::interaction::{
    PendingControlKind, PendingControlSummary, SessionPermissionMode, SkillSummary,
};
use crate::statusline::{StatusLineConfig, StatusLineField, status_line_fields};
use crate::theme::ThemeSummary;
use crate::tool_render::{
    ToolCommand, ToolCommandIntent, ToolCompletionState, ToolDetail, ToolDetailBlockKind,
    ToolRenderKind, ToolReview, preview_tool_details, serialize_tool_details,
};
use crate::ui::StartupDiagnosticsSnapshot;
use agent::types::{SubmittedPromptSnapshot, TaskId, TaskOrigin, TaskStatus, TokenLedgerSnapshot};
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
    CollectionPickerState, HistoryRollbackCandidate, HistoryRollbackState,
    PendingControlEditorState, PendingControlPickerState, StatusLinePickerState, ThemePickerState,
    ThinkingEffortPickerState, ToolReviewOverlayState,
};
pub(crate) use transcript::{
    InspectorAction, InspectorEntry, InspectorKeyAction, TranscriptDetailPrefix, TranscriptEntry,
    TranscriptSerializedPrefix, TranscriptShellBlockKind, TranscriptShellDetail,
    TranscriptShellEntry, TranscriptShellStatus, TranscriptToolEntry,
    TranscriptToolHeadlineSubjectKind, TranscriptToolStatus,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitPorcelainState {
    Unmodified,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GitPorcelainEntry {
    BranchHeader(String),
    Tracked {
        index: GitPorcelainState,
        worktree: GitPorcelainState,
    },
    Untracked,
    Ignored,
}

impl GitPorcelainEntry {
    fn parse(line: &str) -> Option<Self> {
        if let Some(branch) = line.strip_prefix("## ") {
            return Some(Self::BranchHeader(branch.to_string()));
        }
        if let Some(status) = line.get(..2) {
            return match status {
                "??" => Some(Self::Untracked),
                "!!" => Some(Self::Ignored),
                _ => {
                    let mut chars = status.chars();
                    let index = parse_git_porcelain_state(chars.next()?)?;
                    let worktree = parse_git_porcelain_state(chars.next()?)?;
                    Some(Self::Tracked { index, worktree })
                }
            };
        }
        None
    }
}

fn parse_git_porcelain_state(marker: char) -> Option<GitPorcelainState> {
    match marker {
        ' ' => Some(GitPorcelainState::Unmodified),
        'M' | 'A' | 'D' | 'R' | 'C' | 'U' | 'T' => Some(GitPorcelainState::Changed),
        _ => None,
    }
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
    pub(crate) skills: Vec<SkillSummary>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposerContextHint {
    LiveTaskFinished {
        task_id: agent::types::TaskId,
        status: agent::types::TaskStatus,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TurnPhase {
    #[default]
    Idle,
    Working,
    WaitingApproval,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToastState {
    pub(crate) message: String,
    pub(crate) tone: ToastTone,
    pub(crate) expires_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveToolCall {
    pub(crate) call_id: String,
    pub(crate) entry: TranscriptToolEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActiveToolCellKind {
    Single,
    ExplorationGroup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveToolCell {
    pub(crate) cell_id: String,
    pub(crate) kind: ActiveToolCellKind,
    pub(crate) calls: Vec<ActiveToolCall>,
    pub(crate) entry: TranscriptToolEntry,
}

impl ActiveToolCell {
    pub(crate) fn new(call_id: impl Into<String>, entry: TranscriptToolEntry) -> Self {
        let call_id = call_id.into();
        Self::new_with_cell_id(call_id.clone(), call_id, entry)
    }

    pub(crate) fn new_with_cell_id(
        cell_id: impl Into<String>,
        call_id: impl Into<String>,
        entry: TranscriptToolEntry,
    ) -> Self {
        let cell_id = cell_id.into();
        let call_id = call_id.into();
        let kind = active_tool_cell_kind(&entry);
        let calls = vec![ActiveToolCall { call_id, entry }];
        let entry = active_tool_cell_entry(kind, &calls);
        Self {
            cell_id,
            kind,
            calls,
            entry,
        }
    }

    pub(crate) fn contains_call(&self, call_id: &str) -> bool {
        self.calls.iter().any(|call| call.call_id == call_id)
    }

    pub(crate) fn is_running(&self) -> bool {
        self.calls
            .iter()
            .any(|call| is_live_tool_status(call.entry.status))
    }

    pub(crate) fn holds_completed_entry(&self) -> bool {
        self.kind == ActiveToolCellKind::ExplorationGroup && !self.is_running()
    }

    pub(crate) fn update_call(
        &mut self,
        call_id: &str,
        entry: TranscriptToolEntry,
    ) -> Option<TranscriptToolEntry> {
        let call = self.calls.iter_mut().find(|call| call.call_id == call_id)?;
        let previous = std::mem::replace(&mut call.entry, entry);
        self.kind = active_tool_cell_kind(&call.entry);
        self.entry = active_tool_cell_entry(self.kind, &self.calls);
        Some(previous)
    }

    pub(crate) fn remove_call(&mut self, call_id: &str) -> Option<TranscriptToolEntry> {
        let index = self.calls.iter().position(|call| call.call_id == call_id)?;
        let removed = self.calls.remove(index);
        if let Some(first) = self.calls.first() {
            self.kind = active_tool_cell_kind(&first.entry);
            self.entry = active_tool_cell_entry(self.kind, &self.calls);
        }
        Some(removed.entry)
    }

    pub(crate) fn can_absorb_running_call(&self, entry: &TranscriptToolEntry) -> bool {
        entry.status == TranscriptToolStatus::Running && self.can_absorb_exploration_call(entry)
    }

    pub(crate) fn can_absorb_exploration_call(&self, entry: &TranscriptToolEntry) -> bool {
        self.kind == ActiveToolCellKind::ExplorationGroup
            && self
                .calls
                .first()
                .is_some_and(|call| call.entry.tool_name == entry.tool_name)
            && is_exploration_tool_entry(&self.entry)
            && is_exploration_tool_entry(entry)
    }

    pub(crate) fn absorb_exploration_call(
        &mut self,
        call_id: impl Into<String>,
        entry: TranscriptToolEntry,
    ) -> bool {
        if !self.can_absorb_exploration_call(&entry) {
            return false;
        }

        self.calls.push(ActiveToolCall {
            call_id: call_id.into(),
            entry,
        });
        self.entry = active_tool_cell_entry(self.kind, &self.calls);
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveMonitorCell {
    pub(crate) monitor_id: String,
    pub(crate) started_at: Instant,
    pub(crate) entry: TranscriptShellEntry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackedTaskSummary {
    pub(crate) task_id: TaskId,
    pub(crate) role: String,
    pub(crate) origin: TaskOrigin,
    pub(crate) status: TaskStatus,
    pub(crate) summary: Option<String>,
    pub(crate) parent_agent_id: Option<String>,
    pub(crate) child_agent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolSelectionTarget {
    Transcript(usize),
    LiveCell(String),
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
    pub(crate) command_history: Vec<SubmittedPromptSnapshot>,
    pub(crate) persisted_history_entries: Vec<PersistedComposerHistoryEntry>,
    pub(crate) local_input_history: Vec<ComposerDraftState>,
    pub(crate) local_command_history: Vec<ComposerDraftState>,
    pub(crate) input_history_navigation: Option<ComposerHistoryNavigationState>,
    pub(crate) composer_completion_index: usize,
    pub(crate) composer_context_hint: Option<ComposerContextHint>,
    pub(crate) toast: Option<ToastState>,
    pub(crate) transcript: Vec<TranscriptEntry>,
    pub(crate) active_tool_cells: Vec<ActiveToolCell>,
    pub(crate) active_monitors: Vec<ActiveMonitorCell>,
    pub(crate) tracked_tasks: Vec<TrackedTaskSummary>,
    pub(crate) tool_selection: Option<ToolSelectionTarget>,
    pub(crate) transcript_scroll: u16,
    pub(crate) follow_transcript: bool,
    pub(crate) inspector_title: String,
    pub(crate) inspector: Vec<InspectorEntry>,
    pub(crate) inspector_scroll: u16,
    pub(crate) activity: Vec<String>,
    pub(crate) activity_scroll: u16,
    pub(crate) status: String,
    pub(crate) turn_phase: TurnPhase,
    pub(crate) turn_running: bool,
    pub(crate) turn_started_at: Option<Instant>,
    pub(crate) pending_controls: Vec<PendingControlSummary>,
    pub(crate) pending_control_picker: Option<PendingControlPickerState>,
    pub(crate) editing_pending_control: Option<PendingControlEditorState>,
    pub(crate) collection_picker: Option<CollectionPickerState>,
    pub(crate) statusline_picker: Option<StatusLinePickerState>,
    pub(crate) thinking_effort_picker: Option<ThinkingEffortPickerState>,
    pub(crate) theme_picker: Option<ThemePickerState>,
    pub(crate) history_rollback: Option<HistoryRollbackState>,
    pub(crate) tool_review: Option<ToolReviewOverlayState>,
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

    pub(crate) fn push_transcript(&mut self, entry: impl Into<TranscriptEntry>) -> usize {
        let entry = entry.into();
        if let Some((index, last)) = self.transcript.iter_mut().enumerate().last()
            && last.try_merge(&entry)
        {
            self.mark_transcript_follow();
            return index;
        }

        self.transcript.push(entry);
        self.mark_transcript_follow();
        self.transcript.len() - 1
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

    pub(crate) fn move_tool_selection(&mut self, backwards: bool) -> bool {
        let selectable = selectable_tool_targets(&self.transcript, &self.active_tool_cells);
        if selectable.is_empty() {
            return false;
        }
        let current = self.tool_selection.as_ref().and_then(|selected| {
            selectable
                .iter()
                .position(|candidate| candidate == selected)
        });
        let next = match current {
            Some(current) if backwards => current.checked_sub(1).unwrap_or(selectable.len() - 1),
            Some(current) => (current + 1) % selectable.len(),
            None if backwards => selectable.len() - 1,
            None => 0,
        };
        self.follow_transcript = false;
        self.tool_selection = Some(selectable[next].clone());
        true
    }

    pub(crate) fn jump_tool_selection(&mut self, oldest: bool) -> bool {
        let selectable = selectable_tool_targets(&self.transcript, &self.active_tool_cells);
        if selectable.is_empty() {
            return false;
        }
        self.follow_transcript = false;
        self.tool_selection = Some(if oldest {
            selectable[0].clone()
        } else {
            selectable[selectable.len() - 1].clone()
        });
        true
    }

    pub(crate) fn clear_tool_selection(&mut self) {
        self.tool_selection = None;
    }

    pub(crate) fn replace_tracked_tasks(&mut self, tasks: Vec<TrackedTaskSummary>) {
        self.tracked_tasks = tasks;
    }

    pub(crate) fn selected_tool_entry(&self) -> Option<&TranscriptToolEntry> {
        match self.tool_selection.as_ref()? {
            ToolSelectionTarget::Transcript(index) => self
                .transcript
                .get(*index)
                .and_then(TranscriptEntry::tool_entry),
            ToolSelectionTarget::LiveCell(cell_id) => self
                .active_tool_cells
                .iter()
                .find(|active| active.cell_id == *cell_id)
                .map(|active| &active.entry),
        }
    }

    pub(crate) fn promote_live_tool_selection(&mut self, cell_id: &str, transcript_index: usize) {
        if matches!(
            self.tool_selection.as_ref(),
            Some(ToolSelectionTarget::LiveCell(selected)) if selected == cell_id
        ) {
            self.tool_selection = Some(ToolSelectionTarget::Transcript(transcript_index));
        }
    }

    pub(crate) fn redirect_live_tool_selection(&mut self, from: &str, to: &str) {
        if matches!(
            self.tool_selection.as_ref(),
            Some(ToolSelectionTarget::LiveCell(selected)) if selected == from
        ) {
            self.tool_selection = Some(ToolSelectionTarget::LiveCell(to.to_string()));
        }
    }

    pub(crate) fn clear_missing_live_tool_selection(&mut self) {
        if let Some(ToolSelectionTarget::LiveCell(cell_id)) = self.tool_selection.as_ref()
            && !self
                .active_tool_cells
                .iter()
                .any(|active| active.cell_id == *cell_id)
        {
            self.tool_selection = None;
        }
    }

    pub(crate) fn upsert_active_monitor(
        &mut self,
        monitor_id: impl Into<String>,
        started_at: Instant,
        entry: TranscriptShellEntry,
    ) {
        let monitor_id = monitor_id.into();
        if let Some(existing) = self
            .active_monitors
            .iter_mut()
            .find(|monitor| monitor.monitor_id == monitor_id)
        {
            existing.started_at = started_at;
            existing.entry = entry;
            return;
        }
        self.active_monitors.push(ActiveMonitorCell {
            monitor_id,
            started_at,
            entry,
        });
    }

    pub(crate) fn remove_active_monitor(
        &mut self,
        monitor_id: &str,
    ) -> Option<TranscriptShellEntry> {
        let index = self
            .active_monitors
            .iter()
            .position(|monitor| monitor.monitor_id == monitor_id)?;
        Some(self.active_monitors.remove(index).entry)
    }

    pub(crate) fn drain_transcript_ready_tool_cells(&mut self) -> Vec<(String, TranscriptEntry)> {
        let mut drained = Vec::new();
        let mut remaining = Vec::with_capacity(self.active_tool_cells.len());
        for cell in self.active_tool_cells.drain(..) {
            if cell.holds_completed_entry() {
                drained.push((cell.cell_id.clone(), TranscriptEntry::Tool(cell.entry)));
            } else {
                remaining.push(cell);
            }
        }
        self.active_tool_cells = remaining;
        drained
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

fn selectable_tool_targets(
    transcript: &[TranscriptEntry],
    active_tool_cells: &[ActiveToolCell],
) -> Vec<ToolSelectionTarget> {
    let mut selectable = transcript
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .tool_entry()
                .map(|_| ToolSelectionTarget::Transcript(index))
        })
        .collect::<Vec<_>>();
    selectable.extend(
        active_tool_cells
            .iter()
            .map(|active| ToolSelectionTarget::LiveCell(active.cell_id.clone())),
    );
    selectable
}

fn active_tool_cell_kind(entry: &TranscriptToolEntry) -> ActiveToolCellKind {
    if is_exploration_tool_entry(entry) {
        ActiveToolCellKind::ExplorationGroup
    } else {
        ActiveToolCellKind::Single
    }
}

fn active_tool_cell_entry(
    kind: ActiveToolCellKind,
    calls: &[ActiveToolCall],
) -> TranscriptToolEntry {
    match kind {
        ActiveToolCellKind::Single => calls
            .first()
            .map(|call| call.entry.clone())
            .expect("single tool cell requires one call"),
        // Keep grouped exploration state derived from the member calls so status
        // transitions do not depend on stringly observer branches. This mirrors
        // Codex's active exec cell contract: add calls in place, then flush the
        // coalesced cell only when the turn boundary makes it durable history.
        ActiveToolCellKind::ExplorationGroup => build_grouped_exploration_entry(calls),
    }
}

fn build_grouped_exploration_entry(calls: &[ActiveToolCall]) -> TranscriptToolEntry {
    let first = calls
        .first()
        .expect("grouped exploration cell requires at least one call");
    let mut command = first
        .entry
        .detail_lines
        .iter()
        .find_map(|detail| match detail {
            ToolDetail::Command(command) => Some(command.clone()),
            _ => None,
        })
        .expect("grouped exploration call requires command detail");
    for call in calls.iter().skip(1) {
        if let Some(other) = call
            .entry
            .detail_lines
            .iter()
            .find_map(|detail| match detail {
                ToolDetail::Command(command) => Some(command),
                _ => None,
            })
        {
            let _ = command.merge_exploration(other);
        }
    }
    let status = grouped_tool_status(calls);
    let completion = grouped_tool_completion(calls, status);
    TranscriptToolEntry::new_with_review_and_completion(
        status,
        first.entry.tool_name.clone(),
        vec![ToolDetail::Command(command)],
        None,
        completion,
    )
}

fn grouped_tool_status(calls: &[ActiveToolCall]) -> TranscriptToolStatus {
    if calls
        .iter()
        .any(|call| is_live_tool_status(call.entry.status))
    {
        TranscriptToolStatus::Running
    } else if calls
        .iter()
        .any(|call| call.entry.status == TranscriptToolStatus::Failed)
    {
        TranscriptToolStatus::Failed
    } else if calls
        .iter()
        .any(|call| call.entry.status == TranscriptToolStatus::Cancelled)
    {
        TranscriptToolStatus::Cancelled
    } else if calls
        .iter()
        .any(|call| call.entry.status == TranscriptToolStatus::Denied)
    {
        TranscriptToolStatus::Denied
    } else {
        TranscriptToolStatus::Finished
    }
}

fn grouped_tool_completion(
    calls: &[ActiveToolCall],
    status: TranscriptToolStatus,
) -> ToolCompletionState {
    if is_live_tool_status(status) {
        return ToolCompletionState::Neutral;
    }
    if calls.iter().any(|call| {
        call.entry.completion == ToolCompletionState::Failure
            || matches!(
                call.entry.status,
                TranscriptToolStatus::Denied
                    | TranscriptToolStatus::Failed
                    | TranscriptToolStatus::Cancelled
            )
    }) {
        ToolCompletionState::Failure
    } else {
        ToolCompletionState::Success
    }
}

fn is_live_tool_status(status: TranscriptToolStatus) -> bool {
    matches!(
        status,
        TranscriptToolStatus::Requested
            | TranscriptToolStatus::WaitingApproval
            | TranscriptToolStatus::Approved
            | TranscriptToolStatus::Running
    )
}

fn is_exploration_tool_entry(entry: &TranscriptToolEntry) -> bool {
    entry.detail_lines.iter().any(|detail| {
        matches!(
            detail,
            ToolDetail::Command(ToolCommand {
                intent: ToolCommandIntent::Explore,
                ..
            })
        )
    })
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
    let repo_name = git_repo_name(workspace_root).unwrap_or_default();
    let mut branch = "unknown".to_string();
    let mut staged = 0;
    let mut modified = 0;
    let mut untracked = 0;
    // `git status --short --branch` is a stable porcelain protocol. Parse it
    // into typed entries here so UI counters do not depend on ad-hoc prefix
    // slicing spread across the renderer.
    for line in stdout.lines() {
        let Some(entry) = GitPorcelainEntry::parse(line) else {
            continue;
        };
        match entry {
            GitPorcelainEntry::BranchHeader(parsed_branch) => branch = parsed_branch,
            GitPorcelainEntry::Tracked { index, worktree } => {
                if index == GitPorcelainState::Changed {
                    staged += 1;
                }
                if worktree == GitPorcelainState::Changed {
                    modified += 1;
                }
            }
            GitPorcelainEntry::Untracked => {
                untracked += 1;
            }
            GitPorcelainEntry::Ignored => {}
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
