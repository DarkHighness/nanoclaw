use super::super::state::{TuiState, draft_preview_text};
use super::shared::clamp_scroll;
use super::theme::palette;
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
            .title_style(
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().footer_bg)),
        popup,
    );

    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(inner);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(sections[1]);

    frame.render_widget(
        Block::default()
            .title(" Candidates ")
            .title_style(
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().footer_bg)),
        body[0],
    );
    frame.render_widget(
        Block::default()
            .title(" Preview ")
            .title_style(
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().footer_bg)),
        body[1],
    );
    let list_area = body[0].inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let preview_area = body[1].inner(Margin {
        vertical: 1,
        horizontal: 1,
    });

    frame.render_widget(
        Paragraph::new(build_history_rollback_summary_text(state))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        sections[0],
    );

    let list = build_history_rollback_list_text(state);
    let selected_line = overlay.selected.saturating_mul(4);
    // Each list entry uses four lines (title, prompt, metadata, spacer), so anchoring
    // scroll by that stride keeps the highlighted turn centered predictably.
    let requested_scroll = selected_line.saturating_sub(usize::from(list_area.height / 2));
    let scroll = clamp_scroll(
        requested_scroll.min(u16::MAX as usize) as u16,
        list.lines.len(),
        list_area.height,
    );
    frame.render_widget(
        Paragraph::new(list)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        list_area,
    );

    frame.render_widget(
        Paragraph::new(build_history_rollback_preview_text(state))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        preview_area,
    );

    frame.render_widget(
        Paragraph::new(build_history_rollback_help_text())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().muted).bg(palette().footer_bg)),
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
    Text::from(vec![
        Line::from(vec![
            Span::styled("Selected", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(
                format!(
                    "Turn {} of {}",
                    overlay.selected + 1,
                    overlay.candidates.len()
                ),
                Style::default()
                    .fg(palette().user)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(
                format!(
                    "Remove {} turn(s) · {} message(s)",
                    candidate.removed_turn_count, candidate.removed_message_count
                ),
                Style::default().fg(palette().accent),
            ),
        ]),
        Line::from(vec![Span::styled(
            "Rollback restores the selected draft and rewinds the transcript to the start of that turn.",
            Style::default().fg(palette().muted),
        )]),
    ])
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
                if selected { "●" } else { "○" },
                Style::default().fg(if selected {
                    palette().accent
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                format!("Turn {}", index + 1),
                Style::default()
                    .fg(if selected {
                        palette().header
                    } else {
                        palette().text
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Prompt ", Style::default().fg(palette().muted)),
            Span::styled(
                draft_preview_text(&candidate.draft, &candidate.prompt, 34),
                Style::default().fg(if selected {
                    palette().text
                } else {
                    palette().muted
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Effect ", Style::default().fg(palette().muted)),
            Span::styled(
                format!(
                    "rewind {} turn(s) · remove {} message(s)",
                    candidate.removed_turn_count, candidate.removed_message_count
                ),
                Style::default().fg(palette().muted),
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
            Span::styled("Prompt ", Style::default().fg(palette().muted)),
            Span::styled(
                draft_preview_text(&candidate.draft, &candidate.prompt, 56),
                Style::default().fg(palette().text),
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
        Span::styled("esc", Style::default().fg(palette().accent)),
        Span::styled(" cancel", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("←/→", Style::default().fg(palette().accent)),
        Span::styled(" select turn", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("enter", Style::default().fg(palette().accent)),
        Span::styled(" rollback", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("q", Style::default().fg(palette().accent)),
        Span::styled(" close", Style::default().fg(palette().muted)),
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
