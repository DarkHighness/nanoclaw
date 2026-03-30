use super::theme::{ACCENT, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const FULL_LOGO_LINES: [&str; 6] = [
    "▄▄     ▄▄▄    ▄▄       ▄▄     ▄▄▄    ▄▄▄▄     ▄   ▄▄▄▄   ▄▄▄         ▄▄     ▄▄▄",
    "██▄   ██▀   ▄█▀▀█▄     ██▄   ██▀   ▄█▀▀████▄  ▀██████▀  ▀██▀       ▄█▀▀█▄  █▀██  ██  ██▀▀",
    "███▄  ██    ██  ██     ███▄  ██    ██    ██     ██       ██        ██  ██    ██  ██  ██",
    "██ ▀█▄██    ██▀▀██     ██ ▀█▄██    ██    ██     ██       ██        ██▀▀██    ██  ██  ██",
    "██   ▀██  ▄ ██  ██     ██   ▀██    ██    ██     ██       ██      ▄ ██  ██    ██▄ ██▄ ██",
    "▀██▀    ██  ▀██▀  ▀█▄█ ▀██▀    ██     ▀████▀      ▀█████  ████████ ▀██▀  ▀█▄█  ▀████▀███▀",
];
const COMPACT_LOGO_LINES: [&str; 3] = [
    "███  ██ ▄████▄ ███  ██ ▄████▄ ▄█████ ██     ▄████▄ ██     ██",
    "██ ▀▄██ ██▄▄██ ██ ▀▄██ ██  ██ ██     ██     ██▄▄██ ██ ▄█▄ ██",
    "██   ██ ██  ██ ██   ██ ▀████▀ ▀█████ ██████ ██  ██  ▀██▀██▀",
];

pub(super) fn build_welcome_lines(
    state: &TuiState,
    viewport_width: u16,
    viewport_height: u16,
) -> Vec<Line<'static>> {
    let compact = viewport_height < 20 || viewport_width < 110;
    let mut core = build_welcome_logo_lines(compact);
    core.push(Line::raw(""));
    core.push(build_meta_summary_line(state));
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
        return COMPACT_LOGO_LINES
            .into_iter()
            .map(logo_line)
            .collect::<Vec<_>>();
    }

    // Preserve the operator-supplied brand mark as-is on wide terminals.
    FULL_LOGO_LINES
        .into_iter()
        .map(logo_line)
        .collect::<Vec<_>>()
}

fn build_meta_summary_line(state: &TuiState) -> Line<'static> {
    Line::from(vec![
        Span::styled("workspace", Style::default().fg(SUBTLE)),
        Span::styled(" ", Style::default().fg(SUBTLE)),
        Span::styled(
            state.session.workspace_name.clone(),
            Style::default().fg(MUTED),
        ),
        Span::styled("  ·  ", Style::default().fg(SUBTLE)),
        Span::styled("model", Style::default().fg(SUBTLE)),
        Span::styled(" ", Style::default().fg(SUBTLE)),
        Span::styled(model_label(state), Style::default().fg(ACCENT)),
    ])
}

fn model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) => format!("{} · {}", state.session.model, effort),
        None => state.session.model.clone(),
    }
}

fn logo_line(text: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.to_string(),
        Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
    )])
}
