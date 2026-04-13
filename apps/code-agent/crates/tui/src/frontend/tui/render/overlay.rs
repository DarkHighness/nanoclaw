use super::theme::palette;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear};

pub(super) fn centered_overlay_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
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

pub(super) fn render_overlay_container(
    frame: &mut ratatui::Frame<'_>,
    popup: Rect,
    title: &str,
    title_color: Color,
    border_color: Color,
) -> Rect {
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(format!(" {title} "))
            .title_style(
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(palette().overlay_surface())),
        popup,
    );
    popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    })
}

pub(super) fn overlay_panel_block(title: &str, title_color: Color) -> Block<'static> {
    Block::default()
        .title(format!(" {title} "))
        .title_style(
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette().chrome_border()))
        .style(Style::default().bg(palette().elevated_surface()))
}

pub(super) fn overlay_container_style() -> Style {
    Style::default()
        .fg(palette().text)
        .bg(palette().overlay_surface())
}

pub(super) fn overlay_help_style() -> Style {
    Style::default()
        .fg(palette().muted)
        .bg(palette().overlay_surface())
}

pub(super) fn overlay_panel_style() -> Style {
    Style::default()
        .fg(palette().text)
        .bg(palette().elevated_surface())
}
