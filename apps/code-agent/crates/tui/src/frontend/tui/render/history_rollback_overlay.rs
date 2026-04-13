use super::super::state::{TuiState, draft_preview_text};
use super::overlay::{
    centered_overlay_rect, overlay_container_style, overlay_help_style, overlay_panel_block,
    overlay_panel_style, render_overlay_container,
};
use super::shared::clamp_scroll;
use super::theme::palette;
use super::transcript::format_transcript_cell;
use agent::types::CheckpointRestoreMode;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

pub(super) fn render_history_rollback_overlay(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
) {
    let Some(overlay) = state.history_rollback_overlay() else {
        return;
    };
    let popup = centered_overlay_rect(area, 84, 80);
    let inner = render_overlay_container(
        frame,
        popup,
        "History Rollback",
        palette().header,
        palette().emphasis_border(),
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(sections[1]);

    frame.render_widget(overlay_panel_block("Candidates", palette().accent), body[0]);
    frame.render_widget(overlay_panel_block("Preview", palette().header), body[1]);
    let list_area = body[0].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let preview_area = body[1].inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    frame.render_widget(
        Paragraph::new(build_history_rollback_summary_text(state))
            .wrap(Wrap { trim: false })
            .style(overlay_container_style()),
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
            .style(overlay_panel_style()),
        list_area,
    );

    frame.render_widget(
        Paragraph::new(build_history_rollback_preview_text(state))
            .wrap(Wrap { trim: false })
            .style(overlay_panel_style()),
        preview_area,
    );

    frame.render_widget(
        Paragraph::new(build_history_rollback_help_text())
            .wrap(Wrap { trim: false })
            .style(overlay_help_style()),
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
            match (
                overlay.restore_mode,
                candidate.checkpoint.as_ref(),
            ) {
                (CheckpointRestoreMode::Both, Some(checkpoint)) => {
                    format!(
                        "Restore mode: rewind visible conversation and restore code from {} ({} file change(s)).",
                        checkpoint.checkpoint_id, checkpoint.changed_file_count
                    )
                }
                (_, Some(checkpoint)) => {
                    format!(
                        "Transcript rollback restores the selected draft and rewinds visible conversation only. Press Tab to include code restore from {}.",
                        checkpoint.checkpoint_id
                    )
                }
                _ => "Transcript rollback restores the selected draft and rewinds visible conversation only. No durable checkpoint was recorded for this turn, so workspace files stay unchanged.".to_string(),
            },
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
                match candidate.checkpoint.as_ref() {
                    Some(checkpoint) => format!(
                        "rewind transcript {} turn(s) · remove {} message(s) · checkpoint {} ({} file(s))",
                        candidate.removed_turn_count,
                        candidate.removed_message_count,
                        checkpoint.checkpoint_id,
                        checkpoint.changed_file_count
                    ),
                    None => format!(
                        "rewind transcript {} turn(s) · remove {} message(s)",
                        candidate.removed_turn_count, candidate.removed_message_count
                    ),
                },
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
        Span::styled("tab", Style::default().fg(palette().accent)),
        Span::styled(" toggle code restore", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("enter", Style::default().fg(palette().accent)),
        Span::styled(
            " apply selected restore",
            Style::default().fg(palette().muted),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("q", Style::default().fg(palette().accent)),
        Span::styled(" close", Style::default().fg(palette().muted)),
    ])])
}
