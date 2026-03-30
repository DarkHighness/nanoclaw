use unicode_width::UnicodeWidthStr;

pub(super) fn composer_cursor_width(input: &str) -> u16 {
    UnicodeWidthStr::width(input).min(u16::MAX as usize) as u16
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
