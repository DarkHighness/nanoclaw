use super::theme::{ACCENT, HEADER, MUTED, SUBTLE};
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub(super) fn build_welcome_lines(state: &TuiState, viewport_height: u16) -> Vec<Line<'static>> {
    let compact = viewport_height < 16;
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

    [
        " _   _    _    _   _  ___   ____ _        _    _    _",
        "| \\ | |  / \\  | \\ | |/ _ \\ / ___| |      / \\  | |  | |",
        "|  \\| | / _ \\ |  \\| | | | | |   | |     / _ \\ | |  | |",
        "| |\\  |/ ___ \\| |\\  | |_| | |___| |___ / ___ \\| |__| |",
        "|_| \\_/_/   \\_\\_| \\_|\\___/ \\____|_____/_/   \\_\\\\____/ ",
    ]
    .into_iter()
    .map(|line| {
        Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))
    })
    .collect()
}
