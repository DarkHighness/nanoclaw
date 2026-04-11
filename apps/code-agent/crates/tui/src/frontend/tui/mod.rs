mod approval;
mod commands;
mod composer;
mod history;
mod history_rollback;
mod input_history;
mod interaction_keys;
mod observer;
mod paste_burst;
mod render;
mod runtime_flow;
mod session_shell;
mod slash_commands;
mod state;
mod tool_state;

use crate::backend::{CodeAgentUiSession, preview_id};
use crate::config::persist_tui_theme_selection;
use crate::interaction::{
    ApprovalPrompt, ModelReasoningEffortOutcome, PermissionProfile, PermissionRequestDecision,
    PermissionRequestPrompt, SessionPermissionMode, UserInputAnswer, UserInputPrompt,
    UserInputSubmission,
};
use crate::statusline::status_line_fields;
use crate::theme::{ThemeCatalog, active_theme_id, install_theme_catalog, set_active_theme};
use crate::ui::{
    HistoryRollbackRound, LiveTaskAttentionAction, LiveTaskAttentionOutcome, LiveTaskControlAction,
    LiveTaskControlOutcome, LiveTaskMessageAction, LiveTaskMessageOutcome, LiveTaskSpawnOutcome,
    LiveTaskSummary, LiveTaskWaitOutcome, LoadedAgentSession, LoadedMcpPrompt, LoadedMcpResource,
    LoadedSession, LoadedTask, McpPromptSummary, McpResourceSummary, McpServerSummary,
    PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
    PersistedTaskSummary, SessionEvent, SessionExportArtifact, SessionOperation,
    SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot, SideQuestionOutcome,
    StartupDiagnosticsSnapshot, UIAsyncCommand, UIAsyncValue, UICommand, UIQuery, UIQueryValue,
    UIResultValue,
};
use approval::approval_decision_for_key;
use commands::{
    SlashCommand, SlashCommandEnterAction, command_palette_lines_for, cycle_slash_command,
    move_slash_command_selection, parse_slash_command, resolve_slash_enter_action,
};
use history::{
    format_agent_session_inspector, format_agent_session_summary_line,
    format_live_task_control_outcome, format_live_task_message_outcome,
    format_live_task_spawn_outcome, format_live_task_summary_line, format_live_task_wait_outcome,
    format_mcp_prompt_summary_line, format_mcp_resource_summary_line,
    format_mcp_server_summary_line, format_session_export_result, format_session_inspector,
    format_session_operation_outcome, format_session_search_line, format_session_summary_line,
    format_session_transcript_lines, format_startup_diagnostics, format_task_inspector,
    format_task_summary_line, format_visible_transcript_lines,
    format_visible_transcript_preview_lines,
};
use observer::SharedRenderObserver;
use paste_burst::{CharDecision, FlushResult, PasteBurst};
use render::{main_pane_viewport_height, render};
pub use state::SharedUiState;
use state::{
    ComposerDraftAttachmentKind, ComposerDraftAttachmentState, ComposerDraftState,
    ComposerSubmission, InspectorEntry, ToastTone, TranscriptEntry, TranscriptShellDetail,
    TuiState,
};
use tool_state::restore_tool_panels;

use agent::RuntimeCommand;
use agent::tools::{
    ToolExecutionContext, load_tool_image, resolve_tool_path_against_workspace_root,
};
use agent::types::{
    AgentStatus, Message, MessagePart, SubmittedPromptSnapshot, message_operator_text,
};
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use std::collections::BTreeMap;
use std::env;
use std::fs as stdfs;
use std::io::{self, Stdout};
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::time::Instant;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::task::{JoinHandle, spawn_local};
use tokio::time::{Duration, sleep};
use tracing::error;

pub struct CodeAgentTui {
    session: CodeAgentUiSession,
    initial_prompt: Option<String>,
    ui_state: SharedUiState,
    event_renderer: SharedRenderObserver,
    active_user_input: Option<ActiveUserInputState>,
    turn_task: Option<JoinHandle<Result<()>>>,
    operator_task: Option<JoinHandle<Result<OperatorTaskOutcome>>>,
    paste_burst: PasteBurst,
}

enum OperatorTaskOutcome {
    WaitLiveTask(LiveTaskWaitOutcome),
    SideQuestion(SideQuestionOutcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlainInputSubmitAction {
    StartPrompt,
    QueuePrompt,
    SteerActiveTurn,
}

const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

#[derive(Clone, Debug, Default)]
struct ActiveUserInputState {
    prompt_id: String,
    current_question: usize,
    answers: BTreeMap<String, UserInputAnswer>,
    collecting_other_note: bool,
}

impl ActiveUserInputState {
    fn new(prompt_id: String) -> Self {
        Self {
            prompt_id,
            ..Self::default()
        }
    }
}

struct UserInputView<'a> {
    prompt: &'a UserInputPrompt,
    flow: Option<&'a ActiveUserInputState>,
    input: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadedComposerFile {
    requested_path: String,
    file_name: Option<String>,
    mime_type: Option<String>,
    data_base64: String,
}

fn summarize_nonfatal_error(operation: &'static str, error: &anyhow::Error) -> String {
    error!(operation, error = ?error, "UI operation failed");
    error.to_string()
}

impl CodeAgentTui {
    pub fn new(
        session: CodeAgentUiSession,
        initial_prompt: Option<String>,
        ui_state: SharedUiState,
        theme_catalog: ThemeCatalog,
    ) -> Self {
        install_theme_catalog(theme_catalog);
        Self {
            session,
            initial_prompt,
            event_renderer: SharedRenderObserver::new(ui_state.clone()),
            ui_state,
            active_user_input: None,
            turn_task: None,
            operator_task: None,
            paste_burst: PasteBurst::default(),
        }
    }

    fn query<T: UIQueryValue>(&self, query: UIQuery) -> T {
        self.session.query(query)
    }

    fn dispatch<T: UIResultValue>(&self, command: UICommand) -> Result<T> {
        self.session.dispatch(command)
    }

    async fn run_ui<T: UIAsyncValue>(&self, command: UIAsyncCommand) -> Result<T> {
        self.session.run(command).await
    }

    fn workspace_root_buf(&self) -> std::path::PathBuf {
        self.query(UIQuery::WorkspaceRoot)
    }

    fn startup_snapshot(&self) -> SessionStartupSnapshot {
        self.query(UIQuery::StartupSnapshot)
    }

    fn host_process_surfaces_allowed(&self) -> bool {
        self.query(UIQuery::HostProcessSurfacesAllowed)
    }

    fn approval_prompt(&self) -> Option<ApprovalPrompt> {
        self.query(UIQuery::ApprovalPrompt)
    }

    fn permission_request_prompt(&self) -> Option<PermissionRequestPrompt> {
        self.query(UIQuery::PermissionRequestPrompt)
    }

    fn user_input_prompt(&self) -> Option<UserInputPrompt> {
        self.query(UIQuery::UserInputPrompt)
    }

    fn pending_controls(&self) -> Vec<crate::interaction::PendingControlSummary> {
        self.query(UIQuery::PendingControls)
    }

    fn queued_command_count(&self) -> usize {
        self.query(UIQuery::QueuedCommandCount)
    }

    fn startup_diagnostics(&self) -> StartupDiagnosticsSnapshot {
        self.query(UIQuery::StartupDiagnostics)
    }

    fn permission_grant_profiles(&self) -> (PermissionProfile, PermissionProfile) {
        self.query(UIQuery::PermissionGrantProfiles)
    }

    fn skills(&self) -> Vec<crate::interaction::SkillSummary> {
        self.query(UIQuery::Skills)
    }

    fn resolve_approval(&self, decision: crate::interaction::ApprovalDecision) -> bool {
        self.dispatch(UICommand::ResolveApproval(decision))
            .unwrap_or(false)
    }

    fn resolve_permission_request(&self, decision: PermissionRequestDecision) -> bool {
        self.dispatch(UICommand::ResolvePermissionRequest(decision))
            .unwrap_or(false)
    }

    fn resolve_user_input(&self, submission: UserInputSubmission) -> bool {
        self.dispatch(UICommand::ResolveUserInput(submission))
            .unwrap_or(false)
    }

    fn cancel_user_input(&self, reason: impl Into<String>) -> bool {
        self.dispatch(UICommand::CancelUserInput {
            reason: reason.into(),
        })
        .unwrap_or(false)
    }

    fn remove_pending_control(
        &self,
        control_ref: &str,
    ) -> Result<crate::interaction::PendingControlSummary> {
        self.dispatch(UICommand::RemovePendingControl {
            control_ref: control_ref.to_string(),
        })
    }

    fn update_pending_control(
        &self,
        control_ref: &str,
        content: &str,
    ) -> Result<crate::interaction::PendingControlSummary> {
        self.dispatch(UICommand::UpdatePendingControl {
            control_ref: control_ref.to_string(),
            content: content.to_string(),
        })
    }

    fn schedule_runtime_steer(
        &self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> Result<String> {
        self.dispatch(UICommand::ScheduleRuntimeSteer {
            message: message.into(),
            reason,
        })
    }

    fn take_pending_steers(&self) -> Result<Vec<crate::interaction::PendingControlSummary>> {
        self.dispatch(UICommand::TakePendingSteers)
    }

    fn cycle_model_reasoning_effort_result(&self) -> Result<ModelReasoningEffortOutcome> {
        self.dispatch(UICommand::CycleModelReasoningEffort)
    }

    fn set_model_reasoning_effort_result(
        &self,
        effort: &str,
    ) -> Result<ModelReasoningEffortOutcome> {
        self.dispatch(UICommand::SetModelReasoningEffort {
            effort: effort.to_string(),
        })
    }

    fn schedule_live_task_attention(
        &self,
        outcome: &LiveTaskWaitOutcome,
        turn_running: bool,
    ) -> Result<LiveTaskAttentionOutcome> {
        self.dispatch(UICommand::ScheduleLiveTaskAttention {
            outcome: outcome.clone(),
            turn_running,
        })
    }

    async fn refresh_stored_session_count(&self) -> Result<usize> {
        self.run_ui(UIAsyncCommand::RefreshStoredSessionCount).await
    }

    pub async fn run(mut self) -> Result<()> {
        self.ui_state.replace(self.startup_state());

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Keep terminal-native mouse selection available in the main transcript.
        // The TUI only uses keyboard navigation here, so capturing mouse events
        // would mostly disable copy/select without providing meaningful utility.
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        if let Some(prompt) = self.initial_prompt.take() {
            self.start_turn(prompt).await;
        }

        let result = self.event_loop(&mut terminal).await;
        if let Some(task) = self.operator_task.take() {
            task.abort();
        }
        let _ = self
            .run_ui::<()>(UIAsyncCommand::EndSession {
                reason: Some("operator_exit".to_string()),
            })
            .await;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        )?;
        terminal.show_cursor()?;
        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        loop {
            self.flush_due_paste_burst().await;
            self.maybe_finish_turn().await?;
            self.apply_backend_events();
            self.maybe_finish_operator_task().await?;
            self.ui_state.mutate(|state| {
                let _ = state.expire_toast_if_due();
            });
            self.sync_runtime_control_state();
            let permission_request_prompt = self.permission_request_prompt();
            let user_input_prompt = self.user_input_prompt();
            self.sync_user_input_prompt(user_input_prompt.as_ref());

            let snapshot = self.ui_state.snapshot();
            let approval = self.approval_prompt();
            let user_input_view = user_input_prompt.as_ref().map(|prompt| UserInputView {
                prompt,
                flow: self.active_user_input.as_ref(),
                input: snapshot.input.as_str(),
            });
            let terminal_size = terminal.size()?;
            let viewport_height = main_pane_viewport_height(
                Rect::new(0, 0, terminal_size.width, terminal_size.height),
                &snapshot,
                approval.as_ref(),
                permission_request_prompt.as_ref(),
                user_input_view.as_ref(),
            );
            terminal.draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    approval.as_ref(),
                    permission_request_prompt.as_ref(),
                    user_input_view.as_ref(),
                )
            })?;

            if !event::poll(Duration::ZERO)? {
                sleep(Duration::from_millis(16)).await;
                continue;
            }
            match event::read()? {
                Event::Paste(text) => {
                    self.handle_explicit_paste(&text).await;
                    continue;
                }
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if self.handle_approval_key(key) {
                        continue;
                    }
                    if self.handle_permission_request_key(key) {
                        continue;
                    }
                    if self.handle_user_input_key(key) {
                        continue;
                    }
                    if self.handle_pending_control_picker_key(key) {
                        continue;
                    }
                    if self.handle_statusline_picker_key(key) {
                        continue;
                    }
                    if self.handle_thinking_effort_picker_key(key) {
                        continue;
                    }
                    if self.handle_theme_picker_key(key) {
                        continue;
                    }
                    if self.handle_history_rollback_key(key).await? {
                        continue;
                    }
                    if self.handle_paste_burst_key(key).await {
                        continue;
                    }
                    match key.code {
                        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                            self.ui_state.mutate(|state| {
                                let opened = state.open_pending_control_picker(true);
                                if opened {
                                    state.status = "Opened pending controls".to_string();
                                }
                            });
                            continue;
                        }
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                            self.ui_state.mutate(|state| {
                                if state.pending_control_picker.is_some() {
                                    let _ = state.move_pending_control_picker(false);
                                } else {
                                    let _ = state.open_pending_control_picker(true);
                                }
                            });
                            continue;
                        }
                        KeyCode::Tab => {
                            let snapshot = self.ui_state.snapshot();
                            if self.try_apply_pending_control_edit(&snapshot.input).await {
                                continue;
                            }
                            if let Some(action) = plain_input_submit_action(
                                &snapshot.input,
                                composer_has_prompt_content(&snapshot),
                                composer_requires_prompt_submission(&snapshot),
                                snapshot.turn_running,
                                KeyCode::Tab,
                            ) {
                                if self.reject_unsupported_image_submission(&snapshot) {
                                    continue;
                                }
                                let submission = self.ui_state.take_submission();
                                self.apply_plain_input_submit(action, submission).await;
                                continue;
                            }
                            if self.apply_command_completion(false) {
                                continue;
                            }
                        }
                        KeyCode::BackTab => {
                            if self.apply_command_completion(true) {
                                continue;
                            }
                        }
                        KeyCode::Up => {
                            if self.move_command_selection(true) {
                                continue;
                            }
                            if self.move_selected_row_attachment(true) {
                                continue;
                            }
                            if self.navigate_input_history(true) {
                                continue;
                            }
                            if self.move_input_cursor_vertical(true) {
                                continue;
                            }
                            if self.move_input_cursor_boundary(true) {
                                continue;
                            }
                            self.ui_state.mutate(|state| state.scroll_focused(-1));
                        }
                        KeyCode::Down => {
                            if self.move_command_selection(false) {
                                continue;
                            }
                            if self.move_selected_row_attachment(false) {
                                continue;
                            }
                            if self.navigate_input_history(false) {
                                continue;
                            }
                            if self.move_input_cursor_vertical(false) {
                                continue;
                            }
                            if self.move_input_cursor_boundary(false) {
                                continue;
                            }
                            self.ui_state.mutate(|state| state.scroll_focused(1));
                        }
                        KeyCode::Left => {
                            if self.move_input_cursor_horizontal(true) {
                                continue;
                            }
                        }
                        KeyCode::Right => {
                            if self.move_input_cursor_horizontal(false) {
                                continue;
                            }
                        }
                        KeyCode::PageUp => {
                            self.ui_state.mutate(|state| {
                                state.scroll_focused_page(viewport_height, false, true)
                            });
                        }
                        KeyCode::PageDown => {
                            self.ui_state.mutate(|state| {
                                state.scroll_focused_page(viewport_height, false, false)
                            });
                        }
                        KeyCode::Home => {
                            if self.move_input_cursor_home() {
                                continue;
                            }
                            self.ui_state.mutate(|state| state.scroll_focused_home());
                        }
                        KeyCode::End => {
                            if self.move_input_cursor_end() {
                                continue;
                            }
                            self.ui_state.mutate(|state| state.scroll_focused_end());
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.ui_state.mutate(|state| {
                                state.scroll_focused_page(viewport_height, true, true)
                            });
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.ui_state.mutate(|state| {
                                state.scroll_focused_page(viewport_height, true, false)
                            });
                        }
                        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.cycle_model_reasoning_effort();
                            continue;
                        }
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.launch_external_editor(terminal).await?;
                            continue;
                        }
                        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if self.kill_input_to_end() {
                                continue;
                            }
                        }
                        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if self.yank_kill_buffer() {
                                continue;
                            }
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if self.stash_composer_draft_on_ctrl_c() {
                                continue;
                            }
                            return Ok(());
                        }
                        KeyCode::Enter => {
                            let snapshot = self.ui_state.snapshot();
                            if self.try_apply_pending_control_edit(&snapshot.input).await {
                                continue;
                            }
                            if snapshot.input.starts_with('/') {
                                if let Some(action) = resolve_slash_enter_action(
                                    &snapshot.input,
                                    snapshot.command_completion_index,
                                ) {
                                    match action {
                                        SlashCommandEnterAction::Complete { input, index } => {
                                            self.ui_state.mutate(|state| {
                                                state.replace_input(input);
                                                state.command_completion_index = index;
                                            });
                                            continue;
                                        }
                                        SlashCommandEnterAction::Execute(input) => {
                                            self.record_submitted_input(&input);
                                            self.ui_state.mutate(|state| {
                                                state.clear_input();
                                            });
                                            if self.apply_command(&input).await? {
                                                return Ok(());
                                            }
                                            continue;
                                        }
                                    }
                                }
                            }
                            if let Some(action) = plain_input_submit_action(
                                &snapshot.input,
                                composer_has_prompt_content(&snapshot),
                                composer_requires_prompt_submission(&snapshot),
                                snapshot.turn_running,
                                KeyCode::Enter,
                            ) {
                                // Rejecting here keeps the rich draft intact. Once
                                // `take_submission()` runs the composer buffer and
                                // attachment state are cleared on success.
                                if self.reject_unsupported_image_submission(&snapshot) {
                                    continue;
                                }
                                let submission = self.ui_state.take_submission();
                                self.apply_plain_input_submit(action, submission).await;
                                continue;
                            }
                            let input = self.ui_state.take_input();
                            if input.trim().is_empty() {
                                continue;
                            }
                            if input.starts_with('/') {
                                self.record_submitted_input(&input);
                                if self.apply_command(&input).await? {
                                    return Ok(());
                                }
                            } else {
                                self.start_turn(input).await;
                            }
                        }
                        KeyCode::Esc => {
                            let snapshot = self.ui_state.snapshot();
                            if snapshot.editing_pending_control.is_some() {
                                self.ui_state.mutate(|state| {
                                    state.clear_pending_control_edit();
                                    state.clear_input();
                                    state.status = "Cancelled pending control edit".to_string();
                                    state.push_activity("cancelled pending control edit");
                                });
                                continue;
                            }
                            if self.turn_task.is_some() {
                                self.interrupt_active_turn().await?;
                                continue;
                            }
                            if snapshot.input.is_empty()
                                && snapshot.main_pane == state::MainPaneMode::Transcript
                            {
                                self.prime_history_rollback().await?;
                                continue;
                            }
                        }
                        KeyCode::Backspace => {
                            if self.remove_selected_row_attachment() {
                                continue;
                            }
                            self.ui_state.mutate(|state| {
                                state.pop_input_char();
                            });
                        }
                        KeyCode::Delete => {
                            if self.remove_selected_row_attachment() {
                                continue;
                            }
                        }
                        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.ui_state.mutate(|state| {
                                state.push_input_char(ch);
                            });
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn plain_input_submit_action(
    input: &str,
    has_prompt_content: bool,
    requires_prompt_submission: bool,
    turn_running: bool,
    key: KeyCode,
) -> Option<PlainInputSubmitAction> {
    if !has_prompt_content || input.starts_with('/') {
        return None;
    }
    match (turn_running, key) {
        (true, KeyCode::Enter) if requires_prompt_submission => {
            Some(PlainInputSubmitAction::QueuePrompt)
        }
        (true, KeyCode::Enter) => Some(PlainInputSubmitAction::SteerActiveTurn),
        (true, KeyCode::Tab) => Some(PlainInputSubmitAction::QueuePrompt),
        (false, KeyCode::Enter) => Some(PlainInputSubmitAction::StartPrompt),
        _ => None,
    }
}

fn merge_interrupt_steers(steers: Vec<String>) -> Option<String> {
    if steers.is_empty() {
        None
    } else {
        Some(steers.join("\n"))
    }
}

fn build_history_rollback_candidates(
    rounds: &[HistoryRollbackRound],
) -> Vec<state::HistoryRollbackCandidate> {
    rounds
        .iter()
        .map(|round| {
            let prompt = agent::types::message_operator_text(&round.prompt_message);
            let draft = state::composer_draft_from_message(&round.prompt_message);
            state::HistoryRollbackCandidate {
                message_id: round.rollback_message_id.clone(),
                prompt,
                draft,
                turn_preview_lines: format_visible_transcript_preview_lines(&round.round_messages),
                removed_turn_count: round.removed_turn_count,
                removed_message_count: round.removed_message_count,
            }
        })
        .collect()
}

fn history_rollback_status(
    candidate: &state::HistoryRollbackCandidate,
    selected: usize,
    total: usize,
) -> String {
    format!(
        "Rollback turn {} of {} · removes {} turn(s) / {} message(s) · {}",
        selected + 1,
        total,
        candidate.removed_turn_count,
        candidate.removed_message_count,
        state::draft_preview_text(&candidate.draft, &candidate.prompt, 40)
    )
}

fn attachment_preview_status_label(preview: &state::ComposerRowAttachmentPreview) -> String {
    format!("attachment #{} · {}", preview.index, preview.summary)
}

fn removed_attachment_status_label(
    preview: Option<&state::ComposerRowAttachmentPreview>,
    attachment: &ComposerDraftAttachmentState,
) -> String {
    preview
        .map(attachment_preview_status_label)
        .or_else(|| {
            attachment
                .row_summary()
                .map(|summary| format!("attachment · {summary}"))
        })
        .unwrap_or_else(|| "attachment".to_string())
}

fn external_editor_attachment_status_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    external_editor_attachment_feedback_suffix(summary)
}

fn external_editor_attachment_activity_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    external_editor_attachment_feedback_suffix(summary)
}

fn external_editor_attachment_feedback_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    match (summary.detached.len(), summary.reordered) {
        (0, false) => String::new(),
        (0, true) => " · reordered attachments".to_string(),
        (1, false) => format!(
            " · detached {}",
            attachment_preview_status_label(&summary.detached[0])
        ),
        (count, false) => format!(" · detached {count} attachments"),
        (1, true) => format!(
            " · detached {} and reordered remaining",
            attachment_preview_status_label(&summary.detached[0])
        ),
        (count, true) => format!(" · detached {count} attachments and reordered remaining"),
    }
}

fn preview_path_tail(path: &str) -> String {
    if let Some(segment) = remote_attachment_tail_segment(path) {
        return segment;
    }
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn looks_like_local_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg")
    )
}

fn is_remote_attachment_url(path: &str) -> bool {
    matches!(path.trim(), value if value.starts_with("http://") || value.starts_with("https://"))
}

fn remote_attachment_tail_segment(path: &str) -> Option<String> {
    let (_, remainder) = path.trim().split_once("://")?;
    let path = remainder
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or_default();
    let trimmed = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/');
    (!trimmed.is_empty()).then(|| {
        trimmed
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or(trimmed)
            .to_string()
    })
}

fn remote_attachment_file_name(path: &str) -> Option<String> {
    remote_attachment_tail_segment(path).filter(|segment| !segment.is_empty())
}

async fn load_composer_file(
    requested_path: &str,
    ctx: &ToolExecutionContext,
) -> Result<LoadedComposerFile> {
    let resolved_path = resolve_tool_path_against_workspace_root(
        requested_path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_read_allowed(&resolved_path)?;
    let bytes = fs::read(&resolved_path).await?;
    Ok(LoadedComposerFile {
        requested_path: requested_path.to_string(),
        file_name: resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string),
        mime_type: sniff_composer_file_mime(&bytes, &resolved_path).map(str::to_string),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

fn sniff_composer_file_mime(bytes: &[u8], path: &Path) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

fn sniff_remote_image_mime(path: &str) -> Option<&'static str> {
    match remote_attachment_extension(path)?.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

fn sniff_remote_file_mime(path: &str) -> Option<&'static str> {
    match remote_attachment_extension(path)?.as_str() {
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

fn remote_attachment_extension(path: &str) -> Option<String> {
    let segment = remote_attachment_tail_segment(path)?;
    segment
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .and_then(|extension| {
            let normalized = extension.trim();
            (!normalized.is_empty()).then_some(normalized.to_ascii_lowercase())
        })
}

fn resolve_external_editor_command() -> Result<Vec<String>> {
    let configured = env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or_else(|| anyhow!("Cannot open external editor: set $VISUAL or $EDITOR."))?;
    let command = shlex::split(&configured)
        .filter(|segments| !segments.is_empty())
        .ok_or_else(|| anyhow!("Failed to parse external editor command: {configured}"))?;
    Ok(command)
}

fn run_external_editor(seed: &str, editor_command: &[String]) -> Result<String> {
    let file = NamedTempFile::new().context("create external editor temp file")?;
    stdfs::write(file.path(), seed).context("seed external editor temp file")?;

    let (program, args) = editor_command
        .split_first()
        .ok_or_else(|| anyhow!("External editor command is empty"))?;
    let status = ProcessCommand::new(program)
        .args(args)
        .arg(file.path())
        .status()
        .with_context(|| format!("launch external editor `{program}`"))?;
    if !status.success() {
        return Err(anyhow!(
            "External editor exited with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }

    stdfs::read_to_string(file.path()).context("read external editor output")
}

fn queued_command_preview(command: &RuntimeCommand) -> String {
    match command {
        RuntimeCommand::Prompt { message, .. } => {
            let preview = message_operator_text(message);
            format!("running prompt: {}", state::preview_text(&preview, 40))
        }
        RuntimeCommand::Steer { message, .. } => {
            format!("applying steer: {}", state::preview_text(message, 40))
        }
    }
}

fn format_side_question_inspector(outcome: &SideQuestionOutcome) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("Side Question"),
        InspectorEntry::field("Command", format!("/btw {}", outcome.question)),
        InspectorEntry::section("Answer"),
        InspectorEntry::Plain(outcome.response.clone()),
    ]
}

fn pending_control_kind_label(kind: crate::interaction::PendingControlKind) -> &'static str {
    match kind {
        crate::interaction::PendingControlKind::Prompt => "prompt",
        crate::interaction::PendingControlKind::Steer => "steer",
    }
}

fn composer_has_prompt_content(state: &TuiState) -> bool {
    !state.input.trim().is_empty() || !state.draft_attachments.is_empty()
}

fn composer_requires_prompt_submission(state: &TuiState) -> bool {
    state.draft_attachments.iter().any(|attachment| {
        !matches!(
            attachment.kind,
            ComposerDraftAttachmentKind::LargePaste { .. }
        )
    })
}

fn composer_uses_image_input(state: &TuiState) -> bool {
    state.draft_attachments.iter().any(|attachment| {
        matches!(
            attachment.kind,
            ComposerDraftAttachmentKind::LocalImage { .. }
                | ComposerDraftAttachmentKind::RemoteImage { .. }
        )
    })
}

fn build_startup_inspector(session: &state::SessionSummary) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Ready"),
        InspectorEntry::field("workspace", session.workspace_name.clone()),
        InspectorEntry::field("session ref", session.active_session_ref.clone()),
        InspectorEntry::field("agent session", session.root_agent_session_id.clone()),
        InspectorEntry::field(
            "model",
            format!("{} / {}", session.provider_label, session.model),
        ),
        InspectorEntry::field(
            "image input",
            if session.supports_image_input {
                "enabled"
            } else {
                "disabled"
            },
        ),
        InspectorEntry::field(
            "root",
            state::preview_text(&session.workspace_root.display().to_string(), 56),
        ),
        InspectorEntry::section("Next"),
        InspectorEntry::collection("/help [query]", Some("browse commands")),
        InspectorEntry::collection("/statusline", Some("choose footer items")),
        InspectorEntry::collection("/thinking [level]", Some("pick or set model effort")),
        InspectorEntry::collection("/theme [name]", Some("pick or set tui theme")),
        InspectorEntry::collection("/details", Some("toggle tool details")),
        InspectorEntry::collection(
            "/permissions [mode]",
            Some("inspect or switch sandbox mode"),
        ),
        InspectorEntry::collection("/queue", Some("browse pending prompts and steers")),
        InspectorEntry::collection("/sessions", Some("browse history")),
        InspectorEntry::collection("/agent_sessions", Some("inspect or resume agents")),
        InspectorEntry::collection("/spawn_task <role> <prompt>", Some("launch child agent")),
        InspectorEntry::collection("/new", Some("start fresh without deleting history")),
        InspectorEntry::section("Environment"),
        InspectorEntry::field(
            "store",
            format!(
                "{} ({} sessions)",
                session.store_label, session.stored_session_count
            ),
        ),
        InspectorEntry::field("permissions", session.permission_mode.as_str()),
        InspectorEntry::field("sandbox", session.sandbox_summary.clone()),
        InspectorEntry::field(
            "tools",
            format!(
                "{} local / {} mcp",
                session.startup_diagnostics.local_tool_count,
                session.startup_diagnostics.mcp_tool_count
            ),
        ),
        InspectorEntry::field(
            "plugins",
            format!(
                "{} enabled / {} total",
                session.startup_diagnostics.enabled_plugin_count,
                session.startup_diagnostics.total_plugin_count
            ),
        ),
        InspectorEntry::section("Git"),
        if !session.host_process_surfaces_allowed {
            InspectorEntry::field("branch", "disabled while host subprocesses are blocked")
        } else if session.git.available {
            InspectorEntry::field("branch", session.git.branch.clone())
        } else {
            InspectorEntry::field("branch", "unavailable")
        },
        if !session.host_process_surfaces_allowed {
            InspectorEntry::field("dirty", "unavailable while host subprocesses are blocked")
        } else {
            InspectorEntry::field(
                "dirty",
                format!(
                    "staged {}  modified {}  untracked {}",
                    session.git.staged, session.git.modified, session.git.untracked
                ),
            )
        },
        InspectorEntry::section("Diagnostics"),
        InspectorEntry::field(
            "mcp servers",
            session.startup_diagnostics.mcp_servers.len().to_string(),
        ),
    ];
    if let Some(warning) = &session.store_warning {
        lines.push(InspectorEntry::Muted(format!(
            "warning: {}",
            state::preview_text(warning, 72)
        )));
    }
    if !session.startup_diagnostics.warnings.is_empty() {
        lines.push(InspectorEntry::Muted(format!(
            "warning: {}",
            state::preview_text(&session.startup_diagnostics.warnings.join(" | "), 80)
        )));
    }
    if !session.startup_diagnostics.diagnostics.is_empty() {
        lines.push(InspectorEntry::Plain(format!(
            "diagnostic: {}",
            state::preview_text(&session.startup_diagnostics.diagnostics.join(" | "), 80)
        )));
    }
    lines
}

fn build_permissions_inspector(
    snapshot: &SessionStartupSnapshot,
    turn_grants: &PermissionProfile,
    session_grants: &PermissionProfile,
) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Permissions"),
        InspectorEntry::field("mode", snapshot.permission_mode.as_str()),
        InspectorEntry::field("default sandbox", snapshot.default_sandbox_summary.clone()),
        InspectorEntry::field("effective sandbox", snapshot.sandbox_summary.clone()),
        InspectorEntry::field(
            "host subprocesses",
            if snapshot.host_process_surfaces_allowed {
                "enabled"
            } else {
                "blocked until danger-full-access or a real sandbox backend is available"
            },
        ),
        InspectorEntry::section("Modes"),
        InspectorEntry::Command("/permissions default".to_string()),
        InspectorEntry::Command("/permissions danger-full-access".to_string()),
        InspectorEntry::section("Additional Grants"),
        InspectorEntry::field("turn", permission_profile_summary(turn_grants)),
        InspectorEntry::field("session", permission_profile_summary(session_grants)),
    ];
    if snapshot.permission_mode != SessionPermissionMode::Default {
        lines.push(InspectorEntry::Muted(
            "note: returning to `/permissions default` keeps request_permissions grants, but reapplies the configured base sandbox.".to_string(),
        ));
    }
    lines
}

fn permission_profile_summary(profile: &PermissionProfile) -> String {
    let mut entries = Vec::new();
    if !profile.read_roots.is_empty() {
        entries.push(format!(
            "read {}",
            state::preview_text(&profile.read_roots.join(", "), 56)
        ));
    }
    if !profile.write_roots.is_empty() {
        entries.push(format!(
            "write {}",
            state::preview_text(&profile.write_roots.join(", "), 56)
        ));
    }
    if profile.network_full {
        entries.push("network full".to_string());
    }
    if !profile.network_domains.is_empty() {
        entries.push(format!(
            "domains {}",
            state::preview_text(&profile.network_domains.join(", "), 56)
        ));
    }
    if entries.is_empty() {
        "none".to_string()
    } else {
        entries.join(" · ")
    }
}

fn build_command_error_view(input: &str, message: &str) -> Vec<InspectorEntry> {
    let mut lines = message
        .lines()
        .map(|line| InspectorEntry::Plain(line.to_string()))
        .collect::<Vec<_>>();
    let query = input
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .filter(|query| !query.is_empty());
    let palette = command_palette_lines_for(query);
    if !palette.is_empty() {
        lines.push(InspectorEntry::Empty);
        lines.extend(palette);
    }
    lines
}

fn build_mcp_prompt_inspector(loaded: &LoadedMcpPrompt) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Prompt"),
        InspectorEntry::field("server", loaded.server_name.clone()),
        InspectorEntry::field("prompt", loaded.prompt_name.clone()),
        InspectorEntry::field("arguments", loaded.arguments_summary.clone()),
    ]
}

fn build_mcp_resource_inspector(loaded: &LoadedMcpResource) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Resource"),
        InspectorEntry::field("server", loaded.server_name.clone()),
        InspectorEntry::field("uri", loaded.uri.clone()),
        InspectorEntry::field("mime", loaded.mime_summary.clone()),
    ]
}

fn live_task_wait_notice_entry(outcome: &LiveTaskWaitOutcome) -> TranscriptEntry {
    let headline = format!("Background task {} finished", outcome.task_id);
    let details = live_task_wait_notice_details(outcome);
    match outcome.status {
        AgentStatus::Completed => TranscriptEntry::success_summary_details(headline, details),
        AgentStatus::Failed => TranscriptEntry::error_summary_details(headline, details),
        AgentStatus::Cancelled => TranscriptEntry::warning_summary_details(headline, details),
        _ => TranscriptEntry::shell_summary_details(headline, details),
    }
}

fn live_task_wait_notice_details(outcome: &LiveTaskWaitOutcome) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("status {}", outcome.status),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("summary {}", state::preview_text(&outcome.summary, 96)),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: "next enter steer / tab queue / /task inspect".to_string(),
            continuation: false,
        },
    ];
    if !outcome.claimed_files.is_empty() {
        details.push(TranscriptShellDetail::Raw {
            text: format!(
                "claimed files {}",
                state::preview_text(&outcome.claimed_files.join(", "), 96)
            ),
            continuation: false,
        });
    }
    if !outcome.remaining_live_tasks.is_empty() {
        details.push(TranscriptShellDetail::Raw {
            text: format!(
                "still running {}",
                state::preview_text(
                    &outcome
                        .remaining_live_tasks
                        .iter()
                        .map(|task| format!("{} ({}, {})", task.task_id, task.role, task.status))
                        .collect::<Vec<_>>()
                        .join(", "),
                    96
                )
            ),
            continuation: false,
        });
    }
    details
}

fn live_task_wait_ui_toast_tone(outcome: &LiveTaskWaitOutcome) -> ToastTone {
    match outcome.status {
        AgentStatus::Completed => ToastTone::Success,
        AgentStatus::Failed => ToastTone::Error,
        AgentStatus::Cancelled => ToastTone::Warning,
        _ => ToastTone::Info,
    }
}

fn live_task_wait_toast_message(outcome: &LiveTaskWaitOutcome, turn_running: bool) -> String {
    let next_step = if turn_running {
        "enter steer / tab queue / /task inspect"
    } else {
        "model follow-up queued / /task inspect"
    };
    let mut parts = vec![
        format!("task {} {}", outcome.task_id, outcome.status),
        state::preview_text(&outcome.summary, 64),
    ];
    if !outcome.remaining_live_tasks.is_empty() {
        parts.push(format!(
            "{} still running",
            outcome.remaining_live_tasks.len()
        ));
    }
    parts.push(next_step.to_string());
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
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
    use agent::types::{AgentStatus, Message, MessageId, MessagePart, MessageRole};
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
                InspectorEntry::CollectionItem { primary, secondary } => secondary
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
            lines.iter().any(|line| {
                line.starts_with("/spawn_task <role> <prompt>  launch child agent")
            })
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
            task_id: "task_123".to_string(),
            status: AgentStatus::Completed,
            summary: "done".to_string(),
            agent_id: "agent_123".to_string(),
            claimed_files: Vec::new(),
            remaining_live_tasks: vec![LiveTaskSummary {
                agent_id: "agent_456".to_string(),
                task_id: "task_456".to_string(),
                role: "reviewer".to_string(),
                status: AgentStatus::Running,
                session_ref: "session_456".to_string(),
                agent_session_ref: "agent-session-456".to_string(),
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
        let first_answer =
            Message::assistant("answer one").with_message_id(MessageId::from("msg-2"));
        let second_prompt = Message::user("second").with_message_id(MessageId::from("msg-3"));
        let second_answer =
            Message::assistant("answer two").with_message_id(MessageId::from("msg-4"));
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
        let steer =
            Message::system("prefer terse answers").with_message_id(MessageId::from("msg-1"));
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
}
