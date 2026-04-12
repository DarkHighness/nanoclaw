use super::*;

impl CodeAgentTui {
    pub(crate) async fn apply_skill_slash_submit(
        &mut self,
        skill_name: String,
        prompt: Option<String>,
    ) {
        let slash_input = synthesize_skill_prompt_input(&skill_name, prompt.as_deref());
        let snapshot = self.ui_state.snapshot();
        if prompt
            .as_deref()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
            && snapshot.draft_attachments.is_empty()
        {
            self.ui_state.mutate(|state| {
                state.replace_input_text_preserving_attachments(format!("{slash_input} "));
                state.composer_completion_index = 0;
                state.status = format!("Selected skill {}", skill_name);
                state.push_activity(format!("prepared skill prompt {}", skill_name));
            });
            return;
        }

        let action = plain_input_submit_action(
            &slash_input,
            true,
            composer_requires_prompt_submission(&snapshot),
            snapshot.turn_running,
            KeyCode::Enter,
        )
        .expect("skill slash submissions should map to plain prompt submission");
        self.ui_state.mutate(|state| {
            state.replace_input_text_preserving_attachments(slash_input);
            state.composer_completion_index = 0;
        });
        let submission = self.ui_state.take_submission();
        self.apply_plain_input_submit(action, submission).await;
    }

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
                    Some(crate::interaction::PendingControlReason::InlineEnter),
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
        let skills = self.ui_state.snapshot().session.skills;
        let (skill_references, prompt_text) = split_leading_skill_references(&draft.text, &skills);
        parts.extend(skill_references.into_iter().map(|skill| {
            MessagePart::reference("skill", Some(skill.name), None, Some(skill.description))
        }));
        for attachment in draft
            .draft_attachments
            .iter()
            .filter(|attachment| attachment.placeholder.is_none())
        {
            parts.extend(self.materialize_attachment_parts(attachment).await?);
        }

        let mut remaining = prompt_text.as_str();
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

pub(crate) fn synthesize_skill_prompt_input(skill_name: &str, prompt: Option<&str>) -> String {
    let prompt = prompt.map(str::trim).filter(|value| !value.is_empty());
    match prompt {
        Some(prompt) => format!("${skill_name} {prompt}"),
        None => format!("${skill_name}"),
    }
}

fn split_leading_skill_references(
    text: &str,
    skills: &[crate::interaction::SkillSummary],
) -> (Vec<crate::interaction::SkillSummary>, String) {
    let mut remaining = text.trim_start();
    let mut matched = Vec::new();

    while let Some(body) = remaining.strip_prefix('$') {
        let body = body.trim_start();
        let token_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let token = body[..token_end].trim();
        if token.is_empty() {
            break;
        }
        let Some(skill) = skills
            .iter()
            .find(|skill| skill.name == token || skill.aliases.iter().any(|alias| alias == token))
        else {
            break;
        };
        matched.push(skill.clone());
        remaining = body[token_end..].trim_start();
    }

    (matched, remaining.to_string())
}

#[cfg(test)]
mod tests {
    use super::{split_leading_skill_references, synthesize_skill_prompt_input};
    use crate::interaction::SkillSummary;

    fn skill(name: &str, aliases: &[&str]) -> SkillSummary {
        SkillSummary {
            name: name.to_string(),
            description: format!("Use {name}"),
            aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn extracts_multiple_leading_skill_directives() {
        let skills = vec![
            skill("openai-docs", &["docs"]),
            skill("frontend-design", &["ui"]),
        ];
        let (matched, remaining) = split_leading_skill_references(
            "$docs $frontend-design summarize the API differences",
            &skills,
        );

        assert_eq!(matched.len(), 2);
        assert_eq!(matched[0].name, "openai-docs");
        assert_eq!(matched[1].name, "frontend-design");
        assert_eq!(remaining, "summarize the API differences");
    }

    #[test]
    fn leaves_prompt_intact_when_skill_token_is_unknown() {
        let skills = vec![skill("openai-docs", &["docs"])];
        let (matched, remaining) = split_leading_skill_references("$unknown inspect this", &skills);

        assert!(matched.is_empty());
        assert_eq!(remaining, "$unknown inspect this");
    }

    #[test]
    fn synthesizes_skill_prompt_input_from_optional_tail() {
        assert_eq!(
            synthesize_skill_prompt_input("openai-docs", Some("summarize models")),
            "$openai-docs summarize models"
        );
        assert_eq!(
            synthesize_skill_prompt_input("openai-docs", None),
            "$openai-docs"
        );
    }
}
