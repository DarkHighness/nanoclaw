use super::theme::{ACCENT, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub(super) fn build_welcome_lines(state: &TuiState, viewport_height: u16) -> Vec<Line<'static>> {
    let compact = viewport_height < 22;
    let mut core = build_welcome_logo_lines(compact);
    core.push(Line::raw(""));
    core.push(meta_line("workspace", &state.session.workspace_name, MUTED));
    core.push(meta_line("model", &model_label(state), ACCENT));
    core.push(Line::raw(""));
    core.push(Line::from(vec![
        Span::styled("Type a prompt", Style::default().fg(TEXT)),
        Span::styled(" or ", Style::default().fg(SUBTLE)),
        Span::styled("/help", Style::default().fg(ACCENT)),
        Span::styled(".", Style::default().fg(SUBTLE)),
    ]));

    let top_padding = usize::from(viewport_height.saturating_sub(core.len() as u16) / 2);
    let mut lines = vec![Line::raw(""); top_padding];
    lines.extend(core);
    lines
}

fn build_welcome_logo_lines(compact: bool) -> Vec<Line<'static>> {
    if compact {
        return vec![Line::from(vec![
            Span::styled(
                "NANO".to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "CLAW".to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])];
    }

    let rows = [
        (
            " _   _    _    _   _   ___ ",
            "   ____ _        _    __        __",
        ),
        (
            "| \\ | |  / \\  | \\ | | / _ \\",
            " / ___| |      / \\   \\ \\      / /",
        ),
        (
            "|  \\| | / _ \\ |  \\| || | | |",
            "| |   | |     / _ \\   \\ \\ /\\ / / ",
        ),
        (
            "| |\\  |/ ___ \\| |\\  || |_| |",
            "| |___| |___ / ___ \\   \\ V  V /  ",
        ),
        (
            "|_| \\_/_/   \\_\\_| \\_| \\___/ ",
            "\\____|_____/_/   \\_\\   \\_/\\_/   ",
        ),
    ];

    let mut lines = Vec::with_capacity(rows.len() * 2);
    for (left, right) in rows {
        lines.push(wordmark_line(left, right));
        lines.push(wordmark_shadow_line(left, right));
    }
    lines
}

fn meta_line(label: &str, value: &str, value_color: ratatui::style::Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_string(), Style::default().fg(SUBTLE)),
        Span::styled("  ", Style::default().fg(SUBTLE)),
        Span::styled(value.to_string(), Style::default().fg(value_color)),
    ])
}

fn model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) => format!("{} · {}", state.session.model, effort),
        None => state.session.model.clone(),
    }
}

fn wordmark_line(left: &'static str, right: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            left.to_string(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            right.to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn wordmark_shadow_line(left: &'static str, right: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  ", Style::default().fg(SUBTLE)),
        Span::styled(format!("{left}{right}"), Style::default().fg(SUBTLE)),
    ])
}
