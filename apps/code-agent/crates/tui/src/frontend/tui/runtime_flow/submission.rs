use super::*;

impl CodeAgentTui {
    pub(crate) async fn apply_plain_input_submit(
        &mut self,
        action: PlainInputSubmitAction,
        submission: ComposerSubmission,
    ) {
        self.record_submitted_prompt(&submission);
        let message = match self
            .materialize_prompt_message(&submission.local_history_draft)
            .await
        {
            Ok(message) => message,
            Err(error) => {
                let message = summarize_nonfatal_error("materialize prompt", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Failed to prepare prompt: {message}");
                    state.push_activity(format!(
                        "failed to prepare prompt: {}",
                        state::preview_text(&message, 56)
                    ));
                });
                return;
            }
        };
        match action {
            PlainInputSubmitAction::StartPrompt => {
                self.start_turn_message(message, Some(submission.prompt_snapshot))
                    .await
            }
            PlainInputSubmitAction::QueuePrompt => {
                self.queue_prompt_behind_active_turn_message(
                    message,
                    Some(submission.prompt_snapshot),
                )
                .await;
            }
            PlainInputSubmitAction::SteerActiveTurn => {
                self.schedule_runtime_steer_while_active(
                    submission.prompt_snapshot.text,
                    Some("inline_enter".to_string()),
                )
                .await;
            }
        }
    }

    pub(crate) async fn materialize_prompt_message(
        &self,
        draft: &ComposerDraftState,
    ) -> Result<Message> {
        let mut parts = Vec::new();
        for attachment in draft
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.placeholder.is_none())
        {
            parts.extend(self.materialize_attachment_parts(attachment).await?);
        }

        let mut remaining = draft.text.as_str();
        while !remaining.is_empty() {
            let next_attachment = draft
                .draft_attachments
                .iter()
                .filter_map(|attachment| {
                    let placeholder = attachment.placeholder.as_ref()?;
                    remaining.find(placeholder).map(|index| (index, attachment))
                })
                .min_by_key(|(index, _)| *index);
            let Some((index, attachment)) = next_attachment else {
                parts.push(MessagePart::inline_text(remaining));
                break;
            };

            if index > 0 {
                parts.push(MessagePart::inline_text(&remaining[..index]));
            }
            parts.extend(self.materialize_attachment_parts(attachment).await?);
            remaining = &remaining[index
                + attachment
                    .placeholder
                    .as_ref()
                    .expect("inline attachment placeholder")
                    .len()..];
        }

        Ok(if parts.is_empty() {
            Message::user("")
        } else {
            Message::new(agent::types::MessageRole::User, parts)
        })
    }

    pub(crate) async fn materialize_attachment_parts(
        &self,
        attachment: &ComposerDraftAttachmentState,
    ) -> Result<Vec<MessagePart>> {
        Ok(match &attachment.kind {
            ComposerDraftAttachmentKind::LargePaste { payload } => {
                let label = attachment
                    .placeholder
                    .clone()
                    .unwrap_or_else(|| "[Paste]".to_string());
                vec![MessagePart::paste(label, payload.clone())]
            }
            ComposerDraftAttachmentKind::LocalImage {
                part: Some(part), ..
            }
            | ComposerDraftAttachmentKind::LocalFile {
                part: Some(part), ..
            }
            | ComposerDraftAttachmentKind::RemoteImage { part, .. }
            | ComposerDraftAttachmentKind::RemoteFile { part, .. } => vec![part.clone()],
            ComposerDraftAttachmentKind::LocalImage {
                requested_path,
                part: None,
                ..
            } => vec![
                load_tool_image(requested_path, &self.composer_attachment_context())
                    .await?
                    .message_part(),
            ],
            ComposerDraftAttachmentKind::LocalFile {
                requested_path,
                file_name,
                mime_type,
                part: None,
            } => {
                let file =
                    load_composer_file(requested_path, &self.composer_attachment_context()).await?;
                vec![MessagePart::File {
                    file_name: file_name.clone().or(file.file_name),
                    mime_type: mime_type.clone().or(file.mime_type),
                    data_base64: Some(file.data_base64),
                    uri: Some(file.requested_path),
                }]
            }
        })
    }

    pub(crate) async fn try_apply_pending_control_edit(&mut self, input: &str) -> bool {
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
        match self.update_pending_control(&editing.id, content) {
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
}
