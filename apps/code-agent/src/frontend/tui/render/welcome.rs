use super::theme::{ACCENT, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const PLATE_FACE: Color = Color::Rgb(30, 34, 39);
const PLATE_HIGHLIGHT: Color = Color::Rgb(37, 42, 48);
const PLATE_SHADOW: Color = Color::Rgb(11, 13, 16);
const WORDMARK_SHADOW: Color = Color::Rgb(86, 92, 98);
const PLATE_WIDTH: usize = 44;
const PLATE_SIDE_SHADOW_WIDTH: usize = 2;
const FULL_WORDMARK: &str = "N A N O C L A W";
const COMPACT_WORDMARK: &str = "NANOCLAW";

pub(super) fn build_welcome_lines(
    state: &TuiState,
    viewport_width: u16,
    viewport_height: u16,
) -> Vec<Line<'static>> {
    let compact = viewport_height < 22 || viewport_width < 76;
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
        return vec![
            plate_blank_line(PLATE_HIGHLIGHT),
            embossed_wordmark_line(COMPACT_WORDMARK, HEADER, PLATE_HIGHLIGHT, 0, true),
            embossed_wordmark_line(COMPACT_WORDMARK, WORDMARK_SHADOW, PLATE_FACE, 1, false),
            plate_shadow_line(),
        ];
    }

    // Keep the welcome brand restrained: a single spaced word on a raised
    // plate reads cleaner in a terminal than a sprawling ASCII banner, while
    // the bevel + offset shadow still gives the mark depth.
    let mut lines = Vec::with_capacity(5);
    lines.push(plate_blank_line(PLATE_HIGHLIGHT));
    lines.push(embossed_wordmark_line(
        FULL_WORDMARK,
        HEADER,
        PLATE_HIGHLIGHT,
        0,
        true,
    ));
    lines.push(embossed_wordmark_line(
        FULL_WORDMARK,
        WORDMARK_SHADOW,
        PLATE_FACE,
        1,
        false,
    ));
    lines.push(plate_blank_line(PLATE_FACE));
    lines.push(plate_shadow_line());
    lines
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

fn embossed_wordmark_line(
    text: &str,
    foreground: Color,
    background: Color,
    horizontal_offset: usize,
    bold: bool,
) -> Line<'static> {
    let content_width = text.len() + horizontal_offset;
    let side_padding = PLATE_WIDTH.saturating_sub(content_width) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(content_width + side_padding);
    let mut spans = plate_padding(side_padding + horizontal_offset, background);
    let mut style = Style::default().fg(foreground).bg(background);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    spans.push(Span::styled(text.to_string(), style));
    spans.extend(plate_padding(right_padding, background));
    spans.push(Span::styled(
        " ".repeat(PLATE_SIDE_SHADOW_WIDTH),
        Style::default().bg(PLATE_SHADOW),
    ));
    Line::from(spans)
}

fn plate_blank_line(color: Color) -> Line<'static> {
    let mut spans = plate_padding(PLATE_WIDTH, color);
    spans.push(Span::styled(
        " ".repeat(PLATE_SIDE_SHADOW_WIDTH),
        Style::default().bg(PLATE_SHADOW),
    ));
    Line::from(spans)
}

fn plate_shadow_line() -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(" ".repeat(PLATE_WIDTH), Style::default().bg(PLATE_SHADOW)),
    ])
}

fn plate_padding(width: usize, color: Color) -> Vec<Span<'static>> {
    vec![Span::styled(" ".repeat(width), Style::default().bg(color))]
}
