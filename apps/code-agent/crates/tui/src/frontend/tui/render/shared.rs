use crate::interaction::{PendingControlKind, PendingControlReason, PendingControlSummary};
use std::cmp::min;
use unicode_width::UnicodeWidthStr;

pub(super) fn composer_cursor_width(input: &str) -> u16 {
    UnicodeWidthStr::width(input).min(u16::MAX as usize) as u16
}

pub(super) fn composer_cursor_metrics(input: &str, cursor: usize) -> (u16, u16) {
    let cursor = min(cursor, input.len());
    let prefix = &input[..cursor];
    let line = prefix.split('\n').count().saturating_sub(1);
    let column = prefix
        .rsplit_once('\n')
        .map(|(_, tail)| tail)
        .unwrap_or(prefix);
    (
        line.min(u16::MAX as usize) as u16,
        composer_cursor_width(column),
    )
}

pub(super) fn clamp_scroll(requested: u16, content_lines: usize, viewport_height: u16) -> u16 {
    let viewport = usize::from(viewport_height.max(1));
    let max_scroll = content_lines.saturating_sub(viewport);
    if requested == u16::MAX {
        max_scroll.min(u16::MAX as usize) as u16
    } else {
        usize::from(requested)
            .min(max_scroll)
            .min(u16::MAX as usize) as u16
    }
}

pub(super) fn pending_control_reason_label(
    reason: Option<&PendingControlReason>,
) -> Option<String> {
    reason.map(PendingControlReason::label)
}

pub(super) fn pending_control_kind_label(kind: PendingControlKind) -> &'static str {
    match kind {
        PendingControlKind::Prompt => "prompt",
        PendingControlKind::Steer => "steer",
    }
}

pub(super) fn pending_control_focus_label(selected_index: usize, total: usize) -> String {
    match (selected_index, total) {
        (_, 0) => "empty queue".to_string(),
        (_, 1) => "only item".to_string(),
        (0, _) => "next to run".to_string(),
        (index, count) if index + 1 == count => "latest draft".to_string(),
        (index, count) => format!("item {} of {}", index + 1, count),
    }
}

#[derive(Clone, Copy)]
pub(super) struct PendingControlKindSummary<'a> {
    pub(super) kind: PendingControlKind,
    pub(super) latest_index: usize,
    pub(super) latest: &'a PendingControlSummary,
    pub(super) count: usize,
}

pub(super) fn pending_controls_have_kind(
    controls: &[PendingControlSummary],
    kind: PendingControlKind,
) -> bool {
    controls.iter().any(|control| control.kind == kind)
}

pub(super) fn pending_control_kind_summaries(
    controls: &[PendingControlSummary],
) -> Vec<PendingControlKindSummary<'_>> {
    // Keep steer summaries ahead of queued prompts because steer is the
    // actionable live-turn control the operator may need to notice first.
    [PendingControlKind::Steer, PendingControlKind::Prompt]
        .into_iter()
        .filter_map(|kind| {
            let count = controls
                .iter()
                .filter(|control| control.kind == kind)
                .count();
            let (latest_index, latest) = controls
                .iter()
                .enumerate()
                .rev()
                .find(|(_, control)| control.kind == kind)?;
            Some(PendingControlKindSummary {
                kind,
                latest_index,
                latest,
                count,
            })
        })
        .collect()
}
