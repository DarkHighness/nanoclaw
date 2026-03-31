use crate::backend::PendingControlKind;
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

pub(super) fn pending_control_reason_label(reason: Option<&str>) -> Option<String> {
    let reason = reason.map(str::trim).filter(|value| !value.is_empty())?;
    Some(match reason {
        "inline_enter" => "from Enter while running".to_string(),
        "manual_command" => "from /steer".to_string(),
        _ => reason.replace('_', " "),
    })
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
