use super::super::input_history::{self, ComposerHistoryKind};
use super::{ComposerContextHint, TuiState, preview_text};
use agent::types::{
    Message, MessagePart, MessageRole, SubmittedPromptAttachment, SubmittedPromptAttachmentKind,
    SubmittedPromptSnapshot,
};
use std::path::Path;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ComposerHistoryNavigationState {
    pub(crate) mode: ComposerHistoryBrowseMode,
    pub(crate) index: usize,
    pub(crate) draft: ComposerDraftState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComposerHistoryBrowseMode {
    PromptOnly,
    CommandOnly,
    Combined,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ComposerDraftState {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) draft_attachments: Vec<ComposerDraftAttachmentState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposerRowAttachmentPreview {
    pub(crate) index: usize,
    pub(crate) summary: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ComposerAttachmentEditSummary {
    pub(crate) detached: Vec<ComposerRowAttachmentPreview>,
    pub(crate) reordered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ComposerDraftAttachmentState {
    pub(crate) placeholder: Option<String>,
    pub(crate) kind: ComposerDraftAttachmentKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ComposerDraftAttachmentKind {
    LargePaste {
        payload: String,
    },
    LocalImage {
        requested_path: String,
        mime_type: Option<String>,
        part: Option<MessagePart>,
    },
    RemoteImage {
        requested_url: String,
        part: MessagePart,
    },
    LocalFile {
        requested_path: String,
        file_name: Option<String>,
        mime_type: Option<String>,
        part: Option<MessagePart>,
    },
    RemoteFile {
        requested_url: String,
        part: MessagePart,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ComposerKillBufferState {
    pub(crate) text: String,
    pub(crate) draft_attachments: Vec<ComposerDraftAttachmentState>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ComposerSubmission {
    pub(crate) prompt_snapshot: SubmittedPromptSnapshot,
    pub(crate) local_history_draft: ComposerDraftState,
}

impl ComposerDraftState {
    pub(crate) fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            draft_attachments: Vec::new(),
        }
    }

    pub(crate) fn row_attachment_previews(&self) -> Vec<ComposerRowAttachmentPreview> {
        summarize_row_attachments(&self.draft_attachments)
    }

    pub(crate) fn row_attachment_summaries(&self) -> Vec<(usize, String, String)> {
        self.row_attachment_previews()
            .into_iter()
            .map(|preview| (preview.index, preview.summary, preview.detail))
            .collect()
    }

    fn normalized(mut self) -> Self {
        self.draft_attachments.retain(|attachment| {
            attachment
                .placeholder
                .as_ref()
                .is_none_or(|placeholder| self.text.contains(placeholder))
        });
        normalize_attachment_placeholders(&mut self.text, &mut self.draft_attachments);
        self.cursor = normalize_input_cursor(&self.text, self.cursor.min(self.text.len()));
        self
    }
}

pub(crate) fn draft_preview_text(
    draft: &ComposerDraftState,
    fallback_prompt: &str,
    max_chars: usize,
) -> String {
    let attachment_preview = draft
        .row_attachment_summaries()
        .into_iter()
        .map(|(index, summary, _)| format!("#{index} {summary}"))
        .collect::<Vec<_>>();
    let mut parts = Vec::new();
    if !attachment_preview.is_empty() {
        let head = attachment_preview
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let remainder = attachment_preview.len().saturating_sub(2);
        if remainder == 0 {
            parts.push(head);
        } else {
            parts.push(format!("{head}, +{remainder} more"));
        }
    }
    let trimmed_text = draft.text.trim();
    if !trimmed_text.is_empty() {
        parts.push(trimmed_text.to_string());
    }
    if parts.is_empty() {
        preview_text(fallback_prompt, max_chars)
    } else {
        preview_text(&parts.join(" · "), max_chars)
    }
}

impl ComposerDraftAttachmentState {
    fn submitted_prompt_attachment(&self) -> SubmittedPromptAttachment {
        SubmittedPromptAttachment {
            placeholder: self.placeholder.clone(),
            kind: match &self.kind {
                ComposerDraftAttachmentKind::LargePaste { payload } => {
                    SubmittedPromptAttachmentKind::Paste {
                        text: payload.clone(),
                    }
                }
                ComposerDraftAttachmentKind::LocalImage {
                    requested_path,
                    mime_type,
                    ..
                } => SubmittedPromptAttachmentKind::LocalImage {
                    requested_path: requested_path.clone(),
                    mime_type: mime_type.clone(),
                },
                ComposerDraftAttachmentKind::RemoteImage {
                    requested_url,
                    part,
                } => {
                    let mime_type = match part {
                        MessagePart::ImageUrl { mime_type, .. } => mime_type.clone(),
                        _ => None,
                    };
                    SubmittedPromptAttachmentKind::RemoteImage {
                        requested_url: requested_url.clone(),
                        mime_type,
                    }
                }
                ComposerDraftAttachmentKind::LocalFile {
                    requested_path,
                    file_name,
                    mime_type,
                    ..
                } => SubmittedPromptAttachmentKind::LocalFile {
                    requested_path: requested_path.clone(),
                    file_name: file_name.clone(),
                    mime_type: mime_type.clone(),
                },
                ComposerDraftAttachmentKind::RemoteFile {
                    requested_url,
                    part,
                } => {
                    let (file_name, mime_type) = match part {
                        MessagePart::File {
                            file_name,
                            mime_type,
                            ..
                        } => (file_name.clone(), mime_type.clone()),
                        _ => (None, None),
                    };
                    SubmittedPromptAttachmentKind::RemoteFile {
                        requested_url: requested_url.clone(),
                        file_name,
                        mime_type,
                    }
                }
            },
        }
    }

    fn same_persisted_attachment(&self, other: &Self) -> bool {
        self.submitted_prompt_attachment() == other.submitted_prompt_attachment()
    }

    fn is_row_attachment(&self) -> bool {
        self.placeholder.is_none()
    }

    fn external_editor_row_token(&self, index: usize) -> Option<String> {
        match self.kind {
            ComposerDraftAttachmentKind::LocalImage { .. }
            | ComposerDraftAttachmentKind::RemoteImage { .. } => Some(format!("[Image #{index}]")),
            ComposerDraftAttachmentKind::LocalFile { .. }
            | ComposerDraftAttachmentKind::RemoteFile { .. } => Some(format!("[File #{index}]")),
            ComposerDraftAttachmentKind::LargePaste { .. } => None,
        }
    }

    fn external_editor_row_line(&self, index: usize) -> Option<String> {
        let token = self.external_editor_row_token(index)?;
        let detail = self.row_detail()?;
        Some(format!("{token} {detail}"))
    }

    fn default_inline_placeholder(&self) -> Option<String> {
        default_inline_placeholder(&self.kind)
    }

    pub(crate) fn row_summary(&self) -> Option<String> {
        match &self.kind {
            ComposerDraftAttachmentKind::LargePaste { .. } => None,
            ComposerDraftAttachmentKind::LocalImage { requested_path, .. }
            | ComposerDraftAttachmentKind::RemoteImage {
                requested_url: requested_path,
                ..
            } => Some(format!("image · {}", preview_path_tail(requested_path))),
            ComposerDraftAttachmentKind::LocalFile { requested_path, .. }
            | ComposerDraftAttachmentKind::RemoteFile {
                requested_url: requested_path,
                ..
            } => Some(format!("file · {}", preview_path_tail(requested_path))),
        }
    }

    pub(crate) fn row_detail(&self) -> Option<String> {
        match &self.kind {
            ComposerDraftAttachmentKind::LargePaste { .. } => None,
            ComposerDraftAttachmentKind::LocalImage { requested_path, .. }
            | ComposerDraftAttachmentKind::RemoteImage {
                requested_url: requested_path,
                ..
            }
            | ComposerDraftAttachmentKind::LocalFile { requested_path, .. }
            | ComposerDraftAttachmentKind::RemoteFile {
                requested_url: requested_path,
                ..
            } => Some(requested_path.clone()),
        }
    }
}

pub(crate) fn composer_draft_from_message(message: &Message) -> ComposerDraftState {
    let mut text = String::new();
    let mut previous_inline = false;
    let mut has_previous_text_fragment = false;
    let mut draft_attachments = Vec::new();

    for part in &message.parts {
        if let Some(attachment) = composer_draft_attachment_from_part(part) {
            if let Some(placeholder) = attachment.placeholder.as_ref() {
                if has_previous_text_fragment && !previous_inline {
                    text.push('\n');
                }
                text.push_str(placeholder);
                previous_inline = true;
                has_previous_text_fragment = true;
            }
            draft_attachments.push(attachment);
            continue;
        }

        let fragment = composer_draft_text_fragment(part);
        if fragment.is_empty() {
            continue;
        }
        let inline = composer_draft_part_is_inline(part);
        if has_previous_text_fragment {
            if previous_inline && inline {
                text.push_str(&fragment);
            } else {
                text.push('\n');
                text.push_str(&fragment);
            }
        } else {
            text.push_str(&fragment);
        }
        previous_inline = inline;
        has_previous_text_fragment = true;
    }

    ComposerDraftState {
        cursor: text.len(),
        text,
        draft_attachments,
    }
    .normalized()
}

pub(crate) fn composer_draft_from_messages(messages: &[Message]) -> ComposerDraftState {
    let mut combined = ComposerDraftState::default();
    for draft in messages.iter().map(composer_draft_from_message) {
        if !combined.text.is_empty() && !draft.text.is_empty() {
            combined.text.push_str("\n\n");
        }
        combined.text.push_str(&draft.text);
        combined.draft_attachments.extend(draft.draft_attachments);
    }
    combined.cursor = combined.text.len();
    combined.normalized()
}

pub(crate) fn composer_draft_from_parts(parts: &[MessagePart]) -> ComposerDraftState {
    composer_draft_from_message(&Message::new(MessageRole::User, parts.to_vec()))
}

pub(crate) fn composer_draft_from_prompt_snapshot(
    snapshot: &SubmittedPromptSnapshot,
) -> ComposerDraftState {
    ComposerDraftState {
        cursor: snapshot.text.len(),
        text: snapshot.text.clone(),
        draft_attachments: snapshot
            .attachments
            .iter()
            .cloned()
            .map(composer_draft_attachment_from_snapshot)
            .collect(),
    }
    .normalized()
}

fn submitted_prompt_snapshot_from_draft(draft: &ComposerDraftState) -> SubmittedPromptSnapshot {
    SubmittedPromptSnapshot {
        text: draft.text.clone(),
        attachments: draft
            .draft_attachments
            .iter()
            .map(ComposerDraftAttachmentState::submitted_prompt_attachment)
            .collect(),
    }
}

fn composer_draft_attachment_from_snapshot(
    attachment: SubmittedPromptAttachment,
) -> ComposerDraftAttachmentState {
    let placeholder = attachment.placeholder.clone();
    let kind = match attachment.kind {
        SubmittedPromptAttachmentKind::Paste { text } => {
            ComposerDraftAttachmentKind::LargePaste { payload: text }
        }
        SubmittedPromptAttachmentKind::LocalImage {
            requested_path,
            mime_type,
        } => ComposerDraftAttachmentKind::LocalImage {
            requested_path,
            mime_type,
            part: None,
        },
        SubmittedPromptAttachmentKind::RemoteImage {
            requested_url,
            mime_type,
        } => ComposerDraftAttachmentKind::RemoteImage {
            requested_url: requested_url.clone(),
            part: MessagePart::ImageUrl {
                url: requested_url,
                mime_type,
            },
        },
        SubmittedPromptAttachmentKind::LocalFile {
            requested_path,
            file_name,
            mime_type,
        } => ComposerDraftAttachmentKind::LocalFile {
            requested_path,
            file_name,
            mime_type,
            part: None,
        },
        SubmittedPromptAttachmentKind::RemoteFile {
            requested_url,
            file_name,
            mime_type,
        } => ComposerDraftAttachmentKind::RemoteFile {
            requested_url: requested_url.clone(),
            part: MessagePart::File {
                file_name,
                mime_type,
                data_base64: None,
                uri: Some(requested_url),
            },
        },
        SubmittedPromptAttachmentKind::EmbeddedImage { mime_type } => {
            ComposerDraftAttachmentKind::RemoteImage {
                requested_url: mime_type
                    .clone()
                    .unwrap_or_else(|| "embedded-image".to_string()),
                part: MessagePart::ImageUrl {
                    url: mime_type
                        .clone()
                        .unwrap_or_else(|| "embedded-image".to_string()),
                    mime_type,
                },
            }
        }
        SubmittedPromptAttachmentKind::EmbeddedFile {
            file_name,
            mime_type,
            uri,
        } => {
            let requested_path = uri
                .clone()
                .or(file_name.clone())
                .unwrap_or_else(|| "embedded-file".to_string());
            ComposerDraftAttachmentKind::LocalFile {
                requested_path,
                file_name,
                mime_type,
                part: None,
            }
        }
    };
    ComposerDraftAttachmentState { placeholder, kind }
}

fn composer_draft_attachment_from_part(part: &MessagePart) -> Option<ComposerDraftAttachmentState> {
    match part {
        MessagePart::Paste { label, text } => Some(ComposerDraftAttachmentState {
            placeholder: Some(label.clone()),
            kind: ComposerDraftAttachmentKind::LargePaste {
                payload: text.clone(),
            },
        }),
        MessagePart::Image { mime_type, .. } => Some(ComposerDraftAttachmentState {
            placeholder: Some("[Image #1]".to_string()),
            kind: ComposerDraftAttachmentKind::LocalImage {
                requested_path: format!("[image:{mime_type}]"),
                mime_type: Some(mime_type.clone()),
                part: Some(part.clone()),
            },
        }),
        MessagePart::ImageUrl { url, .. } => Some(ComposerDraftAttachmentState {
            placeholder: None,
            kind: ComposerDraftAttachmentKind::RemoteImage {
                requested_url: url.clone(),
                part: part.clone(),
            },
        }),
        MessagePart::File {
            file_name,
            mime_type,
            uri,
            ..
        } => {
            let requested_path = uri
                .as_ref()
                .or(file_name.as_ref())
                .cloned()
                .unwrap_or_else(|| "[file]".to_string());
            let kind = if is_remote_url(&requested_path) {
                ComposerDraftAttachmentKind::RemoteFile {
                    requested_url: requested_path,
                    part: part.clone(),
                }
            } else {
                ComposerDraftAttachmentKind::LocalFile {
                    requested_path,
                    file_name: file_name.clone(),
                    mime_type: mime_type.clone(),
                    part: Some(part.clone()),
                }
            };
            Some(ComposerDraftAttachmentState {
                placeholder: default_inline_placeholder(&kind),
                kind,
            })
        }
        _ => None,
    }
}

fn composer_draft_text_fragment(part: &MessagePart) -> String {
    match part {
        MessagePart::Paste { label, .. } => label.clone(),
        _ => agent::types::message_part_operator_text(part),
    }
}

fn composer_draft_part_is_inline(part: &MessagePart) -> bool {
    matches!(
        part,
        MessagePart::InlineText { .. } | MessagePart::Paste { .. }
    )
}

impl TuiState {
    pub(crate) fn external_editor_seed_text(&self) -> String {
        let row_lines = self
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.is_row_attachment())
            .enumerate()
            .filter_map(|(index, attachment)| attachment.external_editor_row_line(index + 1))
            .collect::<Vec<_>>();
        if row_lines.is_empty() {
            return self.input.clone();
        }

        // Row attachments stay outside the normal composer text so the main TUI
        // can render them as dedicated rows. The external editor needs a text
        // representation though, so we project them into a small prelude that
        // can be reordered or cleared before the actual prompt body.
        let mut lines = Vec::with_capacity(row_lines.len() + 4);
        lines.push("[Attachments]".to_string());
        lines.extend(row_lines);
        lines.push(String::new());
        lines.push("[Prompt]".to_string());
        if !self.input.is_empty() {
            lines.push(self.input.clone());
        }
        lines.join("\n")
    }

    pub(crate) fn set_input_history(
        &mut self,
        entries: Vec<input_history::PersistedComposerHistoryEntry>,
        prompts: Vec<SubmittedPromptSnapshot>,
        commands: Vec<SubmittedPromptSnapshot>,
    ) {
        self.persisted_history_entries = entries;
        self.input_history = prompts;
        self.command_history = commands;
        self.input_history_navigation = None;
    }

    pub(crate) fn record_input_history(&mut self, prompt: SubmittedPromptSnapshot) -> bool {
        self.input_history_navigation = None;
        let recorded = record_persistent_history_entry(
            &mut self.input_history,
            ComposerHistoryKind::Prompt,
            prompt,
        );
        if recorded {
            let _ = input_history::record_input_history(
                &mut self.persisted_history_entries,
                ComposerHistoryKind::Prompt,
                self.input_history
                    .last()
                    .cloned()
                    .expect("recorded prompt history must have a tail entry"),
            );
        }
        recorded
    }

    pub(crate) fn record_command_history(&mut self, prompt: SubmittedPromptSnapshot) -> bool {
        self.input_history_navigation = None;
        let recorded = record_persistent_history_entry(
            &mut self.command_history,
            ComposerHistoryKind::Command,
            prompt,
        );
        if recorded {
            let _ = input_history::record_input_history(
                &mut self.persisted_history_entries,
                ComposerHistoryKind::Command,
                self.command_history
                    .last()
                    .cloned()
                    .expect("recorded command history must have a tail entry"),
            );
        }
        recorded
    }

    pub(crate) fn record_local_input_history(&mut self, input: &str) -> bool {
        self.input_history_navigation = None;
        let Some(text) = input_history::normalized_history_text(input) else {
            return false;
        };
        let draft = ComposerDraftState::from_text(text);
        self.record_local_input_draft(draft)
    }

    pub(crate) fn record_local_input_draft(&mut self, mut draft: ComposerDraftState) -> bool {
        self.input_history_navigation = None;
        let normalized_text = input_history::normalized_history_text(&draft.text);
        match normalized_text {
            Some(text) => {
                draft.text = text;
                draft.cursor = draft.text.len();
            }
            None if draft.draft_attachments.is_empty() => return false,
            None => {
                draft.text.clear();
                draft.cursor = 0;
            }
        }
        let draft = draft.normalized();
        let kind = ComposerHistoryKind::classify_text(&draft.text);
        let local_history = match kind {
            ComposerHistoryKind::Prompt => &mut self.local_input_history,
            ComposerHistoryKind::Command => &mut self.local_command_history,
        };
        if local_history.last().is_some_and(|existing| {
            submitted_prompt_snapshot_from_draft(existing)
                == submitted_prompt_snapshot_from_draft(&draft)
        }) {
            return false;
        }
        local_history.push(draft);
        if local_history.len() > input_history::MAX_COMPOSER_HISTORY_ENTRIES {
            let overflow = local_history.len() - input_history::MAX_COMPOSER_HISTORY_ENTRIES;
            local_history.drain(0..overflow);
        }
        true
    }

    pub(crate) fn stash_current_input_draft(&mut self) -> bool {
        self.input_history_navigation = None;
        if self.input.is_empty() && self.draft_attachments.is_empty() {
            return false;
        }

        let draft = self.current_input_draft();
        let kind = ComposerHistoryKind::classify_text(&draft.text);
        let local_history = match kind {
            ComposerHistoryKind::Prompt => &mut self.local_input_history,
            ComposerHistoryKind::Command => &mut self.local_command_history,
        };
        if local_history.last().is_some_and(|existing| {
            submitted_prompt_snapshot_from_draft(existing)
                == submitted_prompt_snapshot_from_draft(&draft)
        }) {
            return false;
        }
        local_history.push(draft);
        if local_history.len() > input_history::MAX_COMPOSER_HISTORY_ENTRIES {
            let overflow = local_history.len() - input_history::MAX_COMPOSER_HISTORY_ENTRIES;
            local_history.drain(0..overflow);
        }
        true
    }

    pub(crate) fn replace_input(&mut self, input: impl Into<String>) {
        self.replace_input_draft(ComposerDraftState::from_text(input));
    }

    pub(crate) fn clear_input(&mut self) {
        self.replace_input_draft(ComposerDraftState::default());
    }

    pub(crate) fn set_live_task_finished_hint(
        &mut self,
        task_id: agent::types::TaskId,
        status: agent::types::TaskStatus,
    ) {
        self.composer_context_hint =
            Some(ComposerContextHint::LiveTaskFinished { task_id, status });
    }

    pub(crate) fn clear_composer_context_hint(&mut self) {
        self.composer_context_hint = None;
    }

    pub(crate) fn restore_input_draft(&mut self, draft: ComposerDraftState) {
        self.replace_input_draft(draft);
    }

    pub(crate) fn push_input_char(&mut self, ch: char) {
        self.input.insert(self.input_cursor, ch);
        self.input_cursor += ch.len_utf8();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.input_vertical_column = None;
        self.reset_composer_completion();
    }

    pub(crate) fn push_input_str(&mut self, text: &str) {
        self.input.insert_str(self.input_cursor, text);
        self.input_cursor += text.len();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.input_vertical_column = None;
        self.reset_composer_completion();
    }

    pub(crate) fn append_input_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.input.push_str(text);
        self.input_cursor = self.input.len();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.input_vertical_column = None;
        self.reset_composer_completion();
    }

    pub(crate) fn push_large_paste(&mut self, payload: &str) -> String {
        let placeholder = format!("[Paste #{}]", self.next_pending_paste_index());
        self.draft_attachments.push(ComposerDraftAttachmentState {
            placeholder: Some(placeholder.clone()),
            kind: ComposerDraftAttachmentKind::LargePaste {
                payload: payload.to_string(),
            },
        });
        self.push_input_str(&placeholder);
        placeholder
    }

    pub(crate) fn push_row_attachment(&mut self, attachment: ComposerDraftAttachmentState) -> bool {
        if !attachment.is_row_attachment() {
            return false;
        }
        if self
            .draft_attachments
            .iter()
            .any(|existing| existing.same_persisted_attachment(&attachment))
        {
            return false;
        }
        self.draft_attachments.push(attachment);
        self.renormalize_draft_attachment_placeholders();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
        true
    }

    pub(crate) fn push_inline_attachment(
        &mut self,
        mut attachment: ComposerDraftAttachmentState,
    ) -> bool {
        if attachment.is_row_attachment() {
            attachment.placeholder = attachment.default_inline_placeholder();
        }
        let Some(placeholder) = attachment.placeholder.clone() else {
            return false;
        };
        if self
            .draft_attachments
            .iter()
            .any(|existing| existing.same_persisted_attachment(&attachment))
        {
            return false;
        }
        self.draft_attachments.push(attachment);
        self.push_input_str(&placeholder);
        self.renormalize_draft_attachment_placeholders();
        true
    }

    pub(crate) fn remove_row_attachment(
        &mut self,
        index: Option<usize>,
    ) -> Option<ComposerDraftAttachmentState> {
        let row_indices = self
            .draft_attachments
            .iter()
            .enumerate()
            .filter_map(|(index, attachment)| attachment.is_row_attachment().then_some(index))
            .collect::<Vec<_>>();
        let target = match index {
            Some(index) if index > 0 => row_indices.get(index - 1).copied(),
            Some(_) => None,
            None => row_indices.last().copied(),
        }?;
        let removed = self.draft_attachments.remove(target);
        self.renormalize_draft_attachment_placeholders();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
        Some(removed)
    }

    pub(crate) fn move_row_attachment(&mut self, from: usize, to: usize) -> bool {
        if from == 0 || to == 0 || from == to {
            return false;
        }

        let row_count = self.row_attachment_count();
        if from > row_count || to > row_count {
            return false;
        }

        let mut row_attachments = self
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.is_row_attachment())
            .cloned()
            .collect::<Vec<_>>();
        let inline_attachments = self
            .draft_attachments
            .iter()
            .filter(|attachment| !attachment.is_row_attachment())
            .cloned()
            .collect::<Vec<_>>();
        let moved = row_attachments.remove(from - 1);
        row_attachments.insert(to - 1, moved);
        self.draft_attachments = row_attachments
            .into_iter()
            .chain(inline_attachments)
            .collect();
        self.renormalize_draft_attachment_placeholders();
        self.selected_row_attachment = Some(to - 1);
        self.input_history_navigation = None;
        self.reset_composer_completion();
        true
    }

    pub(crate) fn row_attachment_previews(&self) -> Vec<ComposerRowAttachmentPreview> {
        summarize_row_attachments(&self.draft_attachments)
    }

    pub(crate) fn row_attachment_summaries(&self) -> Vec<(usize, String, String)> {
        self.row_attachment_previews()
            .into_iter()
            .map(|preview| (preview.index, preview.summary, preview.detail))
            .collect()
    }

    pub(crate) fn selected_row_attachment_preview(&self) -> Option<ComposerRowAttachmentPreview> {
        let selected = self.selected_row_attachment?;
        self.row_attachment_previews().into_iter().nth(selected)
    }

    #[cfg(test)]
    pub(crate) fn selected_row_attachment_summary(&self) -> Option<(usize, String, String)> {
        self.selected_row_attachment_preview()
            .map(|preview| (preview.index, preview.summary, preview.detail))
    }

    pub(crate) fn row_attachment_preview(
        &self,
        index: Option<usize>,
    ) -> Option<ComposerRowAttachmentPreview> {
        match index {
            Some(index) if index > 0 => self
                .row_attachment_previews()
                .into_iter()
                .find(|preview| preview.index == index),
            Some(_) => None,
            None => self.row_attachment_previews().into_iter().last(),
        }
    }

    pub(crate) fn select_previous_row_attachment(&mut self) -> bool {
        let row_count = self.row_attachment_count();
        if row_count == 0 {
            return false;
        }

        self.selected_row_attachment = Some(match self.selected_row_attachment {
            Some(selected) => selected.saturating_sub(1),
            None if self.input_cursor == 0 => row_count - 1,
            None => return false,
        });
        self.input_vertical_column = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
        true
    }

    pub(crate) fn select_next_row_attachment(&mut self) -> bool {
        let row_count = self.row_attachment_count();
        let Some(selected) = self.selected_row_attachment else {
            return false;
        };
        if selected + 1 < row_count {
            self.selected_row_attachment = Some(selected + 1);
        } else {
            self.selected_row_attachment = None;
        }
        self.input_vertical_column = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
        true
    }

    pub(crate) fn remove_selected_row_attachment(
        &mut self,
    ) -> Option<ComposerDraftAttachmentState> {
        let selected = self.selected_row_attachment?;
        let row_indices = self
            .draft_attachments
            .iter()
            .enumerate()
            .filter_map(|(index, attachment)| attachment.is_row_attachment().then_some(index))
            .collect::<Vec<_>>();
        let target = row_indices.get(selected).copied()?;
        let removed = self.draft_attachments.remove(target);
        self.renormalize_draft_attachment_placeholders();
        let remaining = row_indices.len().saturating_sub(1);
        self.selected_row_attachment = if remaining == 0 {
            None
        } else {
            Some(selected.min(remaining - 1))
        };
        self.input_history_navigation = None;
        self.reset_composer_completion();
        Some(removed)
    }

    pub(crate) fn pop_input_char(&mut self) {
        let Some(previous_index) = previous_char_boundary(&self.input, self.input_cursor) else {
            return;
        };
        self.input.drain(previous_index..self.input_cursor);
        self.input_cursor = previous_index;
        self.prune_draft_attachments();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.input_vertical_column = None;
        self.reset_composer_completion();
    }

    pub(crate) fn kill_input_to_end(&mut self) -> bool {
        if self.input_cursor >= self.input.len() {
            return false;
        }

        let killed_text = self.input[self.input_cursor..].to_string();
        if killed_text.is_empty() {
            return false;
        }

        self.kill_buffer = Some(ComposerKillBufferState {
            text: killed_text,
            draft_attachments: self.draft_attachments_for_text(&self.input[self.input_cursor..]),
        });
        self.input.truncate(self.input_cursor);
        self.prune_draft_attachments();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
        true
    }

    pub(crate) fn yank_kill_buffer(&mut self) -> bool {
        let Some(kill_buffer) = self.kill_buffer.clone() else {
            return false;
        };
        if kill_buffer.text.is_empty() {
            return false;
        }

        self.push_input_str(&kill_buffer.text);
        for attachment in kill_buffer.draft_attachments {
            if self
                .draft_attachments
                .iter()
                .all(|existing| existing.placeholder != attachment.placeholder)
            {
                self.draft_attachments.push(attachment);
            }
        }
        true
    }

    pub(crate) fn move_input_cursor_left(&mut self) -> bool {
        self.selected_row_attachment = None;
        let Some(previous_index) = previous_char_boundary(&self.input, self.input_cursor) else {
            return false;
        };
        self.input_cursor = previous_index;
        self.input_vertical_column = None;
        true
    }

    pub(crate) fn move_input_cursor_right(&mut self) -> bool {
        self.selected_row_attachment = None;
        let Some(next_index) = next_char_boundary(&self.input, self.input_cursor) else {
            return false;
        };
        self.input_cursor = next_index;
        self.input_vertical_column = None;
        true
    }

    pub(crate) fn move_input_cursor_home(&mut self) -> bool {
        self.selected_row_attachment = None;
        if self.input_cursor == 0 {
            return false;
        }
        self.input_cursor = 0;
        self.input_vertical_column = None;
        true
    }

    pub(crate) fn move_input_cursor_end(&mut self) -> bool {
        self.selected_row_attachment = None;
        if self.input_cursor == self.input.len() {
            return false;
        }
        self.input_cursor = self.input.len();
        self.input_vertical_column = None;
        true
    }

    pub(crate) fn move_input_cursor_vertical(&mut self, backwards: bool) -> bool {
        self.selected_row_attachment = None;
        let cursor = normalize_input_cursor(&self.input, self.input_cursor);
        let current_line = line_range_for_cursor(&self.input, cursor);
        let target_line = if backwards {
            previous_line_range(&self.input, current_line.start)
        } else {
            next_line_range(&self.input, current_line.end)
        };
        let Some(target_line) = target_line else {
            return false;
        };

        let current_column = display_width(&self.input[current_line.start..cursor]);
        let desired_column = self.input_vertical_column.unwrap_or(current_column);
        self.input_cursor = target_line.start
            + byte_index_for_display_column(&self.input[target_line.clone()], desired_column);
        self.input_vertical_column = Some(desired_column);
        true
    }

    #[cfg(test)]
    pub(crate) fn input_cursor(&self) -> usize {
        self.input_cursor
    }

    pub(crate) fn input_cursor_at_history_boundary(&self) -> bool {
        self.input.is_empty() || self.input_cursor == 0 || self.input_cursor == self.input.len()
    }

    pub(crate) fn browse_input_history(&mut self, backwards: bool) -> bool {
        if self.input_history_navigation.is_none() && !self.input_cursor_at_history_boundary() {
            return false;
        }

        let mode = self
            .input_history_navigation
            .as_ref()
            .map(|navigation| navigation.mode)
            .unwrap_or_else(|| self.history_browse_mode());
        let history = self.history_entry_drafts_for_mode(mode);
        if history.is_empty() {
            return false;
        }

        if backwards {
            let (next_index, draft) = match self.input_history_navigation.as_ref() {
                Some(navigation) => (navigation.index.saturating_sub(1), navigation.draft.clone()),
                None => (history.len() - 1, self.current_input_draft()),
            };
            self.replace_input_draft(history[next_index].clone());
            self.input_history_navigation = Some(ComposerHistoryNavigationState {
                mode,
                index: next_index,
                draft,
            });
            return true;
        }

        let Some(navigation) = self.input_history_navigation.clone() else {
            return false;
        };
        if navigation.index + 1 < history.len() {
            self.replace_input_draft(history[navigation.index + 1].clone());
            self.input_history_navigation = Some(ComposerHistoryNavigationState {
                mode: navigation.mode,
                index: navigation.index + 1,
                draft: navigation.draft,
            });
        } else {
            self.replace_input_draft(navigation.draft);
            self.input_history_navigation = None;
        }
        true
    }

    pub(crate) fn reset_composer_completion(&mut self) {
        self.composer_completion_index = 0;
    }

    fn replace_input_draft(&mut self, draft: ComposerDraftState) {
        let draft = draft.normalized();
        self.input = draft.text;
        self.input_cursor = draft.cursor;
        self.input_vertical_column = None;
        self.draft_attachments = draft.draft_attachments;
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
    }

    fn current_input_draft(&self) -> ComposerDraftState {
        ComposerDraftState {
            text: self.input.clone(),
            cursor: self.input_cursor,
            draft_attachments: self.draft_attachments.clone(),
        }
        .normalized()
    }

    fn history_browse_mode(&self) -> ComposerHistoryBrowseMode {
        if self.input.trim_start().starts_with('/') {
            ComposerHistoryBrowseMode::CommandOnly
        } else if self.persisted_history_entries.is_empty() {
            ComposerHistoryBrowseMode::PromptOnly
        } else {
            ComposerHistoryBrowseMode::Combined
        }
    }

    fn history_entry_drafts_for_mode(
        &self,
        mode: ComposerHistoryBrowseMode,
    ) -> Vec<ComposerDraftState> {
        let persistent_history = match mode {
            ComposerHistoryBrowseMode::PromptOnly => self
                .input_history
                .iter()
                .cloned()
                .map(|snapshot| composer_draft_from_prompt_snapshot(&snapshot))
                .collect::<Vec<_>>(),
            ComposerHistoryBrowseMode::CommandOnly => self
                .command_history
                .iter()
                .cloned()
                .map(|snapshot| composer_draft_from_prompt_snapshot(&snapshot))
                .collect::<Vec<_>>(),
            ComposerHistoryBrowseMode::Combined => self
                .persisted_history_entries
                .iter()
                .cloned()
                .map(|entry| composer_draft_from_prompt_snapshot(&entry.prompt))
                .collect::<Vec<_>>(),
        };
        let local_history = match mode {
            ComposerHistoryBrowseMode::CommandOnly => &self.local_command_history,
            ComposerHistoryBrowseMode::PromptOnly | ComposerHistoryBrowseMode::Combined => {
                &self.local_input_history
            }
        };

        let mut entries = persistent_history;
        if local_history.is_empty() {
            return entries;
        }

        // Local history retains richer in-session draft state. When it matches
        // the persistent suffix, replace the plain-text entries instead of
        // recalling duplicate prompts back-to-back.
        let shared_suffix = local_history
            .iter()
            .rev()
            .zip(entries.iter().rev())
            .take_while(|(local, persistent)| {
                submitted_prompt_snapshot_from_draft(local)
                    == submitted_prompt_snapshot_from_draft(persistent)
            })
            .count();
        entries.truncate(entries.len().saturating_sub(shared_suffix));
        entries.extend(local_history.iter().cloned());
        entries
    }

    fn next_pending_paste_index(&self) -> usize {
        self.draft_attachments
            .iter()
            .filter_map(|attachment| {
                attachment
                    .placeholder
                    .as_deref()
                    .and_then(|placeholder| placeholder.strip_prefix("[Paste #"))
                    .and_then(|rest| rest.strip_suffix(']'))
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    fn draft_attachments_for_text(&self, text: &str) -> Vec<ComposerDraftAttachmentState> {
        self.draft_attachments
            .iter()
            .filter(|attachment| {
                attachment
                    .placeholder
                    .as_ref()
                    .is_some_and(|placeholder| text.contains(placeholder))
            })
            .cloned()
            .collect()
    }

    fn prune_draft_attachments(&mut self) {
        self.draft_attachments.retain(|attachment| {
            attachment
                .placeholder
                .as_ref()
                .is_none_or(|placeholder| self.input.contains(placeholder))
        });
        self.renormalize_draft_attachment_placeholders();
    }

    pub(super) fn take_submission_input(&mut self) -> String {
        let text = self.expanded_input_text();
        let _ = self.take_submission();
        text
    }

    fn expanded_input_text(&self) -> String {
        let mut expanded = self.input.clone();
        for attachment in &self.draft_attachments {
            let Some(placeholder) = attachment.placeholder.as_ref() else {
                continue;
            };
            let replacement = match &attachment.kind {
                ComposerDraftAttachmentKind::LargePaste { payload } => payload.clone(),
                ComposerDraftAttachmentKind::LocalImage {
                    part: Some(part), ..
                }
                | ComposerDraftAttachmentKind::RemoteImage { part, .. }
                | ComposerDraftAttachmentKind::LocalFile {
                    part: Some(part), ..
                }
                | ComposerDraftAttachmentKind::RemoteFile { part, .. } => {
                    agent::types::message_part_operator_text(part)
                }
                ComposerDraftAttachmentKind::LocalImage {
                    requested_path,
                    mime_type,
                    ..
                } => format!(
                    "[image:{}{}]",
                    requested_path,
                    mime_type
                        .as_deref()
                        .map(|mime| format!(" {mime}"))
                        .unwrap_or_default()
                ),
                ComposerDraftAttachmentKind::LocalFile {
                    requested_path,
                    file_name,
                    mime_type,
                    ..
                } => agent::types::file_display_text(
                    file_name.as_deref(),
                    mime_type.as_deref(),
                    Some(requested_path),
                ),
            };
            expanded = expanded.replace(placeholder, &replacement);
        }
        expanded
    }

    pub(super) fn take_submission(&mut self) -> ComposerSubmission {
        let submission = self.current_submission();
        self.input_history_navigation = None;
        self.composer_completion_index = 0;
        self.input_cursor = 0;
        self.input_vertical_column = None;
        let _ = std::mem::take(&mut self.input);
        self.draft_attachments.clear();
        submission
    }

    fn current_submission(&self) -> ComposerSubmission {
        let local_history_draft = self.current_input_draft();
        let prompt_snapshot = submitted_prompt_snapshot_from_draft(&local_history_draft);
        ComposerSubmission {
            prompt_snapshot,
            local_history_draft,
        }
    }

    fn renormalize_draft_attachment_placeholders(&mut self) {
        normalize_attachment_placeholders(&mut self.input, &mut self.draft_attachments);
        self.input_cursor =
            normalize_input_cursor(&self.input, self.input_cursor.min(self.input.len()));
    }

    pub(crate) fn apply_external_edit(
        &mut self,
        text: impl Into<String>,
    ) -> ComposerAttachmentEditSummary {
        let before_rows = self.row_attachment_previews();
        let (row_attachments, text) = self.parse_external_editor_text(text.into());
        let mut inline_attachments = self
            .draft_attachments
            .iter()
            .filter_map(|attachment| {
                let placeholder = attachment.placeholder.as_ref()?;
                text.find(placeholder)
                    .map(|index| (index, attachment.clone()))
            })
            .collect::<Vec<_>>();
        inline_attachments.sort_by_key(|(index, _)| *index);

        self.input = text;
        self.input_cursor = self.input.len();
        self.input_vertical_column = None;
        self.draft_attachments = row_attachments
            .into_iter()
            .chain(
                inline_attachments
                    .into_iter()
                    .map(|(_, attachment)| attachment),
            )
            .collect();
        self.renormalize_draft_attachment_placeholders();
        self.selected_row_attachment = None;
        self.input_history_navigation = None;
        self.reset_composer_completion();
        summarize_attachment_edit(&before_rows, &self.row_attachment_previews())
    }

    fn parse_external_editor_text(
        &self,
        text: String,
    ) -> (Vec<ComposerDraftAttachmentState>, String) {
        let row_attachments = self
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.is_row_attachment())
            .cloned()
            .collect::<Vec<_>>();
        if row_attachments.is_empty() {
            return (row_attachments, text);
        }

        let lines = text.lines().map(str::to_string).collect::<Vec<_>>();
        if lines.first().map(String::as_str) != Some("[Attachments]") {
            return (row_attachments, text);
        }
        let Some(prompt_index) = lines.iter().position(|line| line == "[Prompt]") else {
            return (row_attachments, text);
        };

        // Only the attachment prelude participates in row replay. The prompt
        // body stays plain text so inline placeholders like `[Paste #N]` can be
        // rebound separately without leaking row markers into the live draft.
        let rebound_rows = lines[1..prompt_index]
            .iter()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                row_attachments
                    .iter()
                    .enumerate()
                    .find_map(|(index, attachment)| {
                        let token = attachment.external_editor_row_token(index + 1)?;
                        trimmed.starts_with(&token).then(|| attachment.clone())
                    })
            })
            .fold(Vec::new(), |mut rebound, attachment| {
                if !rebound.iter().any(|existing| existing == &attachment) {
                    rebound.push(attachment);
                }
                rebound
            });
        let prompt_text = lines[prompt_index + 1..].join("\n");
        (rebound_rows, prompt_text)
    }

    fn row_attachment_count(&self) -> usize {
        self.draft_attachments
            .iter()
            .filter(|attachment| attachment.is_row_attachment())
            .count()
    }
}

fn record_persistent_history_entry(
    entries: &mut Vec<SubmittedPromptSnapshot>,
    kind: ComposerHistoryKind,
    prompt: SubmittedPromptSnapshot,
) -> bool {
    let Some(entry) = input_history::normalized_history_entry(kind, prompt) else {
        return false;
    };
    if entries.last() == Some(&entry.prompt) {
        return false;
    }
    entries.push(entry.prompt);
    if entries.len() > input_history::MAX_COMPOSER_HISTORY_ENTRIES {
        let overflow = entries.len() - input_history::MAX_COMPOSER_HISTORY_ENTRIES;
        entries.drain(0..overflow);
    }
    true
}

fn summarize_row_attachments(
    attachments: &[ComposerDraftAttachmentState],
) -> Vec<ComposerRowAttachmentPreview> {
    attachments
        .iter()
        .filter(|attachment| attachment.is_row_attachment())
        .enumerate()
        .filter_map(|(index, attachment)| {
            Some(ComposerRowAttachmentPreview {
                index: index + 1,
                summary: attachment.row_summary()?,
                detail: attachment.row_detail().unwrap_or_default(),
            })
        })
        .collect()
}

fn default_inline_placeholder(kind: &ComposerDraftAttachmentKind) -> Option<String> {
    match kind {
        ComposerDraftAttachmentKind::LargePaste { .. } => Some("[Paste #1]".to_string()),
        ComposerDraftAttachmentKind::LocalImage { .. } => Some("[Image #1]".to_string()),
        ComposerDraftAttachmentKind::LocalFile { .. } => Some("[File #1]".to_string()),
        ComposerDraftAttachmentKind::RemoteImage { .. }
        | ComposerDraftAttachmentKind::RemoteFile { .. } => None,
    }
}

fn normalize_attachment_placeholders(
    text: &mut String,
    attachments: &mut [ComposerDraftAttachmentState],
) {
    let mut image_index = attachments
        .iter()
        .filter(|attachment| {
            matches!(
                attachment.kind,
                ComposerDraftAttachmentKind::RemoteImage { .. }
            )
        })
        .count();
    let mut file_index = attachments
        .iter()
        .filter(|attachment| {
            matches!(
                attachment.kind,
                ComposerDraftAttachmentKind::RemoteFile { .. }
            )
        })
        .count();
    let mut paste_index = 0;
    let mut replacements = Vec::new();

    for (position, attachment) in attachments.iter_mut().enumerate() {
        let next_placeholder = match (&attachment.kind, attachment.placeholder.as_ref()) {
            (ComposerDraftAttachmentKind::LargePaste { .. }, _) => {
                paste_index += 1;
                Some(format!("[Paste #{paste_index}]"))
            }
            (ComposerDraftAttachmentKind::LocalImage { .. }, Some(_)) => {
                image_index += 1;
                Some(format!("[Image #{image_index}]"))
            }
            (ComposerDraftAttachmentKind::LocalFile { .. }, Some(_)) => {
                file_index += 1;
                Some(format!("[File #{file_index}]"))
            }
            (ComposerDraftAttachmentKind::LocalImage { .. }, None)
            | (ComposerDraftAttachmentKind::LocalFile { .. }, None)
            | (ComposerDraftAttachmentKind::RemoteImage { .. }, _)
            | (ComposerDraftAttachmentKind::RemoteFile { .. }, _) => None,
        };

        match (attachment.placeholder.clone(), next_placeholder) {
            (Some(current), Some(updated)) if current != updated => {
                let temporary = format!("[[NANOCLAW_ATTACH_REBASE_{position}]]");
                *text = text.replace(&current, &temporary);
                attachment.placeholder = Some(updated.clone());
                replacements.push((temporary, updated));
            }
            (None, Some(updated)) => {
                attachment.placeholder = Some(updated);
            }
            (_, None) => attachment.placeholder = None,
            _ => {}
        }
    }

    for (temporary, placeholder) in replacements {
        *text = text.replace(&temporary, &placeholder);
    }
}

fn summarize_attachment_edit(
    before: &[ComposerRowAttachmentPreview],
    after: &[ComposerRowAttachmentPreview],
) -> ComposerAttachmentEditSummary {
    let after_keys = after
        .iter()
        .map(preview_identity_key)
        .collect::<std::collections::BTreeSet<_>>();
    let detached = before
        .iter()
        .filter(|preview| !after_keys.contains(&preview_identity_key(preview)))
        .cloned()
        .collect::<Vec<_>>();

    let before_common = before
        .iter()
        .filter(|preview| after_keys.contains(&preview_identity_key(preview)))
        .map(preview_identity_key)
        .collect::<Vec<_>>();
    let after_common = after.iter().map(preview_identity_key).collect::<Vec<_>>();

    ComposerAttachmentEditSummary {
        detached,
        reordered: before_common != after_common,
    }
}

fn preview_identity_key(preview: &ComposerRowAttachmentPreview) -> String {
    format!("{}\u{0}{}", preview.summary, preview.detail)
}

fn preview_path_tail(path: &str) -> String {
    if let Some(segment) = remote_url_tail_segment(path) {
        return segment;
    }
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn is_remote_url(path: &str) -> bool {
    matches!(path.trim(), value if value.starts_with("http://") || value.starts_with("https://"))
}

fn remote_url_tail_segment(path: &str) -> Option<String> {
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

fn normalize_input_cursor(input: &str, cursor: usize) -> usize {
    if cursor >= input.len() {
        return input.len();
    }
    if input.is_char_boundary(cursor) {
        return cursor;
    }
    previous_char_boundary(input, cursor).unwrap_or(0)
}

fn line_range_for_cursor(input: &str, cursor: usize) -> std::ops::Range<usize> {
    let cursor = normalize_input_cursor(input, cursor);
    let start = input[..cursor]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = input[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(input.len());
    start..end
}

fn previous_line_range(input: &str, current_line_start: usize) -> Option<std::ops::Range<usize>> {
    if current_line_start == 0 {
        return None;
    }
    let previous_end = current_line_start.saturating_sub(1);
    let previous_start = input[..previous_end]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    Some(previous_start..previous_end)
}

fn next_line_range(input: &str, current_line_end: usize) -> Option<std::ops::Range<usize>> {
    if current_line_end >= input.len() {
        return None;
    }
    let next_start = current_line_end + 1;
    let next_end = input[next_start..]
        .find('\n')
        .map(|offset| next_start + offset)
        .unwrap_or(input.len());
    Some(next_start..next_end)
}

fn byte_index_for_display_column(line: &str, desired_column: usize) -> usize {
    if desired_column == 0 {
        return 0;
    }

    let mut width = 0;
    for (index, ch) in line.char_indices() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + char_width > desired_column {
            return index;
        }
        width += char_width;
    }
    line.len()
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn previous_char_boundary(input: &str, cursor: usize) -> Option<usize> {
    if cursor == 0 {
        return None;
    }
    input[..normalize_input_cursor(input, cursor)]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

fn next_char_boundary(input: &str, cursor: usize) -> Option<usize> {
    let cursor = normalize_input_cursor(input, cursor);
    if cursor >= input.len() {
        return None;
    }
    input[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
}
