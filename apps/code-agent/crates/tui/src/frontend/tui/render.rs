mod chrome;
mod history_rollback_overlay;
mod picker;
mod shared;
mod shell;
mod statusline;
mod theme;
mod tool_review_overlay;
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
    composer_cursor_position, composer_height, render_approval_modal, render_composer,
    render_permission_request_modal, render_user_input_band, user_input_band_height,
};
use history_rollback_overlay::render_history_rollback_overlay;
use picker::{pending_control_height, render_command_hint_modal, render_pending_control_band};
use ratatui::layout::{Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Block;
use shell::{bottom_layout_constraints, render_main_pane};
use statusline::{render_status_line, render_toast_band, toast_height};
use theme::palette;
use tool_review_overlay::render_tool_review_overlay;

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

    let approval_active = approval.is_some();
    let permission_request_active = permission_request.is_some();
    let prompt_height = if permission_request_active {
        None
    } else {
        user_input.map(user_input_band_height)
    };
    let pending_height = if !approval_active && !permission_request_active && user_input.is_none() {
        pending_control_height(state)
    } else {
        None
    };
    let command_hint = if !approval_active && !permission_request_active && user_input.is_none() {
        slash_command_hint(&state.input, state.command_completion_index)
    } else {
        None
    };
    let toast_height = toast_height(state);
    let composer_height = composer_height(state, user_input);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            prompt_height,
            pending_height,
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
    let toast_area = toast_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let composer_area = vertical[next_index];
    let status_area = vertical[next_index + 1];

    render_main_pane(frame, main_area, state);
    if let Some(user_input) = user_input {
        render_user_input_band(frame, prompt_area.expect("user input area"), user_input);
    }
    if pending_height.is_some() {
        render_pending_control_band(frame, pending_area.expect("pending area"), state);
    }
    if let Some(command_hint) = command_hint.as_ref() {
        render_command_hint_modal(frame, area, command_hint);
    }
    if toast_height.is_some() {
        render_toast_band(frame, toast_area.expect("toast area"), state);
    }
    render_composer(frame, composer_area, state, user_input);
    render_status_line(frame, status_area, state);
    if let Some(approval) = approval {
        render_approval_modal(frame, area, approval);
    }
    if let Some(permission_request) = permission_request {
        render_permission_request_modal(frame, area, permission_request);
    }
    if state.history_rollback_overlay().is_some() {
        render_history_rollback_overlay(frame, area, state);
    }
    if state.tool_review_overlay().is_some() {
        render_tool_review_overlay(frame, area, state);
    }

    if state.history_rollback_overlay().is_none()
        && state.tool_review_overlay().is_none()
        && approval.is_none()
        && permission_request.is_none()
    {
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
    let approval_active = approval.is_some();
    let permission_request_active = permission_request.is_some();
    let prompt_height = if permission_request_active {
        None
    } else {
        user_input.map(user_input_band_height)
    };
    let pending_height = if !approval_active && !permission_request_active && user_input.is_none() {
        pending_control_height(state)
    } else {
        None
    };
    let toast_height = toast_height(state);
    let composer_height = composer_height(state, user_input);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            prompt_height,
            pending_height,
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
