use super::theme::palette;
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub(super) fn build_welcome_lines(
    state: &TuiState,
    viewport_width: u16,
    viewport_height: u16,
) -> Vec<Line<'static>> {
    let compact = viewport_height < 20 || viewport_width < 96;
    let mut core = build_welcome_title_lines(compact);
    core.push(Line::raw(""));
    core.push(build_meta_summary_line(state));
    core.push(Line::raw(""));
    core.push(build_prompt_line(compact));
    core.push(build_shortcut_line(compact));

    let top_padding = usize::from(viewport_height.saturating_sub(core.len() as u16) / 2);
    let mut lines = vec![Line::raw(""); top_padding];
    lines.extend(core);
    lines
}

fn build_welcome_title_lines(compact: bool) -> Vec<Line<'static>> {
    let subtitle = if compact {
        "Terminal shell for coding work"
    } else {
        "Terminal shell for focused coding work"
    };
    vec![
        Line::from(vec![Span::styled(
            "Code Agent",
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            subtitle,
            Style::default().fg(palette().muted),
        )]),
    ]
}

fn build_prompt_line(compact: bool) -> Line<'static> {
    let detail = if compact {
        "Ask for a change or run /help."
    } else {
        "Ask for a change, inspect the workspace, or run /help."
    };
    Line::from(vec![Span::styled(
        detail,
        Style::default().fg(palette().text),
    )])
}

fn build_shortcut_line(compact: bool) -> Line<'static> {
    let suffix = if compact {
        "Enter run · Tab queue · ^T effort"
    } else {
        "Enter run · Tab queue · ^T effort · ^O editor"
    };
    Line::from(vec![Span::styled(
        suffix,
        Style::default().fg(palette().accent),
    )])
}

fn build_meta_summary_line(state: &TuiState) -> Line<'static> {
    Line::from(vec![
        Span::styled("workspace", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            state.session.workspace_name.clone(),
            Style::default().fg(palette().muted),
        ),
        Span::styled("  ·  ", Style::default().fg(palette().subtle)),
        Span::styled("model", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(model_label(state), Style::default().fg(palette().accent)),
    ])
}

fn model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) => format!("{} · {}", state.session.model, effort),
        None => state.session.model.clone(),
    }
}
