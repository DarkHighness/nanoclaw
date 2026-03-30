use super::super::state::{TuiState, preview_text};
use super::shared::clamp_scroll;
use super::theme::{ACCENT, BORDER_ACTIVE, FOOTER_BG, HEADER, MUTED, SUBTLE, TEXT, USER};
use super::transcript::format_transcript_cell;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn render_history_rollback_overlay(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
) {
    let Some(overlay) = state.history_rollback_overlay() else {
        return;
    };
    let popup = centered_rect(area, 82, 78);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(" History Rollback ")
            .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_ACTIVE))
            .style(Style::default().bg(FOOTER_BG)),
        popup,
    );

    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(sections[1]);

    frame.render_widget(
        Paragraph::new(build_history_rollback_summary_text(state))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        sections[0],
    );

    let list = build_history_rollback_list_text(state);
    let selected_line = overlay.selected.saturating_mul(3);
    // Each list entry uses three lines (title, metadata, spacer), so anchoring
    // scroll by that stride keeps the highlighted turn centered predictably.
    let requested_scroll = selected_line.saturating_sub(usize::from(body[0].height / 2));
    let scroll = clamp_scroll(
        requested_scroll.min(u16::MAX as usize) as u16,
        list.lines.len(),
        body[0].height,
    );
    frame.render_widget(
        Paragraph::new(list)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        body[0],
    );

    frame.render_widget(
        Paragraph::new(build_history_rollback_preview_text(state))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        body[1],
    );

    frame.render_widget(
        Paragraph::new(build_history_rollback_help_text())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED).bg(FOOTER_BG)),
        sections[2],
    );
}

pub(super) fn build_history_rollback_summary_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.history_rollback_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    let Some(candidate) = state.selected_history_rollback_candidate() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    Text::from(vec![Line::from(vec![
        Span::styled("selected", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            format!(
                "turn {} of {}",
                overlay.selected + 1,
                overlay.candidates.len()
            ),
            Style::default().fg(USER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            format!(
                "drops {} turn(s) / {} message(s)",
                candidate.removed_turn_count, candidate.removed_message_count
            ),
            Style::default().fg(ACCENT),
        ),
    ])])
}

pub(super) fn build_history_rollback_list_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.history_rollback_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    let mut lines = Vec::new();
    for (index, candidate) in overlay.candidates.iter().enumerate() {
        let selected = overlay.selected == index;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "›" } else { " " },
                Style::default().fg(if selected { USER } else { SUBTLE }),
            ),
            Span::raw(" "),
            Span::styled(
                preview_text(&candidate.prompt, 40),
                Style::default()
                    .fg(if selected { HEADER } else { TEXT })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "rewind from turn {} · remove {} turn(s)",
                    index + 1,
                    candidate.removed_turn_count
                ),
                Style::default().fg(MUTED),
            ),
        ]));
        if index + 1 < overlay.candidates.len() {
            lines.push(Line::raw(""));
        }
    }
    Text::from(lines)
}

pub(super) fn build_history_rollback_preview_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.history_rollback_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    let Some(candidate) = overlay.candidates.get(overlay.selected) else {
        return Text::from(Vec::<Line<'static>>::new());
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Turn Preview", Style::default().fg(HEADER)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                preview_text(&candidate.prompt, 56),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::raw(""),
    ];
    for (index, entry) in candidate.turn_preview_lines.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        lines.extend(format_transcript_cell(entry));
    }
    Text::from(lines)
}

fn build_history_rollback_help_text() -> Text<'static> {
    Text::from(vec![Line::from(vec![
        Span::styled("esc", Style::default().fg(ACCENT)),
        Span::styled("/← older", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("→ newer", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("enter rollback", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("q cancel", Style::default().fg(MUTED)),
    ])])
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(height_percent)) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100_u16.saturating_sub(height_percent)) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(width_percent)) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100_u16.saturating_sub(width_percent)) / 2),
        ])
        .split(vertical[1])[1]
}
