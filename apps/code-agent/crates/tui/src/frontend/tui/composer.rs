use super::*;

impl CodeAgentTui {
    pub(super) fn apply_command_completion(&mut self, backwards: bool) -> bool {
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

    pub(super) fn navigate_input_history(&mut self, backwards: bool) -> bool {
        let mut navigated = false;
        self.ui_state.mutate(|state| {
            navigated = state.browse_input_history(backwards);
        });
        navigated
    }

    pub(super) async fn flush_due_paste_burst(&mut self) {
        let now = Instant::now();
        match self.paste_burst.flush_if_due(now) {
            FlushResult::Paste(text) => self.insert_pasted_text(&text).await,
            FlushResult::Typed(ch) => self.ui_state.mutate(|state| state.push_input_char(ch)),
            FlushResult::None => {}
        }
    }

    pub(super) async fn handle_explicit_paste(&mut self, text: &str) {
        self.insert_pasted_text(text).await;
        self.paste_burst.clear_after_explicit_paste();
    }

    pub(super) async fn handle_paste_burst_key(&mut self, key: KeyEvent) -> bool {
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
                self.insert_pasted_text("\n").await;
                self.paste_burst.clear_window_after_non_char();
                return true;
            }
        }

        if let Some(flushed) = self.paste_burst.flush_before_modified_input() {
            self.insert_pasted_text(&flushed).await;
        }
        self.paste_burst.clear_window_after_non_char();
        false
    }

    pub(super) async fn insert_pasted_text(&mut self, text: &str) {
        if text.is_empty() || !self.composer_accepts_text_input() {
            return;
        }
        if self.try_attach_pasted_image_path(text).await {
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

    pub(super) fn stash_composer_draft_on_ctrl_c(&mut self) -> bool {
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

    pub(super) fn kill_input_to_end(&mut self) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut killed = false;
        self.ui_state
            .mutate(|state| killed = state.kill_input_to_end());
        killed
    }

    pub(super) fn yank_kill_buffer(&mut self) -> bool {
        if !self.composer_accepts_text_input() {
            return false;
        }

        let mut yanked = false;
        self.ui_state
            .mutate(|state| yanked = state.yank_kill_buffer());
        yanked
    }

    pub(super) async fn launch_external_editor(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<()> {
        if !self.composer_accepts_text_input() {
            return Ok(());
        }

        if let Some(flushed) = self.paste_burst.flush_before_modified_input() {
            self.insert_pasted_text(&flushed).await;
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

    pub(super) fn composer_accepts_text_input(&self) -> bool {
        if self.approval_prompt().is_some() || self.permission_request_prompt().is_some() {
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

    pub(super) fn move_input_cursor_horizontal(&mut self, backwards: bool) -> bool {
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

    pub(super) fn move_input_cursor_boundary(&mut self, backwards: bool) -> bool {
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

    pub(super) fn move_input_cursor_vertical(&mut self, backwards: bool) -> bool {
        let mut moved = false;
        self.ui_state.mutate(|state| {
            moved = state.move_input_cursor_vertical(backwards);
        });
        moved
    }

    pub(super) fn move_selected_row_attachment(&mut self, backwards: bool) -> bool {
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

    pub(super) fn remove_selected_row_attachment(&mut self) -> bool {
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

    pub(super) fn move_input_cursor_home(&mut self) -> bool {
        let mut moved = false;
        self.ui_state
            .mutate(|state| moved = state.move_input_cursor_home());
        moved
    }

    pub(super) fn move_input_cursor_end(&mut self) -> bool {
        let mut moved = false;
        self.ui_state
            .mutate(|state| moved = state.move_input_cursor_end());
        moved
    }

    pub(super) fn record_submitted_input(&mut self, input: &str) {
        let workspace_root = self.workspace_root_buf();
        let mut persisted = None;
        self.ui_state.mutate(|state| {
            let _ = state.record_local_input_history(input);
            if state.record_input_history(SubmittedPromptSnapshot::from_text(input.to_string())) {
                persisted = Some(state.input_history().to_vec());
            }
        });
        if let Some(entries) = persisted {
            input_history::persist_input_history(&workspace_root, &entries);
        }
    }

    pub(super) fn record_submitted_prompt(&mut self, submission: &ComposerSubmission) {
        let workspace_root = self.workspace_root_buf();
        let mut persisted = None;
        self.ui_state.mutate(|state| {
            state.clear_composer_context_hint();
            state.clear_toast();
            let _ = state.record_local_input_draft(submission.local_history_draft.clone());
            if state.record_input_history(submission.prompt_snapshot.clone()) {
                persisted = Some(state.input_history().to_vec());
            }
        });
        if let Some(entries) = persisted {
            input_history::persist_input_history(&workspace_root, &entries);
        }
    }

    pub(super) fn composer_attachment_context(&self) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: self.workspace_root_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    pub(super) fn active_model_supports_image_input(&self) -> bool {
        self.ui_state.snapshot().session.supports_image_input
    }

    pub(super) fn reject_unsupported_image_submission(&mut self, snapshot: &TuiState) -> bool {
        if snapshot.session.supports_image_input || !composer_uses_image_input(snapshot) {
            return false;
        }

        let model = snapshot.session.model.clone();
        self.ui_state.mutate(|state| {
            state.status = format!(
                "Model {model} does not support image input; remove images or switch models"
            );
            state.push_activity(format!("blocked image prompt on non-vision model {model}"));
        });
        true
    }

    pub(super) async fn try_attach_pasted_image_path(&mut self, text: &str) -> bool {
        let Some(path) = self.pasted_local_image_path_candidate(text).await else {
            return false;
        };
        if !self.active_model_supports_image_input() {
            return false;
        }
        self.attach_composer_image(&path).await;
        true
    }

    pub(super) async fn pasted_local_image_path_candidate(&self, text: &str) -> Option<String> {
        let candidate = text.trim();
        if candidate.is_empty()
            || candidate.contains(['\n', '\r'])
            || is_remote_attachment_url(candidate)
        {
            return None;
        }

        let context = self.composer_attachment_context();
        let resolved_path = resolve_tool_path_against_workspace_root(
            candidate,
            context.effective_root(),
            context.container_workdir.as_deref(),
        )
        .ok()?;
        context.assert_path_read_allowed(&resolved_path).ok()?;
        if !looks_like_local_image_path(&resolved_path) {
            return None;
        }
        let metadata = fs::metadata(&resolved_path).await.ok()?;
        metadata.is_file().then(|| candidate.to_string())
    }

    pub(super) async fn attach_composer_image(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            self.ui_state.mutate(|state| {
                state.status = "Usage: /image <path-or-url>".to_string();
                state.push_activity("invalid /image invocation");
            });
            return;
        }

        if !self.active_model_supports_image_input() {
            self.ui_state.mutate(|state| {
                state.status =
                    format!("Model {} does not support image input", state.session.model);
                state.push_activity("rejected image attachment on non-vision model");
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
                let part = image.message_part();
                let mime_type = match &part {
                    MessagePart::Image { mime_type, .. } => Some(mime_type.clone()),
                    _ => None,
                };
                let attachment = ComposerDraftAttachmentState {
                    placeholder: Some("[Image #1]".to_string()),
                    kind: ComposerDraftAttachmentKind::LocalImage {
                        requested_path: path.to_string(),
                        mime_type,
                        part: Some(part),
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

    pub(super) async fn attach_composer_file(&mut self, path: &str) {
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
                        file_name: file.file_name.clone(),
                        mime_type: file.mime_type.clone(),
                        part: Some(MessagePart::File {
                            file_name: file.file_name.clone(),
                            mime_type: file.mime_type.clone(),
                            data_base64: Some(file.data_base64),
                            uri: Some(file.requested_path.clone()),
                        }),
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

    pub(super) fn detach_composer_attachment(&mut self, index: Option<usize>) {
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

    pub(super) fn move_composer_attachment(&mut self, from: usize, to: usize) {
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
}
