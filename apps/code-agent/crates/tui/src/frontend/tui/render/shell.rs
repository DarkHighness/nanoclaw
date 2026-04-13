use super::super::state::{MainPaneMode, TuiState, preview_text};
use super::shared::clamp_scroll;
use super::theme::palette;
use super::transcript::{
    active_turn_title_for_viewport, render_transcript, transcript_content_area,
};
use super::view::{
    build_inspector_text, build_statusline_picker_text, build_theme_picker_text,
    build_thinking_effort_picker_text, collection_picker_footer,
};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn render_top_title(
    frame: &mut ratatui::Frame<'_>,
    title_area: Rect,
    main_area: Rect,
    state: &TuiState,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().main_bg)),
        title_area,
    );
    let inner = title_area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_top_title_line(state, main_area))
            .style(Style::default().fg(palette().muted).bg(palette().main_bg)),
        inner,
    );
}

pub(super) fn render_main_pane(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    match state.main_pane {
        MainPaneMode::Transcript => render_transcript(frame, area, state),
        MainPaneMode::View => render_main_view(frame, area, state),
    }
}

pub(super) fn bottom_band_inner_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    })
}

pub(super) fn bottom_layout_constraints(
    approval_height: Option<u16>,
    pending_height: Option<u16>,
    toast_height: Option<u16>,
    composer_height: u16,
) -> Vec<Constraint> {
    let mut constraints = vec![Constraint::Min(10)];
    if let Some(height) = approval_height {
        constraints.push(Constraint::Length(height));
    }
    if let Some(height) = pending_height {
        constraints.push(Constraint::Length(height));
    }
    if let Some(height) = toast_height {
        constraints.push(Constraint::Length(height));
    }
    constraints.push(Constraint::Length(composer_height.max(1)));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    constraints
}

pub(super) fn build_top_title_line(state: &TuiState, main_area: Rect) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "NANOCLAW",
        Style::default()
            .fg(palette().header)
            .add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::styled(" / ", Style::default().fg(palette().subtle)));

    if state.transcript.is_empty() {
        spans.push(Span::styled(
            "welcome",
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "waiting for first prompt",
            Style::default().fg(palette().text),
        ));
        return Line::from(spans);
    }

    let turn_label = if !state.follow_transcript {
        "history turn"
    } else if state.turn_running {
        "live turn"
    } else {
        "current turn"
    };
    let turn_tone = if !state.follow_transcript {
        palette().user
    } else {
        palette().accent
    };
    let transcript_area = transcript_content_area(main_area);
    let prompt =
        active_turn_title_for_viewport(state, transcript_area.width, transcript_area.height)
            .unwrap_or_else(|| "No prompt captured yet".to_string());
    let prompt_budget = top_title_prompt_budget(main_area.width, turn_label);

    spans.push(Span::styled(
        turn_label,
        Style::default().fg(turn_tone).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled(
        "started from",
        Style::default().fg(palette().muted),
    ));
    spans.push(Span::styled(" ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled(
        preview_text(&prompt, prompt_budget),
        Style::default().fg(palette().text),
    ));

    Line::from(spans)
}

fn render_main_view(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().main_bg)),
        area,
    );
    let title = if state.inspector_title.is_empty() {
        "View"
    } else {
        state.inspector_title.as_str()
    };

    if let Some(picker) = state.statusline_picker.as_ref() {
        render_standard_main_view(
            frame,
            area,
            title,
            build_statusline_picker_text(&state.session.statusline, picker),
            state.inspector_scroll,
            None,
        );
        return;
    }

    if let Some(picker) = state.thinking_effort_picker.as_ref() {
        render_standard_main_view(
            frame,
            area,
            title,
            build_thinking_effort_picker_text(
                state.session.model_reasoning_effort.as_deref(),
                &state.session.supported_model_reasoning_efforts,
                picker,
            ),
            state.inspector_scroll,
            None,
        );
        return;
    }

    if let Some(picker) = state.theme_picker.as_ref() {
        render_standard_main_view(
            frame,
            area,
            title,
            build_theme_picker_text(&state.theme, &state.themes, picker),
            state.inspector_scroll,
            None,
        );
        return;
    }

    if title.starts_with("Command Palette") {
        let footer = collection_picker_footer(title, state.selected_collection_entry().as_ref());
        render_command_palette_modal(
            frame,
            area,
            title,
            build_inspector_text(
                title,
                &state.inspector,
                state
                    .collection_picker
                    .as_ref()
                    .map(|picker| picker.selected),
            ),
            state.inspector_scroll,
            footer,
        );
        return;
    }

    render_standard_main_view(
        frame,
        area,
        title,
        build_inspector_text(
            title,
            &state.inspector,
            state
                .collection_picker
                .as_ref()
                .map(|picker| picker.selected),
        ),
        state.inspector_scroll,
        collection_picker_footer(title, state.selected_collection_entry().as_ref()),
    );
}

fn top_title_prompt_budget(width: u16, turn_label: &str) -> usize {
    usize::from(width.saturating_sub((turn_label.len() + 24) as u16)).max(24)
}

fn render_standard_main_view(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    text: Text<'static>,
    scroll_state: u16,
    footer: Option<Line<'static>>,
) {
    frame.render_widget(
        Block::default()
            .title(format!(" {title} "))
            .title_style(
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().main_bg)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let sections = if footer.is_some() {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(1)])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6)])
            .split(inner)
    };
    let scroll = clamp_scroll(scroll_state, text.lines.len().max(1), sections[0].height);
    frame.render_widget(
        Paragraph::new(text)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().main_bg)),
        sections[0],
    );
    if let Some(footer) = footer
        && sections.len() > 1
    {
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().bg(palette().main_bg)),
            sections[1],
        );
    }
}

fn render_command_palette_modal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    text: Text<'static>,
    scroll_state: u16,
    footer: Option<Line<'static>>,
) {
    let popup = centered_rect(area, 76, 74);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(format!(" {title} "))
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
        .constraints([Constraint::Min(6), Constraint::Length(1)])
        .split(inner);
    let scroll = clamp_scroll(scroll_state, text.lines.len().max(1), sections[0].height);
    frame.render_widget(
        Paragraph::new(text)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        sections[0],
    );
    frame.render_widget(
        Paragraph::new(footer.unwrap_or_else(|| {
            Line::from(vec![
                Span::styled("esc", Style::default().fg(palette().accent)),
                Span::styled(" close", Style::default().fg(palette().muted)),
                Span::styled(" · ", Style::default().fg(palette().subtle)),
                Span::styled("↑↓", Style::default().fg(palette().accent)),
                Span::styled(" move", Style::default().fg(palette().muted)),
            ])
        }))
        .style(Style::default().bg(palette().footer_bg)),
        sections[1],
    );
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
