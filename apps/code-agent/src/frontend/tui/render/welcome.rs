use super::theme::{ACCENT, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const PLATE_FACE: Color = Color::Rgb(30, 34, 39);
const PLATE_HIGHLIGHT: Color = Color::Rgb(37, 42, 48);
const PLATE_SHADOW: Color = Color::Rgb(11, 13, 16);
const PLATE_WIDTH: usize = 60;
const PLATE_SIDE_SHADOW_WIDTH: usize = 2;

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
            plate_blank_line(PLATE_FACE),
            plate_shadow_line(),
        ];
    }

    // The welcome mark renders on a raised plate with a right/bottom shadow so
    // the brand reads like a single terminal wordmark instead of loose glyphs.
    let rows = [
        ("N    N   AAA   N    N   OOO ", " CCCC  L      AAA   W   W"),
        ("NN   N  A   A  NN   N  O   O", " C     L     A   A  W   W"),
        ("N N  N  AAAAA  N N  N  O   O", " C     L     AAAAA  W W W"),
        ("N  N N  A   A  N  N N  O   O", " C     L     A   A  WW WW"),
        ("N   NN  A   A  N   NN   OOO ", " CCCC  LLLLL A   A  W   W"),
    ];

    let mut lines = Vec::with_capacity(rows.len() + 3);
    lines.push(plate_blank_line(PLATE_HIGHLIGHT));
    for (left, right) in rows {
        lines.push(wordmark_plate_line(left, right));
    }
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

fn compact_logo_line() -> Line<'static> {
    let content = "NANOCLAW";
    let side_padding = PLATE_WIDTH.saturating_sub(content.len()) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(content.len() + side_padding);
    let mut spans = plate_padding(side_padding, PLATE_FACE);
    spans.push(Span::styled(
        content.to_string(),
        Style::default()
            .fg(HEADER)
            .bg(PLATE_FACE)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend(plate_padding(right_padding, PLATE_FACE));
    spans.push(Span::styled(
        " ".repeat(PLATE_SIDE_SHADOW_WIDTH),
        Style::default().bg(PLATE_SHADOW),
    ));
    Line::from(spans)
}

fn wordmark_plate_line(left: &'static str, right: &'static str) -> Line<'static> {
    let separator = "  ";
    let content_width = left.len() + separator.len() + right.len();
    let side_padding = PLATE_WIDTH.saturating_sub(content_width) / 2;
    let right_padding = PLATE_WIDTH.saturating_sub(content_width + side_padding);
    let mut spans = plate_padding(side_padding, PLATE_FACE);
    spans.push(Span::styled(
        left.to_string(),
        Style::default()
            .fg(HEADER)
            .bg(PLATE_FACE)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        separator.to_string(),
        Style::default().bg(PLATE_FACE),
    ));
    spans.push(Span::styled(
        right.to_string(),
        Style::default()
            .fg(ACCENT)
            .bg(PLATE_FACE)
            .add_modifier(Modifier::BOLD),
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
