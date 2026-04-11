use super::*;

pub(crate) fn plain_input_submit_action(
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

pub(crate) fn merge_interrupt_steers(steers: Vec<String>) -> Option<String> {
    if steers.is_empty() {
        None
    } else {
        Some(steers.join("\n"))
    }
}

pub(crate) fn build_history_rollback_candidates(
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

pub(crate) fn history_rollback_status(
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

pub(crate) fn pending_control_kind_label(
    kind: crate::interaction::PendingControlKind,
) -> &'static str {
    match kind {
        crate::interaction::PendingControlKind::Prompt => "prompt",
        crate::interaction::PendingControlKind::Steer => "steer",
    }
}

pub(crate) fn composer_has_prompt_content(state: &TuiState) -> bool {
    !state.input.trim().is_empty() || !state.draft_attachments.is_empty()
}

pub(crate) fn composer_requires_prompt_submission(state: &TuiState) -> bool {
    state.draft_attachments.iter().any(|attachment| {
        !matches!(
            attachment.kind,
            ComposerDraftAttachmentKind::LargePaste { .. }
        )
    })
}

pub(crate) fn composer_uses_image_input(state: &TuiState) -> bool {
    state.draft_attachments.iter().any(|attachment| {
        matches!(
            attachment.kind,
            ComposerDraftAttachmentKind::LocalImage { .. }
                | ComposerDraftAttachmentKind::RemoteImage { .. }
        )
    })
}

pub(crate) fn queued_command_preview(command: &RuntimeCommand) -> String {
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
