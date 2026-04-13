use super::super::state::TuiState;
use super::overlay::{
    centered_overlay_rect, overlay_container_style, overlay_help_style, overlay_panel_block,
    overlay_panel_style, render_overlay_container,
};
use super::shared::clamp_scroll;
use super::theme::palette;
use super::transcript_shell::render_tool_review_preview_lines;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

pub(super) fn render_tool_review_overlay(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
) {
    let Some(overlay) = state.tool_review_overlay() else {
        return;
    };
    let popup = centered_overlay_rect(area, 86, 80);
    let inner = render_overlay_container(
        frame,
        popup,
        "Tool Review",
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
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(sections[1]);

    frame.render_widget(
        Paragraph::new(build_tool_review_summary_text(state))
            .wrap(Wrap { trim: false })
            .style(overlay_container_style()),
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
    let list_title = match overlay.review.kind {
        crate::tool_render::ToolReviewKind::FileDiff => " Files ",
        crate::tool_render::ToolReviewKind::Structured => " Sections ",
    };
    frame.render_widget(
        overlay_panel_block(list_title.trim(), palette().accent),
        body[0],
    );
    frame.render_widget(
        Paragraph::new(list)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .style(overlay_panel_style()),
        body[0].inner(Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );

    let preview_title = match overlay.review.kind {
        crate::tool_render::ToolReviewKind::FileDiff => " Diff Preview ",
        crate::tool_render::ToolReviewKind::Structured => " Section Preview ",
    };
    frame.render_widget(
        overlay_panel_block(preview_title.trim(), palette().header),
        body[1],
    );
    frame.render_widget(
        Paragraph::new(build_tool_review_preview_text(state))
            .wrap(Wrap { trim: false })
            .style(overlay_panel_style()),
        body[1].inner(Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );

    frame.render_widget(
        Paragraph::new(build_tool_review_help_text())
            .wrap(Wrap { trim: false })
            .style(overlay_help_style()),
        sections[2],
    );
}

pub(super) fn build_tool_review_summary_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.tool_review_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    let selected = state.selected_tool_review_item();
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
                "{} {} of {}",
                overlay.review.kind.singular_label(),
                overlay.selected + 1,
                overlay.review.items.len()
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
            Span::styled(
                overlay.review.kind.singular_label(),
                Style::default().fg(palette().muted),
            ),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(selected.title.clone(), Style::default().fg(palette().text)),
        ]));
    }

    Text::from(lines)
}

pub(super) fn build_tool_review_list_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.tool_review_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };

    let mut lines = Vec::new();
    for (index, item) in overlay.review.items.iter().enumerate() {
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
                item.title.clone(),
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
                tool_review_item_summary(&overlay.review.kind, item),
                Style::default().fg(palette().muted),
            ),
        ]));
        if index + 1 < overlay.review.items.len() {
            lines.push(Line::raw(""));
        }
    }

    Text::from(lines)
}

pub(super) fn build_tool_review_preview_text(state: &TuiState) -> Text<'static> {
    let Some(overlay) = state.tool_review_overlay() else {
        return Text::from(Vec::<Line<'static>>::new());
    };
    let Some(item) = state.selected_tool_review_item() else {
        return Text::from(Vec::<Line<'static>>::new());
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                match overlay.review.kind {
                    crate::tool_render::ToolReviewKind::FileDiff => "Diff Preview",
                    crate::tool_render::ToolReviewKind::Structured => "Section Preview",
                },
                Style::default().fg(palette().header),
            ),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(item.title.clone(), Style::default().fg(palette().text)),
        ]),
        Line::raw(""),
    ];

    lines.extend(render_tool_review_preview_lines(item));

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

fn tool_review_item_summary(
    review_kind: &crate::tool_render::ToolReviewKind,
    item: &crate::tool_render::ToolReviewItem,
) -> String {
    match review_kind {
        crate::tool_render::ToolReviewKind::FileDiff => {
            format!("{} preview line(s)", item.preview_lines.len())
        }
        crate::tool_render::ToolReviewKind::Structured => {
            let first_line = item
                .preview_lines
                .iter()
                .find(|line| !line.trim().is_empty())
                .map(|line| truncate_inline(line, 72))
                .unwrap_or_else(|| "empty preview".to_string());
            if item.preview_lines.len() > 1 {
                format!("{first_line} · +{} more", item.preview_lines.len() - 1)
            } else {
                first_line
            }
        }
    }
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}
