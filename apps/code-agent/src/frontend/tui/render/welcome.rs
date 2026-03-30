use super::theme::{ACCENT, HEADER, MUTED, SUBTLE};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub(super) fn build_welcome_lines(state: &TuiState, viewport_height: u16) -> Vec<Line<'static>> {
    let compact = viewport_height < 18;
    let mut core = build_welcome_logo_lines(compact);
    core.push(Line::raw(""));
    core.push(Line::from(vec![
        Span::styled(
            state.session.workspace_name.clone(),
            Style::default().fg(MUTED),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(state.session.model.clone(), Style::default().fg(ACCENT)),
    ]));
    core.push(Line::from(Span::styled(
        "Type a prompt or /help.",
        Style::default().fg(SUBTLE),
    )));

    let top_padding = usize::from(viewport_height.saturating_sub(core.len() as u16) / 2);
    let mut lines = vec![Line::raw(""); top_padding];
    lines.extend(core);
    lines
}

fn build_welcome_logo_lines(compact: bool) -> Vec<Line<'static>> {
    if compact {
        return vec![Line::from(Span::styled(
            "NANOCLAW".to_string(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))];
    }

    vec![
        wordmark_line(
            "NN   NN   AAA   NN   NN   OOO   ",
            " CCCC  L       AAA   WW     WW",
        ),
        wordmark_line(
            "NNN  NN  AAAAA  NNN  NN  OO OO ",
            "CC     L      AAAAA  WW     WW",
        ),
        wordmark_line(
            "NN N NN  AA AA  NN N NN OO   OO",
            "CC     L      AA AA  WW  W  WW",
        ),
        wordmark_line(
            "NN  NNN  AAAAA  NN  NNN OO   OO",
            "CC     L      AAAAA  WW WWW WW",
        ),
        wordmark_line(
            "NN   NN  AA AA  NN   NN  OO OO ",
            "CC     L      AA AA   WWW WWW ",
        ),
        wordmark_line(
            "NN   NN  AA AA  NN   NN   OOO  ",
            " CCCC  LLLLL  AA AA    WW WW  ",
        ),
    ]
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
