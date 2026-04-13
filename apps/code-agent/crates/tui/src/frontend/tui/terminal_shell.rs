use super::*;
use crate::frontend::tui::commands::{
    ComposerCompletionEnterAction, composer_completion_hint, resolve_composer_enter_action,
};

pub(crate) enum TerminalLoopControl {
    Continue,
    Exit,
}

impl CodeAgentTui {
    pub async fn run(mut self) -> Result<()> {
        self.ui_state.replace(self.startup_state());

        enable_raw_mode()?;
        let mut stdout = io::stdout();
        // Capture wheel events so scroll input always targets transcript-style
        // surfaces instead of falling through as accidental history recall.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        )?;
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
            DisableMouseCapture,
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
            let viewport_height = self.sync_and_draw_terminal(terminal).await?;
            if !event::poll(Duration::ZERO)? {
                sleep(Duration::from_millis(16)).await;
                continue;
            }
            if matches!(
                self.handle_terminal_event(event::read()?, terminal, viewport_height)
                    .await?,
                TerminalLoopControl::Exit
            ) {
                return Ok(());
            }
        }
    }

    async fn sync_and_draw_terminal(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<u16> {
        self.flush_due_paste_burst().await;
        self.maybe_finish_turn().await?;
        self.apply_backend_events();
        self.maybe_finish_operator_task().await?;
        self.ui_state.mutate(|state| {
            let _ = state.expire_toast_if_due();
        });
        self.sync_runtime_control_state();
        self.sync_skill_summaries();
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
        Ok(viewport_height)
    }

    async fn handle_terminal_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        viewport_height: u16,
    ) -> Result<TerminalLoopControl> {
        match event {
            Event::Paste(text) => {
                self.handle_explicit_paste(&text).await;
                Ok(TerminalLoopControl::Continue)
            }
            Event::Key(key) => {
                self.handle_terminal_key(key, terminal, viewport_height)
                    .await
            }
            Event::Mouse(mouse) => {
                self.handle_terminal_mouse(mouse, viewport_height).await;
                Ok(TerminalLoopControl::Continue)
            }
            _ => Ok(TerminalLoopControl::Continue),
        }
    }

    async fn handle_terminal_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        viewport_height: u16,
    ) -> Result<TerminalLoopControl> {
        if key.kind != KeyEventKind::Press {
            return Ok(TerminalLoopControl::Continue);
        }
        if self.handle_approval_key(key)
            || self.handle_permission_request_key(key)
            || self.handle_user_input_key(key)
            || self.handle_pending_control_picker_key(key)
            || self.handle_statusline_picker_key(key)
            || self.handle_thinking_effort_picker_key(key)
            || self.handle_theme_picker_key(key)
        {
            return Ok(TerminalLoopControl::Continue);
        }
        if let Some(control) = self.handle_collection_picker_key(key).await? {
            return Ok(control);
        }
        if self.handle_tool_review_key(key)? {
            return Ok(TerminalLoopControl::Continue);
        }
        if self.handle_history_rollback_key(key).await? || self.handle_paste_burst_key(key).await {
            return Ok(TerminalLoopControl::Continue);
        }
        self.handle_terminal_key_code(key, terminal, viewport_height)
            .await
    }

    async fn handle_terminal_key_code(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        viewport_height: u16,
    ) -> Result<TerminalLoopControl> {
        match key.code {
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                self.ui_state.mutate(|state| {
                    let opened = state.open_pending_control_picker(true);
                    if opened {
                        state.status = "Opened pending controls".to_string();
                    }
                });
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                self.ui_state.mutate(|state| {
                    if state.pending_control_picker.is_some() {
                        let _ = state.move_pending_control_picker(false);
                    } else {
                        let _ = state.open_pending_control_picker(true);
                    }
                });
            }
            KeyCode::Tab => self.handle_tab_key().await?,
            KeyCode::BackTab => {
                let _ = self.apply_composer_completion(true);
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.navigate_input_history(true);
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.navigate_input_history(false);
            }
            KeyCode::Up => self.handle_vertical_navigation(true),
            KeyCode::Down => self.handle_vertical_navigation(false),
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                let _ = self.handle_transcript_horizontal_navigation(true);
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                let _ = self.handle_transcript_horizontal_navigation(false);
            }
            KeyCode::Left => {
                if !self.handle_tool_selection_navigation(true) {
                    let _ = self.move_input_cursor_horizontal(true);
                }
            }
            KeyCode::Right => {
                if !self.handle_tool_selection_navigation(false) {
                    let _ = self.move_input_cursor_horizontal(false);
                }
            }
            KeyCode::PageUp => {
                self.ui_state
                    .mutate(|state| state.scroll_focused_page(viewport_height, false, true));
            }
            KeyCode::PageDown => {
                self.ui_state
                    .mutate(|state| state.scroll_focused_page(viewport_height, false, false));
            }
            KeyCode::Home
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.handle_tool_selection_boundary(true) => {}
            KeyCode::Home => {
                if !self.move_input_cursor_home() {
                    self.ui_state.mutate(|state| state.scroll_focused_home());
                }
            }
            KeyCode::End
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.handle_tool_selection_boundary(false) => {}
            KeyCode::End => {
                if !self.move_input_cursor_end() {
                    self.ui_state.mutate(|state| state.scroll_focused_end());
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ui_state
                    .mutate(|state| state.scroll_focused_page(viewport_height, true, true));
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ui_state
                    .mutate(|state| state.scroll_focused_page(viewport_height, true, false));
            }
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::ALT) => {
                let _ = self.handle_pending_control_edit_shortcut();
            }
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cycle_model_reasoning_effort();
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.launch_external_editor(terminal).await?;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.kill_input_to_end();
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.yank_kill_buffer();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.stash_composer_draft_on_ctrl_c() {
                    return Ok(TerminalLoopControl::Exit);
                }
            }
            KeyCode::Enter => {
                if self.should_open_selected_tool_review() {
                    self.open_selected_tool_review();
                    return Ok(TerminalLoopControl::Continue);
                }
                if matches!(self.handle_enter_key().await?, TerminalLoopControl::Exit) {
                    return Ok(TerminalLoopControl::Exit);
                }
            }
            KeyCode::Esc => self.handle_escape_key().await?,
            KeyCode::Backspace => {
                if !self.remove_selected_row_attachment() {
                    self.ui_state.mutate(|state| {
                        state.pop_input_char();
                    });
                }
            }
            KeyCode::Delete => {
                let _ = self.remove_selected_row_attachment();
            }
            KeyCode::Char('r') | KeyCode::Char('R')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && self.tool_selection_active() =>
            {
                self.open_selected_tool_review();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ui_state.mutate(|state| {
                    state.push_input_char(ch);
                });
            }
            _ => {}
        }

        Ok(TerminalLoopControl::Continue)
    }

    fn handle_vertical_navigation(&mut self, backwards: bool) {
        if self.move_selected_row_attachment(backwards)
            || self.move_composer_completion_selection(backwards)
        {
            return;
        }
        if self.composer_completion_modal_active() {
            return;
        }
        let snapshot = self.ui_state.snapshot();
        if snapshot.input.is_empty()
            && snapshot.main_pane == state::MainPaneMode::Transcript
            && snapshot.history_rollback.is_none()
            && snapshot.pending_control_picker.is_none()
            && snapshot.tool_review_overlay().is_none()
            && snapshot.statusline_picker.is_none()
            && snapshot.thinking_effort_picker.is_none()
            && snapshot.theme_picker.is_none()
        {
            self.ui_state.mutate(|state| {
                state.scroll_focused(if backwards { -1 } else { 1 });
            });
            return;
        }
        if self.navigate_input_history(backwards)
            || self.move_input_cursor_vertical(backwards)
            || self.move_input_cursor_boundary(backwards)
        {
            return;
        }
        self.ui_state.mutate(|state| {
            state.scroll_focused(if backwards { -1 } else { 1 });
        });
    }

    fn handle_transcript_horizontal_navigation(&mut self, backwards: bool) -> bool {
        let snapshot = self.ui_state.snapshot();
        if !snapshot.input.is_empty()
            || snapshot.main_pane != state::MainPaneMode::Transcript
            || snapshot.history_rollback.is_some()
            || snapshot.pending_control_picker.is_some()
            || snapshot.tool_review_overlay().is_some()
            || snapshot.statusline_picker.is_some()
            || snapshot.thinking_effort_picker.is_some()
            || snapshot.theme_picker.is_some()
        {
            return false;
        }

        self.ui_state.mutate(|state| {
            let _ = state.scroll_transcript_horizontal(if backwards { -4 } else { 4 });
        });
        true
    }

    fn handle_pending_control_edit_shortcut(&mut self) -> bool {
        let snapshot = self.ui_state.snapshot();
        if !snapshot.input.is_empty() || snapshot.pending_controls.is_empty() {
            return false;
        }

        let mut edited = None;
        self.ui_state.mutate(|state| {
            edited = if state.pending_control_picker.is_some() {
                state.begin_pending_control_edit()
            } else {
                state.begin_latest_pending_control_edit()
            };
            if let Some(selected) = edited.as_ref() {
                state.status = format!(
                    "Editing queued {} {}",
                    pending_control_kind_label(selected.kind),
                    preview_id(&selected.id)
                );
                state.push_activity(format!(
                    "editing queued {} {} via alt+t",
                    pending_control_kind_label(selected.kind),
                    preview_id(&selected.id)
                ));
            }
        });
        edited.is_some()
    }

    async fn handle_terminal_mouse(&mut self, mouse: MouseEvent, viewport_height: u16) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.handle_mouse_scroll(true, viewport_height).await,
            MouseEventKind::ScrollDown => self.handle_mouse_scroll(false, viewport_height).await,
            _ => {}
        }
    }

    async fn handle_mouse_scroll(&mut self, backwards: bool, viewport_height: u16) {
        let snapshot = self.ui_state.snapshot();
        if snapshot.tool_review_overlay().is_some() || snapshot.history_rollback_overlay().is_some()
        {
            return;
        }
        if snapshot.pending_control_picker.is_some() && snapshot.input.is_empty() {
            self.ui_state.mutate(|state| {
                let _ = state.move_pending_control_picker(backwards);
            });
            return;
        }
        self.ui_state.mutate(|state| {
            state.scroll_focused_page(viewport_height.max(3), true, backwards);
        });
    }

    fn composer_completion_modal_active(&self) -> bool {
        let snapshot = self.ui_state.snapshot();
        composer_completion_hint(
            &snapshot.input,
            snapshot.composer_input_provenance,
            snapshot.composer_completion_index,
            &snapshot.session.skills,
        )
        .is_some()
    }

    fn handle_tool_selection_navigation(&mut self, backwards: bool) -> bool {
        let snapshot = self.ui_state.snapshot();
        if !snapshot.input.is_empty()
            || snapshot.main_pane != state::MainPaneMode::Transcript
            || snapshot.history_rollback.is_some()
            || snapshot.pending_control_picker.is_some()
            || snapshot.tool_review_overlay().is_some()
        {
            return false;
        }
        if snapshot.transcript.is_empty() && snapshot.active_tool_cells.is_empty() {
            return false;
        }

        self.ui_state.mutate(|state| {
            let _ = state.move_tool_selection(backwards);
        });
        self.refresh_tool_selection_status();
        true
    }

    fn handle_tool_selection_boundary(&mut self, oldest: bool) -> bool {
        let snapshot = self.ui_state.snapshot();
        if !snapshot.input.is_empty()
            || snapshot.main_pane != state::MainPaneMode::Transcript
            || snapshot.history_rollback.is_some()
            || snapshot.pending_control_picker.is_some()
            || snapshot.tool_review_overlay().is_some()
        {
            return false;
        }
        if snapshot.transcript.is_empty() && snapshot.active_tool_cells.is_empty() {
            return false;
        }

        self.ui_state.mutate(|state| {
            let _ = state.jump_tool_selection(oldest);
        });
        self.refresh_tool_selection_status();
        true
    }

    fn refresh_tool_selection_status(&self) {
        let snapshot = self.ui_state.snapshot();
        self.ui_state.mutate(|state| {
            if let Some(tool) = snapshot.selected_tool_entry() {
                state.status = format!(
                    "Selected {} [{}]",
                    tool.tool_name,
                    selected_tool_status_label(tool.status)
                );
            } else if snapshot.tool_selection.is_some() {
                state.status = "Browsing transcript".to_string();
            }
        });
    }

    fn should_open_selected_tool_review(&self) -> bool {
        let snapshot = self.ui_state.snapshot();
        snapshot.input.is_empty()
            && snapshot.main_pane == state::MainPaneMode::Transcript
            && snapshot
                .selected_tool_entry()
                .is_some_and(|tool| tool.review.is_some())
    }

    fn tool_selection_active(&self) -> bool {
        let snapshot = self.ui_state.snapshot();
        snapshot.input.is_empty()
            && snapshot.main_pane == state::MainPaneMode::Transcript
            && snapshot.tool_selection.is_some()
    }

    async fn handle_tab_key(&mut self) -> Result<()> {
        let snapshot = self.ui_state.snapshot();
        if self.try_apply_pending_control_edit(&snapshot.input).await {
            return Ok(());
        }
        if composer_completion_hint(
            &snapshot.input,
            snapshot.composer_input_provenance,
            snapshot.composer_completion_index,
            &snapshot.session.skills,
        )
        .is_some()
            && self.apply_composer_completion(false)
        {
            return Ok(());
        }
        if let Some(action) = plain_input_submit_action(
            &snapshot.input,
            composer_has_prompt_content(&snapshot),
            composer_requires_prompt_submission(&snapshot),
            snapshot.turn_running,
            KeyCode::Tab,
        ) {
            if self.reject_unsupported_image_submission(&snapshot) {
                return Ok(());
            }
            let submission = self.ui_state.take_submission();
            self.apply_plain_input_submit(action, submission).await;
            return Ok(());
        }
        Ok(())
    }

    async fn handle_enter_key(&mut self) -> Result<TerminalLoopControl> {
        let snapshot = self.ui_state.snapshot();
        if self.try_apply_pending_control_edit(&snapshot.input).await {
            return Ok(TerminalLoopControl::Continue);
        }
        if let Some(action) = resolve_composer_enter_action(
            &snapshot.input,
            snapshot.composer_input_provenance,
            snapshot.composer_completion_index,
            &snapshot.session.skills,
        ) {
            match action {
                ComposerCompletionEnterAction::Complete { input, index } => {
                    self.ui_state.mutate(|state| {
                        state.replace_input_from_completion(input);
                        state.composer_completion_index = index;
                    });
                    return Ok(TerminalLoopControl::Continue);
                }
                ComposerCompletionEnterAction::ExecuteSlash(input) => {
                    self.record_submitted_input(&input);
                    self.ui_state.mutate(|state| {
                        state.clear_input();
                    });
                    if self.apply_command(&input).await? {
                        return Ok(TerminalLoopControl::Exit);
                    }
                    return Ok(TerminalLoopControl::Continue);
                }
            }
        }
        if snapshot.input.starts_with('/')
            && let SlashCommand::InvokeSkill { skill_name, prompt } =
                parse_slash_command_with_skills(&snapshot.input, &snapshot.session.skills)
        {
            self.apply_skill_slash_submit(skill_name, prompt).await;
            return Ok(TerminalLoopControl::Continue);
        }
        if let Some(action) = plain_input_submit_action(
            &snapshot.input,
            composer_has_prompt_content(&snapshot),
            composer_requires_prompt_submission(&snapshot),
            snapshot.turn_running,
            KeyCode::Enter,
        ) {
            // Rejecting here keeps the rich draft intact. Once `take_submission()`
            // runs the composer buffer and attachment state are cleared on success.
            if self.reject_unsupported_image_submission(&snapshot) {
                return Ok(TerminalLoopControl::Continue);
            }
            let submission = self.ui_state.take_submission();
            self.apply_plain_input_submit(action, submission).await;
            return Ok(TerminalLoopControl::Continue);
        }

        let input = self.ui_state.take_input();
        if input.trim().is_empty() {
            return Ok(TerminalLoopControl::Continue);
        }
        if input.starts_with('/') {
            self.record_submitted_input(&input);
            if self.apply_command(&input).await? {
                return Ok(TerminalLoopControl::Exit);
            }
        } else {
            self.start_turn(input).await;
        }
        Ok(TerminalLoopControl::Continue)
    }

    async fn handle_escape_key(&mut self) -> Result<()> {
        let snapshot = self.ui_state.snapshot();
        if snapshot.tool_review_overlay().is_some() {
            self.ui_state.mutate(|state| {
                state.clear_tool_review();
                state.status = "Closed tool review".to_string();
                state.push_activity("closed tool review overlay");
            });
            return Ok(());
        }
        if snapshot.editing_pending_control.is_some() {
            self.ui_state.mutate(|state| {
                state.clear_pending_control_edit();
                state.clear_input();
                state.status = "Cancelled pending control edit".to_string();
                state.push_activity("cancelled pending control edit");
            });
            return Ok(());
        }
        if snapshot.input.is_empty()
            && snapshot.main_pane == state::MainPaneMode::Transcript
            && snapshot.tool_selection.is_some()
        {
            self.ui_state.mutate(|state| {
                state.clear_tool_selection();
                state.status = "Cleared transcript selection".to_string();
            });
            return Ok(());
        }
        if snapshot.input.is_empty() && snapshot.main_pane == state::MainPaneMode::View {
            self.ui_state.mutate(|state| {
                state.show_transcript_pane();
                state.status = "Closed view".to_string();
                state.push_activity("closed main view");
            });
            return Ok(());
        }
        if self.turn_task.is_some() {
            self.interrupt_active_turn().await?;
            return Ok(());
        }
        if snapshot.input.is_empty() && snapshot.main_pane == state::MainPaneMode::Transcript {
            self.prime_history_rollback().await?;
        }
        Ok(())
    }
}

fn selected_tool_status_label(status: state::TranscriptToolStatus) -> &'static str {
    match status {
        state::TranscriptToolStatus::Requested => "requested",
        state::TranscriptToolStatus::WaitingApproval => "awaiting approval",
        state::TranscriptToolStatus::Approved => "approved",
        state::TranscriptToolStatus::Running => "running",
        state::TranscriptToolStatus::Finished => "finished",
        state::TranscriptToolStatus::Denied => "denied",
        state::TranscriptToolStatus::Failed => "failed",
        state::TranscriptToolStatus::Cancelled => "cancelled",
    }
}
