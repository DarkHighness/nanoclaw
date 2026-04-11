mod chrome;
mod history_rollback_overlay;
mod picker;
mod shared;
mod shell;
mod statusline;
mod theme;
mod transcript;
mod transcript_markdown;
mod transcript_markdown_blocks;
mod transcript_markdown_line;
mod transcript_shell;
mod view;
mod welcome;

use super::UserInputView;
use super::approval::ApprovalPrompt;
use super::commands::slash_command_hint;
use super::state::TuiState;
use crate::interaction::PermissionRequestPrompt;
use chrome::{
    approval_band_height, composer_cursor_position, composer_height,
    permission_request_band_height, render_approval_band, render_composer,
    render_permission_request_band, render_user_input_band, should_render_side_rail,
    side_rail_width, user_input_band_height,
};
use history_rollback_overlay::render_history_rollback_overlay;
use picker::{
    command_hint_height, pending_control_height, render_command_hint_band,
    render_pending_control_band,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Block;
use shell::{bottom_layout_constraints, render_main_pane, render_side_rail};
use statusline::{render_status_line, render_toast_band, toast_height};
use theme::palette;

pub(crate) fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    approval: Option<&ApprovalPrompt>,
    permission_request: Option<&PermissionRequestPrompt>,
    user_input: Option<&UserInputView<'_>>,
) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().bg)),
        area,
    );

    let prompt_height = approval
        .map(approval_band_height)
        .or_else(|| permission_request.map(permission_request_band_height))
        .or_else(|| user_input.map(user_input_band_height));
    let pending_height =
        if approval.is_none() && permission_request.is_none() && user_input.is_none() {
            pending_control_height(state)
        } else {
            None
        };
    let command_hint = if approval.is_none() && permission_request.is_none() && user_input.is_none()
    {
        slash_command_hint(&state.input, state.command_completion_index)
    } else {
        None
    };
    let command_hint_height = command_hint.as_ref().map(command_hint_height);
    let toast_height = toast_height(state);
    let composer_height = composer_height(state, user_input);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            prompt_height,
            pending_height,
            command_hint_height,
            toast_height,
            composer_height,
        ))
        .split(area);
    let mut next_index = 0;
    let main_area = vertical[next_index];
    next_index += 1;
    let prompt_area = prompt_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let pending_area = pending_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let command_hint_area = command_hint_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let toast_area = toast_height.map(|_| {
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
        render_approval_band(frame, prompt_area.expect("approval area"), approval);
    } else if let Some(permission_request) = permission_request {
        render_permission_request_band(
            frame,
            prompt_area.expect("permission request area"),
            permission_request,
        );
    } else if let Some(user_input) = user_input {
        render_user_input_band(frame, prompt_area.expect("user input area"), user_input);
    }
    if pending_height.is_some() {
        render_pending_control_band(frame, pending_area.expect("pending area"), state);
    }
    if let Some(command_hint) = command_hint.as_ref() {
        render_command_hint_band(
            frame,
            command_hint_area.expect("command hint area"),
            command_hint,
        );
    }
    if toast_height.is_some() {
        render_toast_band(frame, toast_area.expect("toast area"), state);
    }
    render_composer(frame, composer_area, state, user_input);
    render_status_line(frame, status_area, state);
    if state.history_rollback_overlay().is_some() {
        render_history_rollback_overlay(frame, area, state);
    }

    if state.history_rollback_overlay().is_none() {
        frame.set_cursor_position(composer_cursor_position(composer_area, state, user_input));
    }
}

pub(crate) fn main_pane_viewport_height(
    area: Rect,
    state: &TuiState,
    approval: Option<&ApprovalPrompt>,
    permission_request: Option<&PermissionRequestPrompt>,
    user_input: Option<&UserInputView<'_>>,
) -> u16 {
    let prompt_height = approval
        .map(approval_band_height)
        .or_else(|| permission_request.map(permission_request_band_height))
        .or_else(|| user_input.map(user_input_band_height));
    let pending_height =
        if approval.is_none() && permission_request.is_none() && user_input.is_none() {
            pending_control_height(state)
        } else {
            None
        };
    let command_hint = if approval.is_none() && permission_request.is_none() && user_input.is_none()
    {
        slash_command_hint(&state.input, state.command_completion_index)
    } else {
        None
    };
    let command_hint_height = command_hint.as_ref().map(command_hint_height);
    let toast_height = toast_height(state);
    let composer_height = composer_height(state, user_input);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            prompt_height,
            pending_height,
            command_hint_height,
            toast_height,
            composer_height,
        ))
        .split(area);
    vertical
        .first()
        .map(|rect| rect.height.max(1))
        .unwrap_or(area.height.max(1))
}

#[cfg(test)]
mod tests;
