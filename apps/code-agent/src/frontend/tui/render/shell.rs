use super::super::state::{MainPaneMode, TuiState};
use super::chrome::build_side_rail_lines;
use super::theme::{MAIN_BG, MUTED, TEXT};
use super::transcript::render_transcript;
use super::view::{build_inspector_text, build_statusline_picker_text, should_render_view_title};
use ratatui::layout::{Constraint, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

pub(super) fn render_main_pane(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    match state.main_pane {
        MainPaneMode::Transcript => render_transcript(frame, area, state),
        MainPaneMode::View => render_main_view(frame, area, state),
    }
}

pub(super) fn render_side_rail(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let rail = Paragraph::new(Text::from(build_side_rail_lines(state)))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(rail, inner);
}

pub(super) fn composer_inner_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    })
}

pub(super) fn bottom_layout_constraints(
    approval_height: Option<u16>,
    command_hint_height: Option<u16>,
) -> Vec<Constraint> {
    let mut constraints = vec![Constraint::Min(10)];
    if let Some(height) = approval_height {
        constraints.push(Constraint::Length(height));
    }
    if let Some(height) = command_hint_height {
        constraints.push(Constraint::Length(height));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    constraints
}

fn render_main_view(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    let title = if state.inspector_title.is_empty() {
        "View"
    } else {
        state.inspector_title.as_str()
    };
    let text = if let Some(picker) = state.statusline_picker.as_ref() {
        build_statusline_picker_text(&state.session.statusline, picker)
    } else {
        let mut lines = Vec::new();
        if should_render_view_title(title, &state.inspector) {
            lines.push(Line::from(Span::styled(
                title.to_string(),
                Style::default().fg(MUTED),
            )));
            lines.push(Line::raw(""));
        }
        lines.extend(build_inspector_text(title, &state.inspector).lines);
        Text::from(lines)
    };
    let scroll = super::clamp_scroll(
        state.inspector_scroll,
        text.lines.len().max(1),
        inner.height,
    );
    let view = Paragraph::new(text)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(view, inner);
}
