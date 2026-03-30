use super::theme::{ACCENT, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const WORDMARK_SHADOW: Color = Color::Rgb(92, 97, 102);
const FULL_WORDMARK: &str = "N A N O C L A W";
const COMPACT_WORDMARK: &str = "NANOCLAW";

pub(super) fn build_welcome_lines(
    state: &TuiState,
    viewport_width: u16,
    viewport_height: u16,
) -> Vec<Line<'static>> {
    let compact = viewport_height < 20 || viewport_width < 72;
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
    let wordmark = if compact {
        COMPACT_WORDMARK
    } else {
        FULL_WORDMARK
    };

    // Keep the brand mark typographic and quiet. In a terminal, spacing,
    // shadow, and one stabilizing rule read more deliberate than oversized
    // ASCII glyphs or framed cards.
    vec![
        wordmark_line(wordmark, 0, HEADER, true),
        wordmark_line(wordmark, 1, WORDMARK_SHADOW, false),
        underline_line(wordmark),
    ]
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

fn wordmark_line(text: &str, horizontal_offset: usize, color: Color, bold: bool) -> Line<'static> {
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::raw(" ".repeat(horizontal_offset)),
        Span::styled(text.to_string(), style),
    ])
}

fn underline_line(wordmark: &str) -> Line<'static> {
    let width = wordmark.chars().count().saturating_sub(4);
    Line::from(vec![
        Span::raw("  "),
        Span::styled("─".repeat(width), Style::default().fg(ACCENT)),
    ])
}
