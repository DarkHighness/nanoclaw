mod chrome;
mod picker;
mod shared;
mod shell;
mod statusline;
mod theme;
mod transcript;
mod view;
mod welcome;

use super::approval::ApprovalPrompt;
use super::commands::slash_command_hint;
use super::state::TuiState;
use chrome::{
    approval_band_height, approval_preview_lines, build_approval_text, render_approval_band,
    render_composer, should_render_side_rail, side_rail_width,
};
use picker::{build_command_hint_text, command_hint_height, render_command_hint_band};
use ratatui::layout::{Constraint, Direction, Layout, Position};
use ratatui::style::Style;
use ratatui::widgets::Block;
use shared::composer_cursor_width;
use shell::{bottom_layout_constraints, composer_inner_area, render_main_pane, render_side_rail};
use statusline::render_status_line;
use theme::*;

pub(crate) fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    approval: Option<&ApprovalPrompt>,
) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let approval_height = approval.map(approval_band_height);
    let command_hint = approval
        .is_none()
        .then(|| slash_command_hint(&state.input, state.command_completion_index))
        .flatten();
    let command_hint_height = command_hint.as_ref().map(command_hint_height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            approval_height,
            command_hint_height,
        ))
        .split(area);
    let mut next_index = 0;
    let main_area = vertical[next_index];
    next_index += 1;
    let approval_area = approval_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let command_hint_area = command_hint_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let composer_area = vertical[next_index];
    let status_area = vertical[next_index + 1];

    if should_render_side_rail(state, main_area) {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(10),
                Constraint::Length(side_rail_width(main_area.width)),
            ])
            .split(main_area);
        render_main_pane(frame, horizontal[0], state);
        render_side_rail(frame, horizontal[1], state);
    } else {
        render_main_pane(frame, main_area, state);
    }
    if let Some(approval) = approval {
        render_approval_band(frame, approval_area.expect("approval area"), approval);
    }
    if let Some(command_hint) = command_hint.as_ref() {
        render_command_hint_band(
            frame,
            command_hint_area.expect("command hint area"),
            command_hint,
        );
    }
    render_composer(frame, composer_area, state);
    render_status_line(frame, status_area, state);

    let composer_inner = composer_inner_area(composer_area);
    let prefix_width = 2_u16;
    frame.set_cursor_position(Position::new(
        composer_inner
            .x
            .saturating_add(prefix_width)
            .saturating_add(composer_cursor_width(&state.input)),
        composer_inner.y,
    ));
}

#[cfg(test)]
mod tests;
