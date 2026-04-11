mod approval;
mod commands;
mod composer;
mod history;
mod history_rollback;
mod input_history;
mod interaction_keys;
mod observer;
mod operator_support;
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
use operator_support::*;
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

#[cfg(test)]
mod tests;
