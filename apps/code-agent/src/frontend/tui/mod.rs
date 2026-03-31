mod approval;
mod commands;
mod history;
mod input_history;
mod observer;
mod paste_burst;
mod render;
mod state;

use crate::backend::{
    CodeAgentSession, LiveTaskControlAction, LiveTaskMessageAction, LiveTaskWaitOutcome,
    LoadedMcpPrompt, LoadedMcpResource, SessionOperation, SessionOperationAction,
    SessionOperationOutcome, SessionPermissionMode, SessionStartupSnapshot, UserInputPrompt,
    preview_id,
};
use crate::config::persist_tui_theme_selection;
use crate::statusline::status_line_fields;
use crate::theme::{ThemeCatalog, active_theme_id, install_theme_catalog, set_active_theme};
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
pub(crate) use state::SharedUiState;
use state::{
    ComposerDraftAttachmentKind, ComposerDraftAttachmentState, ComposerSubmission, InspectorEntry,
    TuiState,
};

use agent::RuntimeCommand;
use agent::tools::{
    GrantedPermissionResponse, PermissionGrantScope, RequestPermissionProfile,
    ToolExecutionContext, UserInputAnswer, UserInputResponse, load_tool_image,
    resolve_tool_path_against_workspace_root,
};
use agent::types::{Message, MessagePart, MessageRole, message_operator_text};
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
    session: CodeAgentSession,
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
        session: CodeAgentSession,
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
            .session
            .end_session(Some("operator_exit".to_string()))
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
            self.flush_due_paste_burst();
            self.maybe_finish_turn().await?;
            self.apply_backend_events();
            self.maybe_finish_operator_task().await?;
            self.sync_runtime_control_state();
            let permission_request_prompt = self.session.permission_request_prompt();
            let user_input_prompt = self.session.user_input_prompt();
            self.sync_user_input_prompt(user_input_prompt.as_ref());

            let snapshot = self.ui_state.snapshot();
            let approval = self.session.approval_prompt();
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
                    self.handle_explicit_paste(&text);
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
                    if self.handle_paste_burst_key(key) {
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
                                snapshot.turn_running,
                                KeyCode::Tab,
                            ) {
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
                                snapshot.turn_running,
                                KeyCode::Enter,
                            ) {
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

    fn apply_command_completion(&mut self, backwards: bool) -> bool {
        let snapshot = self.ui_state.snapshot();
        let Some((input, index)) = cycle_slash_command(
            &snapshot.input,
            snapshot.command_completion_index,
            backwards,
        ) else {
            return false;
        };
        self.ui_state.mutate(|state| {
            state.replace_input(input);
            state.command_completion_index = index;
        });
        true
    }

    fn navigate_input_history(&mut self, backwards: bool) -> bool {
        let mut navigated = false;
        self.ui_state.mutate(|state| {
            navigated = state.browse_input_history(backwards);
        });
        navigated
    }

    fn flush_due_paste_burst(&mut self) {
        let now = Instant::now();
        match self.paste_burst.flush_if_due(now) {
            FlushResult::Paste(text) => self.insert_pasted_text(&text),
            FlushResult::Typed(ch) => self.ui_state.mutate(|state| state.push_input_char(ch)),
            FlushResult::None => {}
        }
    }

    fn handle_explicit_paste(&mut self, text: &str) {
        self.insert_pasted_text(text);
        self.paste_burst.clear_after_explicit_paste();
    }

    fn handle_paste_burst_key(&mut self, key: KeyEvent) -> bool {
        let now = Instant::now();
        if let KeyCode::Char(ch) = key.code
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            if ch.is_ascii() {
                match self.paste_burst.on_plain_char(ch, now) {
                    CharDecision::RetainFirstChar => return true,
                    CharDecision::BeginBufferFromPending | CharDecision::BufferAppend => {
                        self.paste_burst.append_char_to_buffer(ch, now);
                        return true;
                    }
                }
            } else if self.paste_burst.on_plain_char_no_hold(now) {
                self.paste_burst.append_char_to_buffer(ch, now);
                return true;
            }
            return false;
        }

        if matches!(key.code, KeyCode::Enter) {
            if self.paste_burst.append_newline_if_active(now) {
                return true;
            }
            if self
                .paste_burst
                .newline_should_insert_instead_of_submit(now)
            {
                self.insert_pasted_text("\n");
                self.paste_burst.clear_window_after_non_char();
                return true;
            }
        }

        if let Some(flushed) = self.paste_burst.flush_before_modified_input() {
            self.insert_pasted_text(&flushed);
        }
        self.paste_burst.clear_window_after_non_char();
        false
    }

    fn insert_pasted_text(&mut self, text: &str) {
        if text.is_empty() || !self.composer_accepts_text_input() {
            return;
        }
        let large_paste = text.chars().count() > LARGE_PASTE_CHAR_THRESHOLD;
        self.ui_state.mutate(|state| {
            if large_paste {
                let placeholder = state.push_large_paste(text);
                state.status = format!("Collapsed large paste into {placeholder}");
                state.push_activity(format!(
                    "collapsed pasted payload into {}",
                    state::preview_text(&placeholder, 24)
                ));
            } else {
                state.push_input_str(text);
            }
        });
    }

    fn stash_composer_draft_on_ctrl_c(&mut self) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut stashed = false;
        self.ui_state.mutate(|state| {
            stashed = !state.input.is_empty() || !state.draft_attachments.is_empty();
            if stashed {
                let _ = state.stash_current_input_draft();
                state.clear_input();
                state.status = "Cleared draft; press Up to restore it".to_string();
                state.push_activity("stashed current draft for history recall");
            }
        });
        stashed
    }

    fn kill_input_to_end(&mut self) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut killed = false;
        self.ui_state
            .mutate(|state| killed = state.kill_input_to_end());
        killed
    }

    fn yank_kill_buffer(&mut self) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut yanked = false;
        self.ui_state
            .mutate(|state| yanked = state.yank_kill_buffer());
        yanked
    }

    async fn launch_external_editor(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        if !self.composer_accepts_text_input() {
            return Ok(());
        }

        if let Some(flushed) = self.paste_burst.flush_before_modified_input() {
            self.insert_pasted_text(&flushed);
        }
        self.paste_burst.clear_window_after_non_char();

        let editor_command = match resolve_external_editor_command() {
            Ok(command) => command,
            Err(error) => {
                self.ui_state.mutate(|state| {
                    state.status = error.to_string();
                    state.push_activity("external editor unavailable");
                });
                return Ok(());
            }
        };
        let seed = self.ui_state.snapshot().external_editor_seed_text();

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        )?;
        terminal.show_cursor()?;

        let edit_result = run_external_editor(&seed, &editor_command);

        enable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste
        )?;
        terminal.clear()?;

        match edit_result {
            Ok(text) => {
                self.ui_state.mutate(|state| {
                    let summary = state.apply_external_edit(text.trim_end().to_string());
                    let status_suffix = external_editor_attachment_status_suffix(&summary);
                    let activity_suffix = external_editor_attachment_activity_suffix(&summary);
                    state.status =
                        format!("Replaced composer text from external editor{status_suffix}");
                    state.push_activity(format!(
                        "updated draft from external editor{activity_suffix}"
                    ));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("external editor", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to open editor: {message}");
                    state.push_activity(format!(
                        "external editor failed: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }

        Ok(())
    }

    fn composer_accepts_text_input(&self) -> bool {
        if self.session.approval_prompt().is_some()
            || self.session.permission_request_prompt().is_some()
        {
            return false;
        }

        let snapshot = self.ui_state.snapshot();
        if snapshot.pending_control_picker.is_some()
            || snapshot.statusline_picker.is_some()
            || snapshot.thinking_effort_picker.is_some()
            || snapshot.theme_picker.is_some()
            || snapshot.history_rollback.is_some()
        {
            return false;
        }

        self.active_user_input
            .as_ref()
            .is_none_or(|flow| flow.collecting_other_note)
    }

    fn move_input_cursor_horizontal(&mut self, backwards: bool) -> bool {
        let mut moved = false;
        self.ui_state.mutate(|state| {
            moved = if backwards {
                state.move_input_cursor_left()
            } else {
                state.move_input_cursor_right()
            };
        });
        moved
    }

    fn move_input_cursor_boundary(&mut self, backwards: bool) -> bool {
        let mut moved = false;
        self.ui_state.mutate(|state| {
            moved = if backwards {
                state.move_input_cursor_home()
            } else {
                state.move_input_cursor_end()
            };
        });
        moved
    }

    fn move_input_cursor_vertical(&mut self, backwards: bool) -> bool {
        let mut moved = false;
        self.ui_state.mutate(|state| {
            moved = state.move_input_cursor_vertical(backwards);
        });
        moved
    }

    fn move_selected_row_attachment(&mut self, backwards: bool) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut moved = false;
        self.ui_state.mutate(|state| {
            moved = if backwards {
                state.select_previous_row_attachment()
            } else {
                state.select_next_row_attachment()
            };
            if moved {
                if let Some(preview) = state.selected_row_attachment_preview() {
                    state.status =
                        format!("Selected {}", attachment_preview_status_label(&preview));
                } else {
                    state.status = "Returned to draft editing".to_string();
                }
            }
        });
        moved
    }

    fn remove_selected_row_attachment(&mut self) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut removed = false;
        self.ui_state.mutate(|state| {
            let removed_preview = state.selected_row_attachment_preview();
            if let Some(attachment) = state.remove_selected_row_attachment() {
                let label = removed_attachment_status_label(removed_preview.as_ref(), &attachment);
                state.status = format!("Detached {label}");
                state.push_activity(format!("detached {label}"));
                removed = true;
            }
        });
        removed
    }

    fn move_input_cursor_home(&mut self) -> bool {
        let mut moved = false;
        self.ui_state
            .mutate(|state| moved = state.move_input_cursor_home());
        moved
    }

    fn move_input_cursor_end(&mut self) -> bool {
        let mut moved = false;
        self.ui_state
            .mutate(|state| moved = state.move_input_cursor_end());
        moved
    }

    fn record_submitted_input(&mut self, input: &str) {
        let workspace_root = self.session.workspace_root().to_path_buf();
        let mut persisted = None;
        self.ui_state.mutate(|state| {
            let _ = state.record_local_input_history(input);
            if state.record_input_history(input) {
                persisted = Some(state.input_history().to_vec());
            }
        });
        if let Some(entries) = persisted {
            input_history::persist_input_history(&workspace_root, &entries);
        }
    }

    fn record_submitted_prompt(&mut self, submission: &ComposerSubmission) {
        let workspace_root = self.session.workspace_root().to_path_buf();
        let mut persisted = None;
        self.ui_state.mutate(|state| {
            let _ = state.record_local_input_draft(submission.local_history_draft.clone());
            if state.record_input_history(&submission.persisted_history_text) {
                persisted = Some(state.input_history().to_vec());
            }
        });
        if let Some(entries) = persisted {
            input_history::persist_input_history(&workspace_root, &entries);
        }
    }

    fn composer_attachment_context(&self) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: self.session.workspace_root().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    async fn attach_composer_image(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            self.ui_state.mutate(|state| {
                state.status = "Usage: /image <path-or-url>".to_string();
                state.push_activity("invalid /image invocation");
            });
            return;
        }

        if is_remote_attachment_url(path) {
            let attachment = ComposerDraftAttachmentState {
                placeholder: None,
                kind: ComposerDraftAttachmentKind::RemoteImage {
                    requested_url: path.to_string(),
                    part: MessagePart::ImageUrl {
                        url: path.to_string(),
                        mime_type: sniff_remote_image_mime(path).map(str::to_string),
                    },
                },
            };
            self.ui_state.mutate(|state| {
                if state.push_row_attachment(attachment) {
                    state.status = format!("Attached image {}", preview_path_tail(path));
                    state.push_activity(format!("attached image {}", path));
                } else {
                    state.status = format!("Image {} is already attached", preview_path_tail(path));
                    state.push_activity(format!("image already attached: {}", path));
                }
            });
            return;
        }

        match load_tool_image(path, &self.composer_attachment_context()).await {
            Ok(image) => {
                let attachment = ComposerDraftAttachmentState {
                    placeholder: Some("[Image #1]".to_string()),
                    kind: ComposerDraftAttachmentKind::LocalImage {
                        requested_path: path.to_string(),
                        part: image.message_part(),
                    },
                };
                self.ui_state.mutate(|state| {
                    if state.push_inline_attachment(attachment) {
                        state.status = format!("Attached image {}", preview_path_tail(path));
                        state.push_activity(format!("attached image {}", path));
                    } else {
                        state.status =
                            format!("Image {} is already attached", preview_path_tail(path));
                        state.push_activity(format!("image already attached: {}", path));
                    }
                });
            }
            Err(error) => {
                let error = anyhow::Error::from(error);
                let message = summarize_nonfatal_error("attach image", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to attach image: {message}");
                    state.push_activity(format!(
                        "failed to attach image: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    async fn attach_composer_file(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            self.ui_state.mutate(|state| {
                state.status = "Usage: /file <path-or-url>".to_string();
                state.push_activity("invalid /file invocation");
            });
            return;
        }

        if is_remote_attachment_url(path) {
            let attachment = ComposerDraftAttachmentState {
                placeholder: None,
                kind: ComposerDraftAttachmentKind::RemoteFile {
                    requested_url: path.to_string(),
                    part: MessagePart::File {
                        file_name: remote_attachment_file_name(path),
                        mime_type: sniff_remote_file_mime(path).map(str::to_string),
                        data_base64: None,
                        uri: Some(path.to_string()),
                    },
                },
            };
            self.ui_state.mutate(|state| {
                if state.push_row_attachment(attachment) {
                    state.status = format!("Attached file {}", preview_path_tail(path));
                    state.push_activity(format!("attached file {}", path));
                } else {
                    state.status = format!("File {} is already attached", preview_path_tail(path));
                    state.push_activity(format!("file already attached: {}", path));
                }
            });
            return;
        }

        match load_composer_file(path, &self.composer_attachment_context()).await {
            Ok(file) => {
                let attachment = ComposerDraftAttachmentState {
                    placeholder: Some("[File #1]".to_string()),
                    kind: ComposerDraftAttachmentKind::LocalFile {
                        requested_path: file.requested_path.clone(),
                        part: MessagePart::File {
                            file_name: file.file_name.clone(),
                            mime_type: file.mime_type.clone(),
                            data_base64: Some(file.data_base64),
                            uri: Some(file.requested_path.clone()),
                        },
                    },
                };
                self.ui_state.mutate(|state| {
                    if state.push_inline_attachment(attachment) {
                        state.status = format!("Attached file {}", preview_path_tail(path));
                        state.push_activity(format!("attached file {}", path));
                    } else {
                        state.status =
                            format!("File {} is already attached", preview_path_tail(path));
                        state.push_activity(format!("file already attached: {}", path));
                    }
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("attach file", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to attach file: {message}");
                    state.push_activity(format!(
                        "failed to attach file: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    fn detach_composer_attachment(&mut self, index: Option<usize>) {
        self.ui_state.mutate(|state| {
            let preview = state.row_attachment_preview(index);
            match state.remove_row_attachment(index) {
                Some(attachment) => {
                    let label = removed_attachment_status_label(preview.as_ref(), &attachment);
                    state.status = format!("Detached {label}");
                    state.push_activity(format!("detached {label}"));
                }
                None => {
                    state.status = match index {
                        Some(index) => format!("No composer attachment {index}"),
                        None => "No composer attachment to detach".to_string(),
                    };
                    state.push_activity("no composer attachment removed");
                }
            }
        });
    }

    fn move_composer_attachment(&mut self, from: usize, to: usize) {
        self.ui_state.mutate(|state| {
            if state.move_row_attachment(from, to) {
                state.status = format!("Moved attachment #{from} to #{to}");
                state.push_activity(format!("moved attachment #{from} -> #{to}"));
            } else {
                state.status = format!("Unable to move attachment #{from} to #{to}");
                state.push_activity("attachment move rejected");
            }
        });
    }

    fn move_command_selection(&mut self, backwards: bool) -> bool {
        let snapshot = self.ui_state.snapshot();
        let Some(index) = move_slash_command_selection(
            &snapshot.input,
            snapshot.command_completion_index,
            backwards,
        ) else {
            return false;
        };
        self.ui_state
            .mutate(|state| state.command_completion_index = index);
        true
    }

    fn handle_approval_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.session.approval_prompt() else {
            return false;
        };
        if let Some(decision) = approval_decision_for_key(key) {
            let approved = matches!(decision, crate::backend::ApprovalDecision::Approve);
            if self.session.resolve_approval(decision) {
                self.ui_state.mutate(|state| {
                    if approved {
                        state.status = format!("Approved {}", prompt.tool_name);
                        state.push_activity(format!("approved {}", prompt.tool_name));
                    } else {
                        state.status = format!("Denied {}", prompt.tool_name);
                        state.push_activity(format!("denied {}", prompt.tool_name));
                    }
                });
            }
            return true;
        }
        true
    }

    fn handle_permission_request_key(&mut self, key: KeyEvent) -> bool {
        let Some(prompt) = self.session.permission_request_prompt() else {
            return false;
        };
        let response = match key.code {
            KeyCode::Char('y') => Some(GrantedPermissionResponse {
                permissions: prompt.requested_normalized.clone(),
                scope: PermissionGrantScope::Turn,
            }),
            KeyCode::Char('a') => Some(GrantedPermissionResponse {
                permissions: prompt.requested_normalized.clone(),
                scope: PermissionGrantScope::Session,
            }),
            KeyCode::Char('n') | KeyCode::Esc => Some(GrantedPermissionResponse {
                permissions: agent::tools::GrantedPermissionProfile::default(),
                scope: PermissionGrantScope::Turn,
            }),
            _ => None,
        };
        if let Some(response) = response {
            let granted = !response.permissions.is_empty();
            let scope = response.scope;
            if self.session.resolve_permission_request(response) {
                self.ui_state.mutate(|state| {
                    if granted {
                        let scope_label = match scope {
                            PermissionGrantScope::Turn => "turn",
                            PermissionGrantScope::Session => "session",
                        };
                        state.status =
                            format!("Granted additional permissions for the {scope_label}");
                        state.push_activity(format!(
                            "granted additional permissions for the {scope_label}"
                        ));
                    } else {
                        state.status = "Denied additional permissions".to_string();
                        state.push_activity("denied additional permissions");
                    }
                });
            }
            return true;
        }
        true
    }

    fn handle_user_input_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            return false;
        }
        let Some(prompt) = self.session.user_input_prompt() else {
            return false;
        };
        self.sync_user_input_prompt(Some(&prompt));
        let Some(flow) = self.active_user_input.as_mut() else {
            return true;
        };
        let Some(question) = prompt.questions.get(flow.current_question) else {
            return true;
        };

        if flow.collecting_other_note {
            match key.code {
                KeyCode::Enter => {
                    let note = self.ui_state.take_input();
                    let note = note.trim();
                    if note.is_empty() {
                        self.ui_state.mutate(|state| {
                            state.status =
                                format!("Other note for {} cannot be empty", question.header);
                            state.push_activity(format!(
                                "rejected empty other note for {}",
                                question.id
                            ));
                        });
                        return true;
                    }
                    flow.answers.insert(
                        question.id.clone(),
                        UserInputAnswer {
                            answers: vec!["Other".to_string(), format!("user_note: {note}")],
                        },
                    );
                    flow.collecting_other_note = false;
                    self.advance_user_input_flow(&prompt);
                    true
                }
                KeyCode::Esc => {
                    flow.collecting_other_note = false;
                    self.ui_state.mutate(|state| {
                        state.clear_input();
                        state.status = format!("Returned to {} options", question.header);
                        state.push_activity(format!("returned to {} options", question.id));
                    });
                    true
                }
                KeyCode::Up => {
                    self.ui_state.mutate(|state| {
                        if !state.browse_input_history(true) {
                            let _ = state.move_input_cursor_vertical(true)
                                || state.move_input_cursor_home();
                        }
                    });
                    true
                }
                KeyCode::Down => {
                    self.ui_state.mutate(|state| {
                        if !state.browse_input_history(false) {
                            let _ = state.move_input_cursor_vertical(false)
                                || state.move_input_cursor_end();
                        }
                    });
                    true
                }
                KeyCode::Left => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_left();
                    });
                    true
                }
                KeyCode::Right => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_right();
                    });
                    true
                }
                KeyCode::Home => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_home();
                    });
                    true
                }
                KeyCode::End => {
                    self.ui_state.mutate(|state| {
                        let _ = state.move_input_cursor_end();
                    });
                    true
                }
                KeyCode::Backspace => {
                    self.ui_state.mutate(|state| {
                        state.pop_input_char();
                    });
                    true
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = self.kill_input_to_end();
                    true
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let _ = self.yank_kill_buffer();
                    true
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.ui_state.mutate(|state| {
                        state.push_input_char(ch);
                    });
                    true
                }
                _ => true,
            }
        } else {
            match key.code {
                KeyCode::Esc => {
                    if self
                        .session
                        .cancel_user_input("operator cancelled user input request")
                    {
                        self.active_user_input = None;
                        self.ui_state.mutate(|state| {
                            state.clear_input();
                            state.status = "Cancelled user input request".to_string();
                            state.push_activity("cancelled user input request");
                        });
                    }
                    true
                }
                KeyCode::Char(ch) if ch.is_ascii_digit() => {
                    let Some(digit) = ch.to_digit(10) else {
                        return true;
                    };
                    if digit == 0 {
                        flow.collecting_other_note = true;
                        self.ui_state.mutate(|state| {
                            state.clear_input();
                            state.status =
                                format!("Provide an alternate answer for {}", question.header);
                            state.push_activity(format!(
                                "collecting other note for {}",
                                question.id
                            ));
                        });
                        return true;
                    }
                    let option_index = digit as usize - 1;
                    if let Some(option) = question.options.get(option_index) {
                        flow.answers.insert(
                            question.id.clone(),
                            UserInputAnswer {
                                answers: vec![option.label.clone()],
                            },
                        );
                        self.advance_user_input_flow(&prompt);
                    } else {
                        self.ui_state.mutate(|state| {
                            state.status = format!("{} has no option {}", question.header, digit);
                            state.push_activity(format!(
                                "invalid selection {} for {}",
                                digit, question.id
                            ));
                        });
                    }
                    true
                }
                _ => true,
            }
        }
    }

    fn handle_statusline_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.statusline_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_statusline_picker(true);
                });
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_statusline_picker(false);
                });
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.statusline_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.statusline_picker.as_mut() {
                        picker.selected = status_line_fields().len().saturating_sub(1);
                    }
                });
                true
            }
            KeyCode::Char(' ') => {
                self.ui_state.mutate(|state| {
                    if let Some((field, enabled)) = state.toggle_selected_statusline_field() {
                        let label = status_line_fields()
                            .iter()
                            .find(|spec| spec.field == field)
                            .map(|spec| spec.label)
                            .unwrap_or("field");
                        state.status = format!(
                            "Status line {} {}",
                            label,
                            if enabled { "enabled" } else { "hidden" }
                        );
                        state.push_activity(format!(
                            "status line {} {}",
                            label,
                            if enabled { "enabled" } else { "hidden" }
                        ));
                    }
                });
                true
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.ui_state.mutate(|state| {
                    state.close_statusline_picker();
                    state.status = "Closed status line picker".to_string();
                    state.push_activity("closed status line picker");
                });
                true
            }
            _ => false,
        }
    }

    fn handle_pending_control_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.pending_control_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_pending_control_picker(true);
                });
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_pending_control_picker(false);
                });
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.pending_control_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.pending_control_picker.as_mut() {
                        picker.selected = state.pending_controls.len().saturating_sub(1);
                    }
                });
                true
            }
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || matches!(key.code, KeyCode::Delete | KeyCode::Backspace) =>
            {
                if let Some(selected) = snapshot.selected_pending_control() {
                    match self.session.remove_pending_control(&selected.id) {
                        Ok(removed) => {
                            let removed_id = removed.id.clone();
                            self.sync_runtime_control_state();
                            self.ui_state.mutate(|state| {
                                if state
                                    .editing_pending_control
                                    .as_ref()
                                    .is_some_and(|editing| editing.id == removed_id)
                                {
                                    state.clear_pending_control_edit();
                                    state.clear_input();
                                }
                                if state.pending_controls.is_empty() {
                                    state.close_pending_control_picker();
                                }
                                state.status = format!(
                                    "Withdrew queued {} {}",
                                    pending_control_kind_label(removed.kind),
                                    preview_id(&removed.id)
                                );
                                state.push_activity(format!(
                                    "withdrew queued {} {}",
                                    pending_control_kind_label(removed.kind),
                                    preview_id(&removed.id)
                                ));
                            });
                        }
                        Err(error) => {
                            let message =
                                summarize_nonfatal_error("withdraw pending control", &error);
                            self.ui_state.mutate(|state| {
                                state.status =
                                    format!("Failed to withdraw pending control: {message}");
                                state.push_activity(format!(
                                    "failed to withdraw pending control: {}",
                                    state::preview_text(&message, 56)
                                ));
                            });
                        }
                    }
                }
                true
            }
            KeyCode::Enter => {
                if let Some(selected) = snapshot.selected_pending_control() {
                    self.ui_state.mutate(|state| {
                        state.begin_pending_control_edit();
                        state.status = format!(
                            "Editing queued {} {}",
                            pending_control_kind_label(selected.kind),
                            preview_id(&selected.id)
                        );
                        state.push_activity(format!(
                            "editing queued {} {}",
                            pending_control_kind_label(selected.kind),
                            preview_id(&selected.id)
                        ));
                    });
                }
                true
            }
            KeyCode::Esc => {
                self.ui_state.mutate(|state| {
                    state.close_pending_control_picker();
                    state.status = "Closed pending controls".to_string();
                    state.push_activity("closed pending controls");
                });
                true
            }
            _ => false,
        }
    }

    fn handle_thinking_effort_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.thinking_effort_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_thinking_effort_picker(true);
                });
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_thinking_effort_picker(false);
                });
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.thinking_effort_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.thinking_effort_picker.as_mut() {
                        picker.selected = state
                            .session
                            .supported_model_reasoning_efforts
                            .len()
                            .saturating_sub(1);
                    }
                });
                true
            }
            KeyCode::Enter => {
                if let Some(level) = snapshot.selected_thinking_effort() {
                    self.set_model_reasoning_effort(&level);
                }
                self.ui_state
                    .mutate(|state| state.close_thinking_effort_picker());
                true
            }
            KeyCode::Esc => {
                self.ui_state.mutate(|state| {
                    state.close_thinking_effort_picker();
                    state.status = "Closed thinking effort picker".to_string();
                    state.push_activity("closed thinking effort picker");
                });
                true
            }
            _ => false,
        }
    }

    fn handle_theme_picker_key(&mut self, key: KeyEvent) -> bool {
        let snapshot = self.ui_state.snapshot();
        if snapshot.theme_picker.is_none() || !snapshot.input.is_empty() {
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_theme_picker(true);
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_theme_picker(false);
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.theme_picker.as_mut() {
                        picker.selected = 0;
                    }
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    if let Some(picker) = state.theme_picker.as_mut() {
                        picker.selected = state.themes.len().saturating_sub(1);
                    }
                });
                self.preview_selected_theme();
                true
            }
            KeyCode::Enter => {
                if let Some(theme_id) = snapshot.selected_theme() {
                    self.apply_tui_theme(&theme_id, true, snapshot.original_theme());
                }
                self.ui_state.mutate(|state| state.close_theme_picker());
                true
            }
            KeyCode::Esc => {
                if let Some(theme_id) = snapshot.original_theme() {
                    self.apply_tui_theme(&theme_id, false, None);
                }
                self.ui_state.mutate(|state| {
                    state.close_theme_picker();
                    state.status = "Closed theme picker and restored the saved theme".to_string();
                    state.push_activity("closed theme picker and restored preview");
                });
                true
            }
            _ => false,
        }
    }

    async fn handle_history_rollback_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(false);
        }
        let snapshot = self.ui_state.snapshot();
        if snapshot.history_rollback.is_none() {
            return Ok(false);
        }
        if !snapshot.input.is_empty() {
            self.ui_state.mutate(|state| state.clear_history_rollback());
            return Ok(false);
        }

        if snapshot.history_rollback_is_primed() {
            if key.code == KeyCode::Esc {
                self.open_history_rollback_overlay().await?;
                return Ok(true);
            }
            self.ui_state.mutate(|state| state.clear_history_rollback());
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Up => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_history_rollback_selection(true);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Right | KeyCode::Down => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_history_rollback_selection(false);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    let _ = state.jump_history_rollback_selection(true);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    let _ = state.jump_history_rollback_selection(false);
                });
                self.refresh_history_rollback_selection_status();
            }
            KeyCode::Enter => {
                self.confirm_history_rollback().await?;
            }
            KeyCode::Char('q') | KeyCode::Backspace | KeyCode::Delete => {
                self.ui_state.mutate(|state| {
                    state.clear_history_rollback();
                    state.status = "Closed history rollback".to_string();
                    state.push_activity("closed history rollback overlay");
                });
            }
            _ => {}
        }

        Ok(true)
    }

    async fn prime_history_rollback(&mut self) -> Result<()> {
        if self
            .history_rollback_candidates()
            .await
            .into_iter()
            .next()
            .is_none()
        {
            self.ui_state.mutate(|state| {
                state.clear_history_rollback();
                state.status = "No visible user turns are available to roll back".to_string();
                state.push_activity("history rollback unavailable");
            });
            return Ok(());
        }

        self.ui_state.mutate(|state| {
            state.prime_history_rollback();
            state.status = "History rollback armed. Press Esc again to choose a turn".to_string();
            state.push_activity("armed history rollback");
        });
        Ok(())
    }

    async fn open_history_rollback_overlay(&mut self) -> Result<()> {
        let candidates = self.history_rollback_candidates().await;
        if candidates.is_empty() {
            self.ui_state.mutate(|state| {
                state.clear_history_rollback();
                state.status = "No visible user turns are available to roll back".to_string();
                state.push_activity("history rollback unavailable");
            });
            return Ok(());
        }

        self.ui_state.mutate(|state| {
            let opened = state.open_history_rollback_overlay(candidates);
            if opened {
                state.status =
                    "History rollback overlay opened. Select a turn to rewind to".to_string();
                state.push_activity("opened history rollback overlay");
            }
        });
        self.refresh_history_rollback_selection_status();
        Ok(())
    }

    async fn history_rollback_candidates(&self) -> Vec<state::HistoryRollbackCandidate> {
        let transcript = self.session.active_visible_transcript().await;
        build_history_rollback_candidates(&transcript)
    }

    fn refresh_history_rollback_selection_status(&self) {
        let snapshot = self.ui_state.snapshot();
        let Some(overlay) = snapshot.history_rollback_overlay() else {
            return;
        };
        let Some(candidate) = overlay.candidates.get(overlay.selected) else {
            return;
        };
        let status = history_rollback_status(candidate, overlay.selected, overlay.candidates.len());
        self.ui_state.mutate(|state| {
            state.status = status;
        });
    }

    async fn confirm_history_rollback(&mut self) -> Result<()> {
        let snapshot = self.ui_state.snapshot();
        let Some(overlay) = snapshot.history_rollback_overlay() else {
            return Ok(());
        };
        let Some(candidate) = overlay.candidates.get(overlay.selected).cloned() else {
            return Ok(());
        };
        let total = overlay.candidates.len();
        let selected = overlay.selected;

        match self
            .session
            .rollback_visible_history_to_message(candidate.message_id.as_str())
            .await
        {
            Ok(outcome) => {
                let transcript = format_visible_transcript_lines(&outcome.transcript);
                let preview = state::preview_text(&candidate.prompt, 48);
                self.ui_state.mutate(move |state| {
                    state.clear_history_rollback();
                    state.show_transcript_pane();
                    state.transcript = transcript;
                    state.follow_transcript = true;
                    state.transcript_scroll = u16::MAX;
                    state.restore_input_draft(candidate.draft.clone());
                    state.status = if candidate.draft.text.trim().is_empty()
                        && candidate.draft.draft_attachments.is_empty()
                    {
                        format!(
                            "Rolled back {} message(s). Selected turn had no text to restore",
                            outcome.removed_message_count
                        )
                    } else {
                        format!(
                            "Rolled back {} message(s). Edit the restored prompt and press Enter",
                            outcome.removed_message_count
                        )
                    };
                    state.push_activity(format!(
                        "rolled back history to turn {} of {}: {}",
                        selected + 1,
                        total,
                        preview
                    ));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("history rollback", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("History rollback failed: {message}");
                    state.push_activity(format!(
                        "history rollback failed: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
        Ok(())
    }

    async fn maybe_finish_turn(&mut self) -> Result<()> {
        let finished = self
            .turn_task
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if !finished {
            return Ok(());
        }
        let git = state::git_snapshot(
            self.session.workspace_root(),
            self.session.host_process_surfaces_allowed(),
        );
        if let Some(task) = self.turn_task.take() {
            match task.await {
                Ok(Ok(())) => {
                    let stored_session_count =
                        self.session.refresh_stored_session_count().await.ok();
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.turn_started_at = None;
                        state.active_tool_label = None;
                        state.session.git = git.clone();
                        if let Some(stored_session_count) = stored_session_count {
                            state.session.stored_session_count = stored_session_count;
                        }
                    });
                }
                Ok(Err(error)) => {
                    let message = summarize_nonfatal_error("turn task", &error);
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.turn_started_at = None;
                        state.active_tool_label = None;
                        state.session.git = git.clone();
                        state.status = format!("Error: {message}");
                        state.push_transcript(state::TranscriptEntry::error_summary_details(
                            message.clone(),
                            Vec::new(),
                        ));
                        state.push_activity(format!(
                            "turn failed: {}",
                            state::preview_text(&message, 56)
                        ));
                    });
                }
                Err(error) => {
                    self.ui_state.mutate(|state| {
                        state.turn_running = false;
                        state.turn_started_at = None;
                        state.active_tool_label = None;
                        state.session.git = git.clone();
                        state.status = format!("Task join error: {error}");
                        state.push_activity(format!("task join error: {error}"));
                    });
                }
            }
        }
        self.sync_runtime_control_state();
        if self.turn_task.is_none() && self.session.queued_command_count() > 0 {
            self.start_runtime_queue_drain();
        }
        Ok(())
    }

    fn sync_user_input_prompt(&mut self, prompt: Option<&UserInputPrompt>) {
        match prompt {
            Some(prompt)
                if self
                    .active_user_input
                    .as_ref()
                    .is_some_and(|flow| flow.prompt_id == prompt.prompt_id) => {}
            Some(prompt) => {
                self.active_user_input = Some(ActiveUserInputState::new(prompt.prompt_id.clone()));
                let status = format!(
                    "Awaiting user input · {} question(s)",
                    prompt.questions.len()
                );
                self.ui_state.mutate(|state| {
                    state.clear_input();
                    state.status = status;
                    state.push_activity("opened user input prompt");
                });
            }
            None if self.active_user_input.take().is_some() => {
                self.ui_state.mutate(|state| {
                    state.clear_input();
                });
            }
            None => {}
        }
    }

    fn advance_user_input_flow(&mut self, prompt: &UserInputPrompt) {
        let Some(flow) = self.active_user_input.as_mut() else {
            return;
        };
        let next_question = flow.current_question + 1;
        if next_question >= prompt.questions.len() {
            let response = UserInputResponse {
                answers: flow.answers.clone(),
            };
            if self.session.resolve_user_input(response) {
                self.active_user_input = None;
                self.ui_state.mutate(|state| {
                    state.clear_input();
                    state.status = "Submitted user input answers".to_string();
                    state.push_activity("submitted user input answers");
                });
            }
            return;
        }

        flow.current_question = next_question;
        flow.collecting_other_note = false;
        let next_header = prompt.questions[next_question].header.clone();
        self.ui_state.mutate(|state| {
            state.clear_input();
            state.status = format!("Next user input question · {next_header}");
            state.push_activity(format!("advanced to user input question {next_header}"));
        });
    }

    async fn maybe_finish_operator_task(&mut self) -> Result<()> {
        let finished = self
            .operator_task
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(false);
        if !finished {
            return Ok(());
        }
        if let Some(task) = self.operator_task.take() {
            match task.await {
                Ok(Ok(OperatorTaskOutcome::WaitLiveTask(outcome))) => {
                    let inspector = format_live_task_wait_outcome(&outcome);
                    self.ui_state.mutate(move |state| {
                        state.show_main_view("Live Task Wait", inspector);
                        state.status = format!(
                            "Live task {} finished with status {}",
                            outcome.task_id, outcome.status
                        );
                        state.push_activity(format!(
                            "wait completed for {} ({})",
                            outcome.task_id, outcome.status
                        ));
                    });
                }
                Ok(Err(error)) => {
                    let message = summarize_nonfatal_error("operator task", &error);
                    self.ui_state.mutate(|state| {
                        state.status = format!("Operator task failed: {message}");
                        state.show_main_view(
                            "Operator Error",
                            vec![
                                InspectorEntry::section("Operator Error"),
                                InspectorEntry::Plain(message.clone()),
                            ],
                        );
                        state.push_activity(format!(
                            "operator task failed: {}",
                            state::preview_text(&message, 56)
                        ));
                    });
                }
                Err(error) => {
                    self.ui_state.mutate(|state| {
                        state.status = format!("Operator task join error: {error}");
                        state.push_activity(format!("operator task join error: {error}"));
                    });
                }
            }
        }
        Ok(())
    }

    async fn apply_plain_input_submit(
        &mut self,
        action: PlainInputSubmitAction,
        submission: ComposerSubmission,
    ) {
        self.record_submitted_prompt(&submission);
        match action {
            PlainInputSubmitAction::StartPrompt => {
                self.start_turn_message(submission.message).await
            }
            PlainInputSubmitAction::QueuePrompt => {
                self.queue_prompt_behind_active_turn_message(submission.message)
                    .await;
            }
            PlainInputSubmitAction::SteerActiveTurn => {
                self.schedule_runtime_steer_while_active(
                    submission.persisted_history_text,
                    Some("inline_enter".to_string()),
                )
                .await;
            }
        }
    }

    async fn try_apply_pending_control_edit(&mut self, input: &str) -> bool {
        let Some(editing) = self.ui_state.snapshot().editing_pending_control.clone() else {
            return false;
        };
        let content = input.trim();
        if content.is_empty() {
            self.ui_state.mutate(|state| {
                state.status = "Pending control edits cannot be empty".to_string();
                state.push_activity("rejected empty pending control edit");
            });
            return true;
        }
        match self.session.update_pending_control(&editing.id, content) {
            Ok(updated) => {
                self.sync_runtime_control_state();
                self.ui_state.mutate(|state| {
                    state.clear_pending_control_edit();
                    state.clear_input();
                    state.status = format!(
                        "Updated queued {} {}",
                        pending_control_kind_label(updated.kind),
                        preview_id(&updated.id)
                    );
                    state.push_activity(format!(
                        "updated queued {} {}",
                        pending_control_kind_label(updated.kind),
                        preview_id(&updated.id)
                    ));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("update pending control", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to update pending control: {message}");
                    state.push_activity(format!(
                        "failed to update pending control: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
        true
    }

    async fn start_turn(&mut self, prompt: String) {
        self.start_turn_message(Message::user(prompt)).await;
    }

    async fn start_turn_message(&mut self, message: Message) {
        if self.turn_task.is_some() {
            self.queue_prompt_behind_active_turn_message(message).await;
            return;
        }

        self.start_command(RuntimeCommand::Prompt { message }).await;
    }

    async fn queue_prompt_behind_active_turn_message(&mut self, message: Message) {
        let preview = state::preview_text(&message_operator_text(&message), 40);
        match self.session.queue_prompt_command(message).await {
            Ok(queued_id) => {
                let pending = self.session.pending_controls();
                let depth = pending.len();
                self.ui_state.mutate(|state| {
                    state.session.queued_commands = depth;
                    state.sync_pending_controls(pending);
                    state.status = "Queued prompt behind the active turn".to_string();
                    state.push_activity(format!("queued prompt {}: {}", queued_id, preview));
                });
            }
            Err(error) => {
                let message = summarize_nonfatal_error("queue prompt", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to queue prompt: {message}");
                    state.push_activity(format!(
                        "failed to queue prompt: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    async fn schedule_runtime_steer_while_active(
        &mut self,
        message: String,
        reason: Option<String>,
    ) {
        let preview = state::preview_text(&message, 40);
        match self.session.schedule_runtime_steer(message, reason) {
            Ok(queued_id) => {
                let pending = self.session.pending_controls();
                self.ui_state.mutate(|state| {
                    state.session.queued_commands = pending.len();
                    state.sync_pending_controls(pending);
                    state.status = "Scheduled steer for the active turn".to_string();
                    state.push_activity(format!(
                        "scheduled active-turn steer {}: {preview}",
                        queued_id
                    ));
                })
            }
            Err(error) => {
                let message = summarize_nonfatal_error("schedule runtime steer", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to schedule steer: {message}");
                    state.push_activity(format!(
                        "failed to schedule active-turn steer: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    async fn start_command(&mut self, command: RuntimeCommand) {
        let preview = queued_command_preview(&command);
        self.ui_state.mutate(|state| {
            state.show_transcript_pane();
            state.follow_transcript = true;
            state.transcript_scroll = u16::MAX;
            state.turn_running = true;
            state.turn_started_at = Some(Instant::now());
            state.active_tool_label = None;
            state.status = "Working".to_string();
            state.push_activity(preview.clone());
        });

        let session = self.session.clone();
        self.turn_task = Some(spawn_local(
            async move { session.apply_control(command).await },
        ));
    }

    fn start_runtime_queue_drain(&mut self) {
        // The host only restarts draining once the active task goes idle. The
        // runtime still owns dequeue order and queue depth, so the TUI reads
        // the current depth instead of speculating about the next popped item.
        let queued = self.session.queued_command_count();
        self.ui_state.mutate(|state| {
            state.show_transcript_pane();
            state.follow_transcript = true;
            state.transcript_scroll = u16::MAX;
            state.turn_running = true;
            state.turn_started_at = Some(Instant::now());
            state.active_tool_label = None;
            state.session.queued_commands = queued;
            state.status = "Working".to_string();
        });

        let session = self.session.clone();
        self.turn_task = Some(spawn_local(async move {
            session.drain_queued_controls().await.map(|_| ())
        }));
    }

    async fn interrupt_active_turn(&mut self) -> Result<()> {
        if !self.abort_turn_task() {
            return Ok(());
        }

        // Once the live task is aborted, any safe-point steer would never be
        // merged in-band. Resubmit all pending steers as one fresh prompt in
        // FIFO order so their intent matches the sequence the operator entered.
        let pending_steers = self.session.take_pending_steers()?;
        self.sync_runtime_control_state();

        let steers = pending_steers
            .into_iter()
            .map(|steer| steer.preview)
            .collect::<Vec<_>>();
        let steer_count = steers.len();

        if let Some(prompt) = merge_interrupt_steers(steers) {
            let preview = state::preview_text(&prompt, 40);
            self.ui_state.mutate(|state| {
                state.turn_running = false;
                state.turn_started_at = None;
                state.active_tool_label = None;
                state.push_transcript(state::TranscriptEntry::error_summary_details(
                    "Interrupted current turn",
                    Vec::new(),
                ));
                state.push_activity(format!(
                    "interrupted current turn and resubmitted {steer_count} steer(s): {preview}"
                ));
            });
            self.start_command(RuntimeCommand::Prompt {
                message: Message::user(prompt),
            })
            .await;
        } else {
            self.ui_state.mutate(|state| {
                state.turn_running = false;
                state.turn_started_at = None;
                state.active_tool_label = None;
                state.status =
                    "Interrupted current turn. What should nanoclaw do next?".to_string();
                state.push_transcript(state::TranscriptEntry::error_summary_details(
                    "Interrupted current turn",
                    Vec::new(),
                ));
                state.push_activity("interrupted current turn");
            });
        }

        Ok(())
    }

    fn cycle_model_reasoning_effort(&mut self) {
        match self.session.cycle_model_reasoning_effort() {
            Ok(outcome) => self.apply_model_reasoning_effort_outcome(outcome, "cycled"),
            Err(error) => self.record_model_reasoning_effort_error(summarize_nonfatal_error(
                "cycle model reasoning effort",
                &error,
            )),
        }
    }

    fn set_model_reasoning_effort(&mut self, effort: &str) {
        match self.session.set_model_reasoning_effort(effort) {
            Ok(outcome) => self.apply_model_reasoning_effort_outcome(outcome, "set"),
            Err(error) => self.record_model_reasoning_effort_error(summarize_nonfatal_error(
                "set model reasoning effort",
                &error,
            )),
        }
    }

    fn apply_model_reasoning_effort_outcome(
        &mut self,
        outcome: crate::backend::ModelReasoningEffortOutcome,
        verb: &str,
    ) {
        let current = outcome
            .current
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let previous = outcome
            .previous
            .clone()
            .unwrap_or_else(|| "default".to_string());
        self.ui_state.mutate(|state| {
            state.session.model_reasoning_effort = outcome.current;
            state.status =
                format!("Thinking effort {verb} to {current}; next model request will use it");
            state.push_activity(format!("thinking effort {previous} -> {current}"));
        });
    }

    fn record_model_reasoning_effort_error(&mut self, message: String) {
        self.ui_state.mutate(|state| {
            state.status = format!("Thinking effort unavailable: {message}");
            state.push_activity(format!(
                "thinking effort rejected: {}",
                state::preview_text(&message, 56)
            ));
        });
    }

    fn preview_selected_theme(&mut self) {
        let snapshot = self.ui_state.snapshot();
        if let Some(theme_id) = snapshot.selected_theme() {
            self.apply_tui_theme(&theme_id, false, None);
        }
    }

    fn apply_tui_theme(
        &mut self,
        theme_id: &str,
        persist: bool,
        previous_override: Option<String>,
    ) {
        match set_active_theme(theme_id) {
            Ok(()) => {
                let current = theme_id.to_string();
                let mut previous = None;
                self.ui_state.mutate(|state| {
                    previous = Some(state.theme.clone());
                    state.theme = current.clone();
                    state.themes = crate::theme::theme_summaries();
                    if !persist {
                        state.status = format!("Previewing theme {current}");
                    }
                });
                if !persist {
                    return;
                }

                match persist_tui_theme_selection(self.session.workspace_root(), &current) {
                    Ok(()) => {
                        let previous = previous_override
                            .or(previous)
                            .unwrap_or_else(|| current.clone());
                        self.ui_state.mutate(|state| {
                            state.status = format!("Theme saved as {current}");
                            if previous == current {
                                state.push_activity(format!("theme persisted: {current}"));
                            } else {
                                state.push_activity(format!("theme {previous} -> {current}"));
                            }
                        });
                    }
                    Err(error) => {
                        let message = summarize_nonfatal_error("persist tui theme", &error);
                        self.ui_state.mutate(|state| {
                            state.status =
                                format!("Theme {current} active, but failed to save: {message}");
                            state.push_activity(format!(
                                "theme persistence failed: {}",
                                state::preview_text(&message, 56)
                            ));
                        });
                    }
                }
            }
            Err(error) => {
                let message = summarize_nonfatal_error("set tui theme", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Theme unavailable: {message}");
                    state.push_activity(format!(
                        "theme rejected: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }

    async fn apply_command(&mut self, input: &str) -> Result<bool> {
        match parse_slash_command(input) {
            SlashCommand::Quit => Ok(true),
            SlashCommand::Status => {
                self.ui_state.mutate(|state| {
                    state.show_main_view("Guide", build_startup_inspector(&state.session));
                    state.status = "Restored session overview".to_string();
                    state.push_activity("restored session overview");
                });
                Ok(false)
            }
            SlashCommand::Details => {
                self.ui_state.mutate(|state| {
                    state.show_tool_details = !state.show_tool_details;
                    let visibility = if state.show_tool_details {
                        "expanded"
                    } else {
                        "collapsed"
                    };
                    state.status = format!("Tool details {visibility}");
                    state.push_activity(format!("tool details {visibility}"));
                });
                Ok(false)
            }
            SlashCommand::StatusLine => {
                self.ui_state.mutate(|state| {
                    state.open_statusline_picker();
                    state.status = "Opened status line picker".to_string();
                    state.push_activity("opened status line picker");
                });
                Ok(false)
            }
            SlashCommand::Thinking { effort } => {
                match effort.as_deref() {
                    Some(effort) => self.set_model_reasoning_effort(effort),
                    None => self.ui_state.mutate(|state| {
                        state.open_thinking_effort_picker();
                        state.status = "Opened thinking effort picker".to_string();
                        state.push_activity("opened thinking effort picker");
                    }),
                }
                Ok(false)
            }
            SlashCommand::Theme { name } => {
                match name.as_deref() {
                    Some(theme_id) => self.apply_tui_theme(theme_id, true, None),
                    None => self.ui_state.mutate(|state| {
                        state.open_theme_picker();
                        state.status =
                            "Opened theme picker; move to preview, Enter to save".to_string();
                        state.push_activity("opened theme picker");
                    }),
                }
                Ok(false)
            }
            SlashCommand::Image { path } => {
                self.attach_composer_image(&path).await;
                Ok(false)
            }
            SlashCommand::File { path } => {
                self.attach_composer_file(&path).await;
                Ok(false)
            }
            SlashCommand::Detach { index } => {
                self.detach_composer_attachment(index);
                Ok(false)
            }
            SlashCommand::MoveAttachment { from, to } => {
                self.move_composer_attachment(from, to);
                Ok(false)
            }
            SlashCommand::Help { query } => {
                let title = query
                    .as_deref()
                    .filter(|query| !query.trim().is_empty())
                    .map(|query| format!("Command Palette · {}", query.trim()))
                    .unwrap_or_else(|| "Command Palette".to_string());
                let lines = command_palette_lines_for(query.as_deref());
                self.ui_state.mutate(|state| {
                    state.show_main_view(title, lines);
                    state.status = "Opened command palette".to_string();
                    state.push_activity("opened command palette");
                });
                Ok(false)
            }
            SlashCommand::Tools => {
                let tool_names = self.session.startup_snapshot().tool_names;
                self.ui_state.mutate(move |state| {
                    let lines = if tool_names.is_empty() {
                        vec![
                            InspectorEntry::section("Tools"),
                            InspectorEntry::Muted("No tools registered.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Tools"))
                            .chain(tool_names.iter().map(|tool| {
                                InspectorEntry::collection(tool.clone(), None::<String>)
                            }))
                            .collect()
                    };
                    state.show_main_view("Tool Catalog", lines);
                    state.status = "Listed core tools".to_string();
                    state.push_activity("inspected tool catalog");
                });
                Ok(false)
            }
            SlashCommand::Skills => {
                let skills = self.session.skills().to_vec();
                self.ui_state.mutate(move |state| {
                    let lines = if skills.is_empty() {
                        vec![
                            InspectorEntry::section("Skills"),
                            InspectorEntry::Muted(
                                "No skills are available in the configured roots.".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Skills"))
                            .chain(skills.iter().map(|skill| {
                                InspectorEntry::collection(
                                    skill.name.clone(),
                                    Some(state::preview_text(&skill.description, 72)),
                                )
                            }))
                            .collect()
                    };
                    state.show_main_view("Skill Catalog", lines);
                    state.status = "Listed available skills".to_string();
                    state.push_activity("inspected skill catalog");
                });
                Ok(false)
            }
            SlashCommand::Diagnostics => {
                let diagnostics = self.session.startup_diagnostics();
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Diagnostics", format_startup_diagnostics(&diagnostics));
                    state.status = "Opened startup diagnostics".to_string();
                    state.push_activity("inspected startup diagnostics");
                });
                Ok(false)
            }
            SlashCommand::Mcp => {
                let servers = self.session.list_mcp_servers().await;
                self.ui_state.mutate(move |state| {
                    let lines = if servers.is_empty() {
                        vec![
                            InspectorEntry::section("MCP"),
                            InspectorEntry::Muted("No MCP servers connected.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP"))
                            .chain(servers.iter().map(format_mcp_server_summary_line))
                            .collect()
                    };
                    state.show_main_view("MCP", lines);
                    state.status = "Listed MCP servers".to_string();
                    state.push_activity("listed mcp servers");
                });
                Ok(false)
            }
            SlashCommand::Prompts => {
                let prompts = self.session.list_mcp_prompts().await;
                self.ui_state.mutate(move |state| {
                    let lines = if prompts.is_empty() {
                        vec![
                            InspectorEntry::section("MCP Prompts"),
                            InspectorEntry::Muted("No MCP prompts available.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP Prompts"))
                            .chain(prompts.iter().map(format_mcp_prompt_summary_line))
                            .collect()
                    };
                    state.show_main_view("Prompts", lines);
                    state.status = "Listed MCP prompts".to_string();
                    state.push_activity("listed mcp prompts");
                });
                Ok(false)
            }
            SlashCommand::Resources => {
                let resources = self.session.list_mcp_resources().await;
                self.ui_state.mutate(move |state| {
                    let lines = if resources.is_empty() {
                        vec![
                            InspectorEntry::section("MCP Resources"),
                            InspectorEntry::Muted("No MCP resources available.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP Resources"))
                            .chain(resources.iter().map(format_mcp_resource_summary_line))
                            .collect()
                    };
                    state.show_main_view("Resources", lines);
                    state.status = "Listed MCP resources".to_string();
                    state.push_activity("listed mcp resources");
                });
                Ok(false)
            }
            SlashCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                let loaded = self
                    .session
                    .load_mcp_prompt(&server_name, &prompt_name)
                    .await?;
                self.ui_state.mutate(move |state| {
                    let inspector = build_mcp_prompt_inspector(&loaded);
                    state.restore_input_draft(state::composer_draft_from_messages(
                        &loaded.input_messages,
                    ));
                    state.show_main_view("Prompt", inspector);
                    state.status =
                        format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
                    state.push_activity(format!("loaded mcp prompt {server_name}/{prompt_name}"));
                });
                Ok(false)
            }
            SlashCommand::Resource { server_name, uri } => {
                let loaded = self.session.load_mcp_resource(&server_name, &uri).await?;
                self.ui_state.mutate(move |state| {
                    let inspector = build_mcp_resource_inspector(&loaded);
                    state
                        .restore_input_draft(state::composer_draft_from_parts(&loaded.input_parts));
                    state.show_main_view("Resource", inspector);
                    state.status = format!("Loaded MCP resource {server_name}:{uri} into input");
                    state.push_activity(format!("loaded mcp resource {server_name}:{uri}"));
                });
                Ok(false)
            }
            SlashCommand::Steer { message } => {
                let Some(message) = message else {
                    self.ui_state.mutate(|state| {
                        state.status = "Usage: /steer <notes>".to_string();
                        state.push_activity("invalid /steer invocation");
                    });
                    return Ok(false);
                };
                if self.turn_task.is_some() {
                    self.schedule_runtime_steer_while_active(
                        message,
                        Some("manual_command".to_string()),
                    )
                    .await;
                    return Ok(false);
                }
                self.start_command(RuntimeCommand::Steer {
                    message,
                    reason: Some("manual_command".to_string()),
                })
                .await;
                Ok(false)
            }
            SlashCommand::Queue => {
                let pending = self.session.pending_controls();
                let opened = !pending.is_empty();
                self.ui_state.mutate(|state| {
                    state.sync_pending_controls(pending);
                    if opened {
                        let _ = state.open_pending_control_picker(true);
                    }
                });
                self.ui_state.mutate(|state| {
                    if opened {
                        state.status = "Opened pending controls".to_string();
                        state.push_activity("opened pending controls");
                    } else {
                        state.status = "No pending prompts or steers".to_string();
                        state.push_activity("no pending controls");
                    }
                });
                Ok(false)
            }
            SlashCommand::Permissions { mode } => {
                if let Some(mode) = mode {
                    if self.turn_task.is_some() {
                        self.ui_state.mutate(|state| {
                            state.status =
                                "Wait for the current turn before switching sandbox mode"
                                    .to_string();
                            state.push_activity(
                                "permissions mode switch blocked while turn running",
                            );
                        });
                        return Ok(false);
                    }

                    let outcome = self.session.set_permission_mode(mode).await?;
                    let snapshot = self.session.startup_snapshot();
                    let (turn_grants, session_grants) = self.session.permission_grant_profiles();
                    let inspector =
                        build_permissions_inspector(&snapshot, &turn_grants, &session_grants);
                    self.sync_session_summary_from_snapshot(&snapshot);
                    self.ui_state.mutate(move |state| {
                        state.show_main_view("Permissions", inspector);
                        if outcome.previous == outcome.current {
                            state.status =
                                format!("Permissions mode already {}", outcome.current.as_str());
                            state.push_activity(format!(
                                "inspected permissions mode {}",
                                outcome.current.as_str()
                            ));
                        } else {
                            state.status =
                                format!("Permissions mode set to {}", outcome.current.as_str());
                            state.push_activity(format!(
                                "permissions mode {} -> {}",
                                outcome.previous.as_str(),
                                outcome.current.as_str()
                            ));
                        }
                    });
                } else {
                    let snapshot = self.session.startup_snapshot();
                    let (turn_grants, session_grants) = self.session.permission_grant_profiles();
                    let inspector =
                        build_permissions_inspector(&snapshot, &turn_grants, &session_grants);
                    self.ui_state.mutate(move |state| {
                        state.show_main_view("Permissions", inspector);
                        state.status = "Opened permissions inspector".to_string();
                        state.push_activity("opened permissions inspector");
                    });
                }
                Ok(false)
            }
            SlashCommand::New => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before starting a new session".to_string();
                        state.push_activity("new session blocked while turn running");
                    });
                    return Ok(false);
                }

                let dropped_commands = self.session.clear_queued_commands().await;
                let outcome = self
                    .session
                    .apply_session_operation(SessionOperation::StartFresh)
                    .await?;
                self.replace_after_session_operation(outcome, dropped_commands);
                Ok(false)
            }
            SlashCommand::Compact { notes } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status = "Wait for the current turn before compacting".to_string();
                        state.push_activity("compact blocked while turn running");
                    });
                    return Ok(false);
                }
                let compacted = self.session.compact_now(notes).await?;
                self.apply_backend_events();
                if !compacted {
                    self.ui_state.mutate(|state| {
                        state.status = "Compaction skipped".to_string();
                        state.push_activity("compaction skipped");
                    });
                }
                Ok(false)
            }
            SlashCommand::LiveTasks => {
                let live_tasks = self.session.list_live_tasks().await?;
                self.ui_state.mutate(move |state| {
                    let lines = if live_tasks.is_empty() {
                        vec![
                            InspectorEntry::section("Live Tasks"),
                            InspectorEntry::Muted(
                                "no live child tasks attached to the active root agent".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Live Tasks"))
                            .chain(
                                live_tasks
                                    .iter()
                                    .map(|task| InspectorEntry::transcript(format_live_task_summary_line(task))),
                            )
                            .collect()
                    };
                    state.show_main_view("Live Tasks", lines);
                    state.status = if live_tasks.is_empty() {
                        "No live child tasks attached".to_string()
                    } else {
                        format!(
                            "Listed {} live child task(s). Use /cancel_task <task-or-agent-ref> to stop one.",
                            live_tasks.len()
                        )
                    };
                    state.push_activity("listed live child tasks");
                });
                Ok(false)
            }
            SlashCommand::SpawnTask { role, prompt } => {
                let outcome = self.session.spawn_live_task(&role, &prompt).await?;
                let inspector = format_live_task_spawn_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Live Task Spawn", inspector);
                    state.status = format!("Spawned live task {}", outcome.task.task_id);
                    state.push_activity(format!(
                        "spawned live task {} ({})",
                        outcome.task.task_id, outcome.task.role
                    ));
                });
                Ok(false)
            }
            SlashCommand::SendTask {
                task_or_agent_ref,
                message,
            } => {
                let Some(message) = message else {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Usage: /send_task <task-or-agent-ref> <message>".to_string();
                        state.push_activity("invalid /send_task invocation");
                    });
                    return Ok(false);
                };
                let outcome = self
                    .session
                    .send_live_task(&task_or_agent_ref, &message)
                    .await?;
                let inspector = format_live_task_message_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Live Task Message", inspector);
                    state.status = match outcome.action {
                        LiveTaskMessageAction::Sent => {
                            format!("Sent steer to live task {}", outcome.task_id)
                        }
                        LiveTaskMessageAction::AlreadyTerminal => {
                            format!("Live task {} was already terminal", outcome.task_id)
                        }
                    };
                    state.push_activity(match outcome.action {
                        LiveTaskMessageAction::Sent => {
                            format!("sent steer to {}", outcome.task_id)
                        }
                        LiveTaskMessageAction::AlreadyTerminal => {
                            format!("live task {} already terminal", outcome.task_id)
                        }
                    });
                });
                Ok(false)
            }
            SlashCommand::WaitTask { task_or_agent_ref } => {
                if self.operator_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current live-task operator action to finish".to_string();
                        state.push_activity("live task wait blocked by existing operator task");
                    });
                    return Ok(false);
                }
                self.start_wait_task(task_or_agent_ref);
                Ok(false)
            }
            SlashCommand::CancelTask {
                task_or_agent_ref,
                reason,
            } => {
                let outcome = self
                    .session
                    .cancel_live_task(&task_or_agent_ref, reason.clone())
                    .await?;
                let inspector = format_live_task_control_outcome(&outcome);
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Live Task Control", inspector);
                    state.status = match outcome.action {
                        LiveTaskControlAction::Cancelled => {
                            format!("Cancelled live task {}", outcome.task_id)
                        }
                        LiveTaskControlAction::AlreadyTerminal => {
                            format!("Live task {} was already terminal", outcome.task_id)
                        }
                    };
                    state.push_activity(match outcome.action {
                        LiveTaskControlAction::Cancelled => {
                            format!("cancelled live task {}", outcome.task_id)
                        }
                        LiveTaskControlAction::AlreadyTerminal => {
                            format!("live task {} already terminal", outcome.task_id)
                        }
                    });
                });
                Ok(false)
            }
            command @ (SlashCommand::AgentSessions { .. }
            | SlashCommand::AgentSession { .. }
            | SlashCommand::Tasks { .. }
            | SlashCommand::Task { .. }
            | SlashCommand::Sessions { .. }
            | SlashCommand::Session { .. }
            | SlashCommand::Resume { .. }
            | SlashCommand::ExportSession { .. }
            | SlashCommand::ExportTranscript { .. }) => self.apply_history_command(command).await,
            SlashCommand::InvalidUsage(message) => {
                let lines = build_command_error_view(input, &message);
                self.ui_state.mutate(|state| {
                    state.status = "Command syntax error".to_string();
                    state.show_main_view("Command Error", lines);
                    state.push_activity("command parse error");
                });
                Ok(false)
            }
        }
    }

    fn start_wait_task(&mut self, task_or_agent_ref: String) {
        let wait_ref = task_or_agent_ref.clone();
        self.ui_state.mutate(|state| {
            state.status = format!("Waiting for live task {}", preview_id(&wait_ref));
            state.push_activity(format!("waiting for live task {}", preview_id(&wait_ref)));
        });
        let session = self.session.clone();
        self.operator_task = Some(spawn_local(async move {
            let outcome = session.wait_live_task(&task_or_agent_ref).await?;
            Ok(OperatorTaskOutcome::WaitLiveTask(outcome))
        }));
    }

    async fn apply_history_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::AgentSessions { session_ref } => {
                let agent_sessions = self
                    .session
                    .list_agent_sessions(session_ref.as_deref())
                    .await?;
                self.ui_state.mutate(move |state| {
                    let lines = if agent_sessions.is_empty() {
                        vec![
                            InspectorEntry::section("Agent Sessions"),
                            InspectorEntry::Muted(
                                "no persisted agent sessions recorded yet".to_string(),
                            ),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Agent Sessions"))
                            .chain(
                                agent_sessions
                                    .iter()
                                    .take(16)
                                    .map(|summary| {
                                        InspectorEntry::transcript(format_agent_session_summary_line(
                                            summary,
                                        ))
                                    }),
                            )
                            .collect()
                    };
                    state.show_main_view("Agent Sessions", lines);
                    state.status = if agent_sessions.is_empty() {
                        "No agent sessions available yet".to_string()
                    } else {
                        format!(
                            "Listed {} agent sessions. Use /agent_session <agent-session-ref> to open one.",
                            agent_sessions.len()
                        )
                    };
                    state.push_activity("listed persisted agent sessions");
                });
                Ok(false)
            }
            SlashCommand::AgentSession { agent_session_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before opening another agent session"
                                .to_string();
                        state.push_activity("agent session replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded = self.session.load_agent_session(&agent_session_ref).await?;
                let inspector = format_agent_session_inspector(&loaded);
                let transcript = format_visible_transcript_lines(&loaded.transcript);
                let agent_session_ref_preview = preview_id(&loaded.summary.agent_session_ref);
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.show_transcript_pane();
                    state.follow_transcript = false;
                    state.inspector_title = "Agent Session".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.status = format!(
                        "Loaded agent session {} with {} transcript messages",
                        agent_session_ref_preview, transcript_count
                    );
                    state.push_activity(format!(
                        "loaded agent session {}",
                        agent_session_ref_preview
                    ));
                });
                Ok(false)
            }
            SlashCommand::Tasks { session_ref } => {
                let tasks = self.session.list_tasks(session_ref.as_deref()).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if tasks.is_empty() {
                        vec![
                            InspectorEntry::section("Tasks"),
                            InspectorEntry::Muted("no persisted tasks recorded yet".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("Tasks"))
                            .chain(tasks.iter().take(16).map(|task| {
                                InspectorEntry::transcript(format_task_summary_line(task))
                            }))
                            .collect()
                    };
                    state.show_main_view("Tasks", lines);
                    state.status = if tasks.is_empty() {
                        "No tasks available yet".to_string()
                    } else {
                        format!(
                            "Listed {} tasks. Use /task <task-id> to open one.",
                            tasks.len()
                        )
                    };
                    state.push_activity("listed persisted tasks");
                });
                Ok(false)
            }
            SlashCommand::Sessions { query } => {
                if let Some(query) = query {
                    let matches = self.session.search_sessions(&query).await?;
                    let stored_session_count =
                        self.session.refresh_stored_session_count().await.ok();
                    self.ui_state.mutate(move |state| {
                        if let Some(stored_session_count) = stored_session_count {
                            state.session.stored_session_count = stored_session_count;
                        }
                        let lines = if matches.is_empty() {
                            vec![
                                InspectorEntry::section("Session Search"),
                                InspectorEntry::Muted(format!("no sessions matched `{query}`")),
                            ]
                        } else {
                            std::iter::once(InspectorEntry::section("Session Search"))
                                .chain(matches.iter().take(12).map(|session| {
                                    InspectorEntry::transcript(format_session_search_line(session))
                                }))
                                .collect()
                        };
                        state.show_main_view("Session Search", lines);
                        state.status = if matches.is_empty() {
                            format!("No sessions matched `{query}`")
                        } else {
                            format!(
                                "Found {} matching sessions. Use /session <session-ref> to open one.",
                                matches.len()
                            )
                        };
                        state.push_activity(format!(
                            "searched sessions: {}",
                            state::preview_text(&query, 40)
                        ));
                    });
                } else {
                    let sessions = self.session.list_sessions().await?;
                    let stored_session_count = sessions.len();
                    self.ui_state.mutate(move |state| {
                        state.session.stored_session_count = stored_session_count;
                        let lines = if sessions.is_empty() {
                            vec![
                                InspectorEntry::section("Sessions"),
                                InspectorEntry::Muted(
                                    "no persisted sessions recorded yet".to_string(),
                                ),
                            ]
                        } else {
                            std::iter::once(InspectorEntry::section("Sessions"))
                                .chain(sessions.iter().take(12).map(|session| {
                                    InspectorEntry::transcript(format_session_summary_line(session))
                                }))
                                .collect()
                        };
                        state.show_main_view("Sessions", lines);
                        state.status = if sessions.is_empty() {
                            "No sessions available yet".to_string()
                        } else {
                            format!(
                                "Listed {} sessions. Use /session <session-ref> to open one.",
                                sessions.len()
                            )
                        };
                        state.push_activity("listed persisted sessions");
                    });
                }
                Ok(false)
            }
            SlashCommand::Session { session_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before opening another session".to_string();
                        state.push_activity("session replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded = self.session.load_session(&session_ref).await?;
                let inspector = format_session_inspector(&loaded);
                let transcript = format_session_transcript_lines(&loaded);
                let session_ref_preview = preview_id(loaded.summary.session_id.as_str());
                let transcript_count = loaded.summary.transcript_message_count;
                self.ui_state.mutate(move |state| {
                    state.show_transcript_pane();
                    state.follow_transcript = false;
                    state.inspector_title = "Session".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.status = format!(
                        "Loaded session {} with {} transcript messages",
                        session_ref_preview, transcript_count
                    );
                    state.push_activity(format!("loaded session {}", session_ref_preview));
                });
                Ok(false)
            }
            SlashCommand::Task { task_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before opening another task".to_string();
                        state.push_activity("task replay blocked while turn running");
                    });
                    return Ok(false);
                }
                let loaded = self.session.load_task(&task_ref).await?;
                let inspector = format_task_inspector(&loaded);
                let transcript = format_visible_transcript_lines(&loaded.child_transcript);
                let task_id = loaded.summary.task_id.clone();
                let transcript_count = loaded.child_transcript.len();
                self.ui_state.mutate(move |state| {
                    state.show_transcript_pane();
                    state.follow_transcript = false;
                    state.inspector_title = "Task".to_string();
                    state.inspector_scroll = 0;
                    state.inspector = inspector;
                    state.transcript = transcript;
                    state.transcript_scroll = 0;
                    state.status = format!(
                        "Loaded task {} with {} child transcript messages",
                        task_id, transcript_count
                    );
                    state.push_activity(format!("loaded task {}", task_id));
                });
                Ok(false)
            }
            SlashCommand::Resume { agent_session_ref } => {
                if self.turn_task.is_some() {
                    self.ui_state.mutate(|state| {
                        state.status =
                            "Wait for the current turn before resuming another session".to_string();
                        state.push_activity("resume blocked while turn running");
                    });
                    return Ok(false);
                }
                let outcome = self
                    .session
                    .apply_session_operation(SessionOperation::ResumeAgentSession {
                        agent_session_ref,
                    })
                    .await?;
                self.replace_after_session_operation(outcome, 0);
                Ok(false)
            }
            SlashCommand::ExportSession { session_ref, path } => {
                let export = self.session.export_session(&session_ref, &path).await?;
                let inspector = format_session_export_result(&export);
                let session_ref_preview = preview_id(export.session_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Export", inspector);
                    state.status = format!(
                        "Exported session {} to {}",
                        session_ref_preview, output_path
                    );
                    state.push_activity(format!("exported session {}", session_ref_preview));
                });
                Ok(false)
            }
            SlashCommand::ExportTranscript { session_ref, path } => {
                let export = self
                    .session
                    .export_session_transcript(&session_ref, &path)
                    .await?;
                let inspector = format_session_export_result(&export);
                let session_ref_preview = preview_id(export.session_id.as_str());
                let output_path = export.output_path.display().to_string();
                self.ui_state.mutate(move |state| {
                    state.show_main_view("Export", inspector);
                    state.status = format!(
                        "Exported transcript {} to {}",
                        session_ref_preview, output_path
                    );
                    state.push_activity(format!("exported transcript {}", session_ref_preview));
                });
                Ok(false)
            }
            _ => unreachable!("history handler received non-history command"),
        }
    }

    fn startup_state(&self) -> TuiState {
        self.startup_state_from_snapshot(&self.session.startup_snapshot())
    }

    fn startup_state_from_snapshot(&self, snapshot: &SessionStartupSnapshot) -> TuiState {
        let workspace_root = snapshot.workspace_root.clone();
        let input_history = input_history::load_input_history(&workspace_root);
        let mut state = TuiState {
            session: state::SessionSummary {
                workspace_name: snapshot.workspace_name.clone(),
                active_session_ref: snapshot.active_session_ref.clone(),
                root_agent_session_id: snapshot.root_agent_session_id.clone(),
                provider_label: snapshot.provider_label.clone(),
                model: snapshot.model.clone(),
                model_reasoning_effort: snapshot.model_reasoning_effort.clone(),
                supported_model_reasoning_efforts: snapshot
                    .supported_model_reasoning_efforts
                    .clone(),
                workspace_root: workspace_root.clone(),
                git: state::git_snapshot(&workspace_root, snapshot.host_process_surfaces_allowed),
                tool_names: snapshot.tool_names.clone(),
                store_label: snapshot.store_label.clone(),
                store_warning: snapshot.store_warning.clone(),
                stored_session_count: snapshot.stored_session_count,
                default_sandbox_summary: snapshot.default_sandbox_summary.clone(),
                sandbox_summary: snapshot.sandbox_summary.clone(),
                permission_mode: snapshot.permission_mode,
                host_process_surfaces_allowed: snapshot.host_process_surfaces_allowed,
                startup_diagnostics: snapshot.startup_diagnostics.clone(),
                queued_commands: 0,
                token_ledger: Default::default(),
                statusline: snapshot.statusline.clone(),
            },
            theme: active_theme_id(),
            themes: crate::theme::theme_summaries(),
            status: "Ready for your next instruction".to_string(),
            follow_transcript: true,
            ..TuiState::default()
        };
        state.set_input_history(input_history);
        state.push_activity("session ready");
        state
    }

    fn sync_session_summary_from_snapshot(&mut self, snapshot: &SessionStartupSnapshot) {
        let git = state::git_snapshot(
            &snapshot.workspace_root,
            snapshot.host_process_surfaces_allowed,
        );
        self.ui_state.mutate(|state| {
            state.session.workspace_name = snapshot.workspace_name.clone();
            state.session.active_session_ref = snapshot.active_session_ref.clone();
            state.session.root_agent_session_id = snapshot.root_agent_session_id.clone();
            state.session.provider_label = snapshot.provider_label.clone();
            state.session.model = snapshot.model.clone();
            state.session.model_reasoning_effort = snapshot.model_reasoning_effort.clone();
            state.session.supported_model_reasoning_efforts =
                snapshot.supported_model_reasoning_efforts.clone();
            state.session.workspace_root = snapshot.workspace_root.clone();
            state.session.git = git;
            state.session.tool_names = snapshot.tool_names.clone();
            state.session.store_label = snapshot.store_label.clone();
            state.session.store_warning = snapshot.store_warning.clone();
            state.session.stored_session_count = snapshot.stored_session_count;
            state.session.default_sandbox_summary = snapshot.default_sandbox_summary.clone();
            state.session.sandbox_summary = snapshot.sandbox_summary.clone();
            state.session.permission_mode = snapshot.permission_mode;
            state.session.host_process_surfaces_allowed = snapshot.host_process_surfaces_allowed;
            state.session.startup_diagnostics = snapshot.startup_diagnostics.clone();
            state.session.statusline = snapshot.statusline.clone();
        });
    }

    fn replace_after_session_operation(
        &mut self,
        outcome: SessionOperationOutcome,
        dropped_commands: usize,
    ) {
        let aborted_operator_task = self.abort_operator_task();
        let previous = self.ui_state.snapshot();
        let show_tool_details = previous.show_tool_details;
        let statusline = previous.session.statusline.clone();
        let mut startup = self.startup_state_from_snapshot(&outcome.startup);
        startup.show_tool_details = show_tool_details;
        startup.session.statusline = statusline;
        startup.session.queued_commands = 0;
        startup.show_transcript_pane();
        startup.follow_transcript = true;
        startup.transcript = format_visible_transcript_lines(&outcome.transcript);
        startup.transcript_scroll = u16::MAX;

        match outcome.action {
            SessionOperationAction::StartedFresh => {
                startup.status = "Started new session".to_string();
                startup.push_activity(format!(
                    "started new session {}",
                    preview_id(&outcome.session_ref)
                ));
            }
            SessionOperationAction::AlreadyAttached => {
                let requested = outcome
                    .requested_agent_session_ref
                    .as_deref()
                    .unwrap_or(outcome.active_agent_session_ref.as_str());
                startup.inspector_title = "Resume".to_string();
                startup.inspector_scroll = 0;
                startup.inspector = format_session_operation_outcome(&outcome);
                startup.status = format!(
                    "Agent session {} is already attached",
                    preview_id(requested)
                );
                startup.push_activity(format!("resume no-op {}", preview_id(requested)));
            }
            SessionOperationAction::Reattached => {
                startup.inspector_title = "Resume".to_string();
                startup.inspector_scroll = 0;
                startup.inspector = format_session_operation_outcome(&outcome);
                startup.status = format!(
                    "Reattached session {} as {}",
                    preview_id(&outcome.session_ref),
                    preview_id(&outcome.active_agent_session_ref)
                );
                startup.push_activity(format!(
                    "resumed session {} as {}",
                    preview_id(&outcome.session_ref),
                    preview_id(&outcome.active_agent_session_ref)
                ));
            }
        }

        if dropped_commands > 0 {
            startup.push_activity(format!("discarded {} queued command(s)", dropped_commands));
        }
        if aborted_operator_task {
            startup.push_activity("aborted pending live-task operator wait after session switch");
        }
        self.ui_state.replace(startup);
    }

    fn sync_runtime_control_state(&self) {
        let pending = self.session.pending_controls();
        self.ui_state.mutate(|state| {
            state.session.queued_commands = pending.len();
            state.sync_pending_controls(pending);
        });
    }

    fn apply_backend_events(&mut self) {
        for event in self.session.drain_events() {
            self.event_renderer.apply_event(event);
        }
    }

    fn abort_operator_task(&mut self) -> bool {
        if let Some(task) = self.operator_task.take() {
            task.abort();
            true
        } else {
            false
        }
    }

    fn abort_turn_task(&mut self) -> bool {
        if let Some(task) = self.turn_task.take() {
            task.abort();
            true
        } else {
            false
        }
    }
}

fn plain_input_submit_action(
    input: &str,
    turn_running: bool,
    key: KeyCode,
) -> Option<PlainInputSubmitAction> {
    if input.trim().is_empty() || input.starts_with('/') {
        return None;
    }
    match (turn_running, key) {
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
    transcript: &[agent::types::Message],
) -> Vec<state::HistoryRollbackCandidate> {
    let user_indices = transcript
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message.role == MessageRole::User).then_some(index))
        .collect::<Vec<_>>();
    let total_turns = user_indices.len();

    user_indices
        .iter()
        .enumerate()
        .filter_map(|(turn_index, start_index)| {
            let start_index = *start_index;
            let message = transcript.get(start_index)?;
            let end_index = user_indices
                .get(turn_index + 1)
                .copied()
                .unwrap_or(transcript.len());
            let turn_slice = transcript.get(start_index..end_index)?;
            let prompt = agent::types::message_operator_text(message);
            let draft = state::composer_draft_from_message(message);
            Some(state::HistoryRollbackCandidate {
                message_id: message.message_id.clone(),
                prompt,
                draft,
                turn_preview_lines: format_visible_transcript_preview_lines(turn_slice),
                removed_turn_count: total_turns.saturating_sub(turn_index),
                removed_message_count: transcript.len().saturating_sub(start_index),
            })
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
        RuntimeCommand::Prompt { message } => {
            let preview = message_operator_text(message);
            format!("running prompt: {}", state::preview_text(&preview, 40))
        }
        RuntimeCommand::Steer { message, .. } => {
            format!("applying steer: {}", state::preview_text(message, 40))
        }
    }
}

fn pending_control_kind_label(kind: crate::backend::PendingControlKind) -> &'static str {
    match kind {
        crate::backend::PendingControlKind::Prompt => "prompt",
        crate::backend::PendingControlKind::Steer => "steer",
    }
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
    turn_grants: &RequestPermissionProfile,
    session_grants: &RequestPermissionProfile,
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

fn permission_profile_summary(profile: &RequestPermissionProfile) -> String {
    let mut entries = Vec::new();
    if let Some(file_system) = profile.file_system.as_ref() {
        if let Some(read) = file_system.read.as_ref() {
            entries.push(format!(
                "read {}",
                state::preview_text(&read.join(", "), 56)
            ));
        }
        if let Some(write) = file_system.write.as_ref() {
            entries.push(format!(
                "write {}",
                state::preview_text(&write.join(", "), 56)
            ));
        }
    }
    if let Some(network) = profile.network.as_ref() {
        if network.enabled == Some(true) {
            entries.push("network full".to_string());
        }
        if let Some(domains) = network.allow_domains.as_ref() {
            entries.push(format!(
                "domains {}",
                state::preview_text(&domains.join(", "), 56)
            ));
        }
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
        external_editor_attachment_status_suffix, merge_interrupt_steers,
        plain_input_submit_action,
    };
    use crate::backend::SessionPermissionMode;
    use agent::types::{Message, MessageId, MessagePart, MessageRole};
    use crossterm::event::KeyCode;
    use std::path::PathBuf;

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
            plain_input_submit_action("tighten the plan", true, KeyCode::Enter),
            Some(PlainInputSubmitAction::SteerActiveTurn)
        );
    }

    #[test]
    fn running_tab_queues_prompt() {
        assert_eq!(
            plain_input_submit_action("write a regression test", true, KeyCode::Tab),
            Some(PlainInputSubmitAction::QueuePrompt)
        );
    }

    #[test]
    fn idle_enter_starts_prompt() {
        assert_eq!(
            plain_input_submit_action("write a regression test", false, KeyCode::Enter),
            Some(PlainInputSubmitAction::StartPrompt)
        );
    }

    #[test]
    fn slash_input_keeps_command_flow() {
        assert_eq!(
            plain_input_submit_action("/help", true, KeyCode::Enter),
            None
        );
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
        let transcript = vec![
            Message::user("first").with_message_id(MessageId::from("msg-1")),
            Message::assistant("answer one").with_message_id(MessageId::from("msg-2")),
            Message::user("second").with_message_id(MessageId::from("msg-3")),
            Message::assistant("answer two").with_message_id(MessageId::from("msg-4")),
        ];

        let candidates = build_history_rollback_candidates(&transcript);

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
    fn history_rollback_candidates_keep_operator_visible_attachment_summaries() {
        let transcript = vec![
            Message::new(
                MessageRole::User,
                vec![MessagePart::ImageUrl {
                    url: "https://example.com/diagram.png".to_string(),
                    mime_type: Some("image/png".to_string()),
                }],
            )
            .with_message_id(MessageId::from("msg-1")),
        ];

        let candidates = build_history_rollback_candidates(&transcript);

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
        let transcript = vec![
            Message::new(
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
            .with_message_id(MessageId::from("msg-1")),
        ];

        let candidates = build_history_rollback_candidates(&transcript);

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
        let transcript = vec![
            Message::new(
                MessageRole::User,
                vec![
                    MessagePart::inline_text("before "),
                    MessagePart::paste("[Paste #1]", "pasted body"),
                    MessagePart::inline_text(" after"),
                ],
            )
            .with_message_id(MessageId::from("msg-1")),
        ];

        let candidates = build_history_rollback_candidates(&transcript);

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
