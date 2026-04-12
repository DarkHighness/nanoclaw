use super::super::state::TuiState;
use super::shared::clamp_scroll;
use super::theme::palette;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn render_tool_review_overlay(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
) {
    let Some(overlay) = state.tool_review_overlay() else {
        return;
    };
    let popup = centered_rect(area, 86, 80);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(" Tool Review ")
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
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(sections[1]);

    frame.render_widget(
        Paragraph::new(build_tool_review_summary_text(state))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        sections[0],
    );

    let list = build_tool_review_list_text(state);
    let selected_line = overlay.selected.saturating_mul(3);
    let requested_scroll = selected_line.saturating_sub(usize::from(body[0].height / 2));
    let scroll = clamp_scroll(
        requested_scroll.min(u16::MAX as usize) as u16,
        list.lines.len(),
        body[0].height.saturating_sub(2),
    );
    frame.render_widget(
        Block::default()
            .title(" Files ")
            .title_style(
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().bottom_pane_bg)),
        body[0],
    );
    frame.render_widget(
        Paragraph::new(list)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .style(
                Style::default()
                    .fg(palette().text)
                    .bg(palette().bottom_pane_bg),
            ),
        body[0].inner(Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );

    frame.render_widget(
        Block::default()
            .title(" Diff Preview ")
            .title_style(
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().bottom_pane_bg)),
        body[1],
    );
    frame.render_widget(
        Paragraph::new(build_tool_review_preview_text(state))
            .wrap(Wrap { trim: false })
            .style(
                Style::default()
                    .fg(palette().text)
                    .bg(palette().bottom_pane_bg),
            ),
        body[1].inner(Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );

    frame.render_widget(
        Paragraph::new(build_tool_review_help_text())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().muted).bg(palette().footer_bg)),
        sections[2],
    );
}

pub(super) fn build_tool_review_summary_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.tool_review_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    let selected = state.selected_tool_review_file();
    let mut lines = vec![Line::from(vec![
        Span::styled("tool", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            overlay.tool_name.clone(),
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!(
                "file {} of {}",
                overlay.selected + 1,
                overlay.review.files.len()
            ),
            Style::default().fg(palette().accent),
        ),
    ])];

    if let Some(summary) = overlay
        .review
        .summary
        .as_deref()
        .filter(|summary| !summary.trim().is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("effect", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(summary.to_string(), Style::default().fg(palette().text)),
        ]));
    } else if let Some(selected) = selected {
        lines.push(Line::from(vec![
            Span::styled("file", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(selected.path.clone(), Style::default().fg(palette().text)),
        ]));
    }

    Text::from(lines)
}

pub(super) fn build_tool_review_list_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.tool_review_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };

    let mut lines = Vec::new();
    for (index, file) in overlay.review.files.iter().enumerate() {
        let selected = overlay.selected == index;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "›" } else { " " },
                Style::default().fg(if selected {
                    palette().accent
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                file.path.clone(),
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
            Span::raw("  "),
            Span::styled(
                format!("{} preview line(s)", file.preview_lines.len()),
                Style::default().fg(palette().muted),
            ),
        ]));
        if index + 1 < overlay.review.files.len() {
            lines.push(Line::raw(""));
        }
    }

    Text::from(lines)
}

pub(super) fn build_tool_review_preview_text(state: &TuiState) -> Text<'static> {
    let Some(file) = state.selected_tool_review_file() else {
        return Text::from(Vec::<Line<'static>>::new());
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Diff Preview", Style::default().fg(palette().header)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(file.path.clone(), Style::default().fg(palette().text)),
        ]),
        Line::raw(""),
    ];

    lines.extend(file.preview_lines.iter().map(|line| {
        let style = if line.starts_with('+') {
            Style::default().fg(palette().assistant)
        } else if line.starts_with('-') {
            Style::default().fg(palette().error)
        } else {
            Style::default().fg(palette().text)
        };
        Line::from(Span::styled(line.clone(), style))
    }));

    Text::from(lines)
}

fn build_tool_review_help_text() -> Text<'static> {
    Text::from(vec![Line::from(vec![
        Span::styled("↑↓", Style::default().fg(palette().accent)),
        Span::styled(" move", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("home/end", Style::default().fg(palette().accent)),
        Span::styled(" jump", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("esc", Style::default().fg(palette().header)),
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
