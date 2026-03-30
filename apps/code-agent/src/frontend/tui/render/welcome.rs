use super::theme::{ACCENT, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const PLATE_FACE: Color = Color::Rgb(30, 34, 39);
const PLATE_HIGHLIGHT: Color = Color::Rgb(37, 42, 48);
const PLATE_SHADOW: Color = Color::Rgb(11, 13, 16);
const WORDMARK_SHADOW: Color = Color::Rgb(86, 92, 98);
const PLATE_WIDTH: usize = 60;
const PLATE_SIDE_SHADOW_WIDTH: usize = 2;

const WORDMARK_ROWS: [&str; 5] = [
    " _   _    _    _   _  ___   ____ _        _ __        __ ",
    "| \\ | |  / \\  | \\ | |/ _ \\ / ___| |      / \\\\ \\      / / ",
    "|  \\| | / _ \\ |  \\| | | | | |   | |     / _ \\\\ \\ /\\ / /  ",
    "| |\\  |/ ___ \\| |\\  | |_| | |___| |___ / ___ \\\\ V  V /   ",
    "|_| \\_/_/   \\_\\_| \\_|\\___/ \\____|_____/_/   \\_\\\\_/\\_/    ",
];

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
            compact_logo_line(),
            compact_logo_shadow_line(),
            plate_blank_line(PLATE_FACE),
            plate_shadow_line(),
        ];
    }

    // Render the full word on a raised plate with a bevel row and an offset
    // shadow row so the logo reads like one embossed mark instead of split
    // halves with decorative echoes.
    let mut lines = Vec::with_capacity(WORDMARK_ROWS.len() * 2 + 3);
    lines.push(plate_blank_line(PLATE_HIGHLIGHT));
    for row in WORDMARK_ROWS {
        lines.push(wordmark_face_line(row));
        lines.push(wordmark_relief_line(row));
    }
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

fn compact_logo_line() -> Line<'static> {
    let content = "NANOCLAW";
    let side_padding = PLATE_WIDTH.saturating_sub(content.len()) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(content.len() + side_padding);
    let mut spans = plate_padding(side_padding, PLATE_HIGHLIGHT);
    spans.push(Span::styled(
        content.to_string(),
        Style::default()
            .fg(HEADER)
            .bg(PLATE_HIGHLIGHT)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend(plate_padding(right_padding, PLATE_HIGHLIGHT));
    spans.push(Span::styled(
        " ".repeat(PLATE_SIDE_SHADOW_WIDTH),
        Style::default().bg(PLATE_SHADOW),
    ));
    Line::from(spans)
}

fn compact_logo_shadow_line() -> Line<'static> {
    let content = "NANOCLAW";
    let side_padding = PLATE_WIDTH.saturating_sub(content.len() + 1) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(content.len() + side_padding + 1);
    let mut spans = plate_padding(side_padding + 1, PLATE_FACE);
    spans.push(Span::styled(
        content.to_string(),
        Style::default().fg(WORDMARK_SHADOW).bg(PLATE_FACE),
    ));
    spans.extend(plate_padding(right_padding, PLATE_FACE));
    spans.push(Span::styled(
        " ".repeat(PLATE_SIDE_SHADOW_WIDTH),
        Style::default().bg(PLATE_SHADOW),
    ));
    Line::from(spans)
}

fn wordmark_face_line(row: &'static str) -> Line<'static> {
    let side_padding = PLATE_WIDTH.saturating_sub(row.len()) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(row.len() + side_padding);
    let mut spans = plate_padding(side_padding, PLATE_HIGHLIGHT);
    spans.push(Span::styled(
        row.to_string(),
        Style::default()
            .fg(HEADER)
            .bg(PLATE_HIGHLIGHT)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend(plate_padding(right_padding, PLATE_HIGHLIGHT));
    spans.push(Span::styled(
        " ".repeat(PLATE_SIDE_SHADOW_WIDTH),
        Style::default().bg(PLATE_SHADOW),
    ));
    Line::from(spans)
}

fn wordmark_relief_line(row: &'static str) -> Line<'static> {
    let side_padding = PLATE_WIDTH.saturating_sub(row.len() + 1) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(row.len() + side_padding + 1);
    let mut spans = plate_padding(side_padding + 1, PLATE_FACE);
    spans.push(Span::styled(
        row.to_string(),
        Style::default().fg(WORDMARK_SHADOW).bg(PLATE_FACE),
    ));
    spans.extend(plate_padding(right_padding, PLATE_FACE));
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
