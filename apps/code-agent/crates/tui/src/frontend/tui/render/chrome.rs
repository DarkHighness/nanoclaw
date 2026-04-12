use super::super::UserInputView;
use super::super::approval::ApprovalPrompt;
use super::super::state::{ComposerContextHint, TuiState, preview_text};
use super::shared::{
    composer_cursor_metrics, pending_control_focus_label, pending_control_kind_label,
};
use super::shell::bottom_band_inner_area;
use super::theme::palette;
use super::transcript_markdown::code_span;
use crate::backend::preview_id;
use crate::interaction::{
    ApprovalOrigin, PendingControlKind, PermissionProfile, PermissionRequestPrompt,
};
use crate::preview::{PreviewCollapse, collapse_preview_lines};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const MAX_COMPOSER_HEIGHT: u16 = 8;

pub(super) fn render_composer(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().footer_bg)),
        area,
    );
    let inner = bottom_band_inner_area(area);
    let scroll = composer_scroll(state, user_input, inner.height);
    frame.render_widget(
        Paragraph::new(build_composer_text(state, user_input))
            .scroll((scroll, 0))
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        inner,
    );
}

pub(super) fn composer_height(state: &TuiState, user_input: Option<&UserInputView<'_>>) -> u16 {
    composer_text_line_count(state, user_input).clamp(2, MAX_COMPOSER_HEIGHT)
}

pub(super) fn composer_cursor_position(
    area: Rect,
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> Position {
    let inner = bottom_band_inner_area(area);
    let scroll = composer_scroll(state, user_input, inner.height);
    let (base_line, column) = composer_cursor_metrics(&state.input, state.input_cursor);
    let line = base_line.saturating_add(composer_attachment_row_count(state, user_input));
    let lead_width = if base_line == 0 {
        first_input_line_lead_width(state, user_input)
    } else {
        0
    };
    Position::new(
        inner
            .x
            .saturating_add(2)
            .saturating_add(lead_width)
            .saturating_add(column),
        inner.y.saturating_add(line.saturating_sub(scroll)),
    )
}

pub(super) fn render_approval_modal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    approval: &ApprovalPrompt,
) {
    let popup = centered_rect(area, 76, 42);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(" Approval ")
            .title_style(
                Style::default()
                    .fg(palette().warn)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().warn))
            .style(Style::default().bg(palette().footer_bg)),
        popup,
    );
    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_approval_text(approval))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        inner,
    );
}

pub(super) fn render_user_input_band(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    user_input: &UserInputView<'_>,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().bottom_pane_bg)),
        area,
    );
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(build_user_input_text(user_input))
            .wrap(Wrap { trim: false })
            .style(
                Style::default()
                    .fg(palette().text)
                    .bg(palette().bottom_pane_bg),
            ),
        inner,
    );
}

pub(super) fn render_permission_request_modal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    prompt: &PermissionRequestPrompt,
) {
    let popup = centered_rect(area, 78, 58);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(" Permissions ")
            .title_style(
                Style::default()
                    .fg(palette().warn)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().warn))
            .style(Style::default().bg(palette().footer_bg)),
        popup,
    );
    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_permission_request_text(prompt))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        inner,
    );
}

pub(super) fn build_approval_text(approval: &ApprovalPrompt) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "Approval Required",
            Style::default()
                .fg(palette().warn)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            approval.tool_name.clone(),
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if let Some(context) = approval_context_line(approval) {
        lines.push(context);
    }
    for (index, line) in approval_preview_lines(&approval.content.preview)
        .into_iter()
        .enumerate()
    {
        lines.push(approval_detail_line(
            (index == 0).then_some(approval.content.kind.as_str()),
            vec![code_span(&line)],
        ));
    }
    if !approval.reasons.is_empty() {
        lines.extend(
            approval
                .reasons
                .iter()
                .take(2)
                .enumerate()
                .map(|(index, reason)| {
                    approval_detail_line(
                        (index == 0).then_some("Reason"),
                        vec![Span::styled(
                            preview_text(reason, 96),
                            Style::default().fg(palette().muted),
                        )],
                    )
                }),
        );
    }
    lines.push(approval_detail_line(
        Some("Keys"),
        vec![
            Span::styled("y", Style::default().fg(palette().accent)),
            Span::styled(" approve", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("n", Style::default().fg(palette().error)),
            Span::styled(" deny", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("esc", Style::default().fg(palette().header)),
            Span::styled(" dismiss", Style::default().fg(palette().muted)),
        ],
    ));
    Text::from(lines)
}

pub(super) fn user_input_band_height(user_input: &UserInputView<'_>) -> u16 {
    build_user_input_text(user_input).lines.len().clamp(6, 12) as u16
}

#[cfg(test)]
pub(super) fn should_render_side_rail(state: &TuiState, area: Rect) -> bool {
    // Plan and execution state now belong to transcript-native system cells, so
    // the live timeline keeps full width instead of competing with a side rail.
    let _ = (state, area);
    false
}

fn approval_section_label(label: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        label.to_string(),
        Style::default()
            .fg(palette().muted)
            .add_modifier(Modifier::BOLD),
    )])
}

fn approval_context_line(approval: &ApprovalPrompt) -> Option<Line<'static>> {
    let mut spans = Vec::new();
    match &approval.origin {
        ApprovalOrigin::Local => {}
        ApprovalOrigin::Mcp { server_name } => {
            spans.push(Span::styled(
                format!("mcp:{server_name}"),
                Style::default().fg(palette().muted),
            ));
        }
        ApprovalOrigin::Provider { provider } => {
            spans.push(Span::styled(
                format!("provider:{provider}"),
                Style::default().fg(palette().muted),
            ));
        }
    }
    if let Some(working_directory) = approval.working_directory.as_deref() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        }
        spans.push(Span::styled(
            preview_text(working_directory, 56),
            Style::default().fg(palette().text),
        ));
    }
    if let Some(mode) = approval.mode.as_deref() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        }
        spans.push(Span::styled(
            mode.to_string(),
            Style::default().fg(palette().accent),
        ));
    }
    (!spans.is_empty()).then(|| approval_detail_line(Some("Context"), spans))
}

fn approval_detail_line(label: Option<&str>, mut body: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled("  ", Style::default().fg(palette().subtle))];
    if let Some(label) = label {
        spans.push(Span::styled(
            format!("{label:<8}"),
            Style::default()
                .fg(palette().muted)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    } else {
        spans.push(Span::styled(
            " ".repeat(9),
            Style::default().fg(palette().subtle),
        ));
    }
    spans.append(&mut body);
    Line::from(spans)
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

pub(super) fn approval_preview_lines(lines: &[String]) -> Vec<String> {
    collapse_preview_lines(lines, 4, PreviewCollapse::Head)
}

pub(super) fn build_composer_line(state: &TuiState) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            "›",
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if let Some(editing) = state.editing_pending_control.as_ref() {
        spans.push(Span::styled(
            match editing.kind {
                PendingControlKind::Prompt => "edit queued prompt",
                PendingControlKind::Steer => "edit queued steer",
            },
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    }
    if state.input.is_empty() {
        if state.history_rollback_is_primed() {
            spans.push(Span::styled(
                "history rollback armed",
                Style::default().fg(palette().header),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "esc choose turn",
                Style::default().fg(palette().muted),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "type to cancel",
                Style::default().fg(palette().muted),
            ));
        } else if let Some(overlay) = state.history_rollback_overlay() {
            if let Some(selected) = state.selected_history_rollback_candidate() {
                spans.push(Span::styled(
                    "rollback ",
                    Style::default().fg(palette().muted),
                ));
                spans.push(Span::styled(
                    format!("{}/{}", overlay.selected + 1, overlay.candidates.len()),
                    Style::default().fg(palette().header),
                ));
                spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
                spans.push(Span::styled(
                    crate::frontend::tui::state::draft_preview_text(
                        &selected.draft,
                        &selected.prompt,
                        32,
                    ),
                    Style::default().fg(palette().text),
                ));
            } else {
                spans.push(Span::styled(
                    "history rollback",
                    Style::default().fg(palette().muted),
                ));
            }
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "enter rollback",
                Style::default().fg(palette().muted),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "esc/← older",
                Style::default().fg(palette().muted),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "→ newer",
                Style::default().fg(palette().muted),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "q cancel",
                Style::default().fg(palette().muted),
            ));
        } else if state.pending_control_picker.is_some() {
            if let (Some(selected), Some(picker)) = (
                state.selected_pending_control(),
                state.pending_control_picker.as_ref(),
            ) {
                spans.push(Span::styled(
                    "selected ",
                    Style::default().fg(palette().muted),
                ));
                spans.push(Span::styled(
                    pending_control_kind_label(selected.kind),
                    Style::default().fg(palette().header),
                ));
                spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
                spans.push(Span::styled(
                    pending_control_focus_label(picker.selected, state.pending_controls.len()),
                    Style::default().fg(palette().accent),
                ));
            } else {
                spans.push(Span::styled(
                    "pending queue",
                    Style::default().fg(palette().muted),
                ));
            }
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "enter edit",
                Style::default().fg(palette().muted),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "del withdraw",
                Style::default().fg(palette().muted),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                "esc close",
                Style::default().fg(palette().muted),
            ));
        } else if let Some(hint) = state.composer_context_hint.as_ref() {
            spans.extend(composer_context_hint_spans(state, hint));
        } else {
            spans.push(Span::styled(
                "Type a prompt or /help",
                Style::default().fg(palette().subtle),
            ));
        }
        return Line::from(spans);
    }

    if state.input.starts_with('/') {
        let (command, tail) = state
            .input
            .split_once(' ')
            .map_or((state.input.as_str(), None), |(command, tail)| {
                (command, Some(tail))
            });
        spans.push(Span::styled(
            command.to_string(),
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ));
        if let Some(tail) = tail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                tail.to_string(),
                Style::default().fg(palette().text),
            ));
        }
    } else {
        spans.push(Span::styled(
            state.input.clone(),
            Style::default().fg(palette().text),
        ));
    }

    if state.editing_pending_control.is_some() {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "enter/tab save",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "esc cancel",
            Style::default().fg(palette().muted),
        ));
    }

    Line::from(spans)
}

pub(super) fn build_composer_text(
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> Text<'static> {
    if composer_uses_multiline_layout(state, user_input) {
        build_multiline_composer_text(state, user_input)
    } else {
        Text::from(match user_input {
            Some(view) => build_user_input_composer_line(view),
            None => build_composer_line(state),
        })
    }
}

pub(super) fn build_user_input_text(user_input: &UserInputView<'_>) -> Text<'static> {
    let mut lines = Vec::new();
    let current_index = user_input
        .flow
        .map(|flow| flow.current_question)
        .unwrap_or(0)
        .min(user_input.prompt.questions.len().saturating_sub(1));
    let question = &user_input.prompt.questions[current_index];
    let answered = user_input
        .flow
        .map(|flow| flow.answers.len())
        .unwrap_or(0)
        .min(user_input.prompt.questions.len());
    let collecting_other_note = user_input
        .flow
        .is_some_and(|flow| flow.collecting_other_note);

    lines.push(Line::from(vec![
        Span::styled(
            "user input",
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!(
                "Question {}/{}",
                current_index + 1,
                user_input.prompt.questions.len()
            ),
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(question.header.clone(), Style::default().fg(palette().text)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{answered} answered"),
            Style::default().fg(palette().muted),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(question.id.clone(), Style::default().fg(palette().subtle)),
    ]));
    lines.push(Line::from(vec![Span::styled(
        question.question.clone(),
        Style::default().fg(palette().text),
    )]));

    if collecting_other_note {
        lines.push(approval_section_label("other"));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled(
                "Type the alternate answer and press Enter.",
                Style::default().fg(palette().muted),
            ),
        ]));
        if !user_input.input.trim().is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(palette().subtle)),
                code_span(user_input.input.trim()),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("enter", Style::default().fg(palette().accent)),
            Span::styled(" submit", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("esc", Style::default().fg(palette().header)),
            Span::styled(" back to options", Style::default().fg(palette().muted)),
        ]));
    } else {
        lines.push(approval_section_label("options"));
        for (index, option) in question.options.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(
                    format!("{}", index + 1),
                    Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", Style::default().fg(palette().subtle)),
                Span::styled(option.label.clone(), Style::default().fg(palette().text)),
                Span::styled(" · ", Style::default().fg(palette().subtle)),
                Span::styled(
                    option.description.clone(),
                    Style::default().fg(palette().muted),
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled(
                "0",
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(palette().subtle)),
            Span::styled("Other", Style::default().fg(palette().text)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(
                "Provide a different answer with a note.",
                Style::default().fg(palette().muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("1-9", Style::default().fg(palette().accent)),
            Span::styled(" choose", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("0", Style::default().fg(palette().header)),
            Span::styled(" other", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("esc", Style::default().fg(palette().error)),
            Span::styled(" cancel", Style::default().fg(palette().muted)),
        ]));
    }

    Text::from(lines)
}

pub(super) fn build_permission_request_text(prompt: &PermissionRequestPrompt) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "permissions",
            Style::default()
                .fg(palette().warn)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            "Grant additional permissions?",
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if let Some(reason) = prompt.reason.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("reason", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(
                preview_text(reason, 96),
                Style::default().fg(palette().text),
            ),
        ]));
    }
    lines.push(approval_section_label("requested"));
    lines.extend(permission_profile_lines(&prompt.requested));
    if !prompt.current_turn.is_empty() || !prompt.current_session.is_empty() {
        if !prompt.current_turn.is_empty() {
            lines.push(approval_section_label("current turn"));
            lines.extend(permission_profile_lines(&prompt.current_turn));
        }
        if !prompt.current_session.is_empty() {
            lines.push(approval_section_label("current session"));
            lines.extend(permission_profile_lines(&prompt.current_session));
        }
    }
    lines.push(Line::from(vec![
        Span::styled("y", Style::default().fg(palette().accent)),
        Span::styled(" grant once", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("a", Style::default().fg(palette().header)),
        Span::styled(" grant for session", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("n", Style::default().fg(palette().error)),
        Span::styled(" deny", Style::default().fg(palette().muted)),
    ]));
    Text::from(lines)
}

fn permission_profile_lines(profile: &PermissionProfile) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !profile.read_roots.is_empty() {
        lines.push(permission_profile_line("read", &profile.read_roots));
    }
    if !profile.write_roots.is_empty() {
        lines.push(permission_profile_line("write", &profile.write_roots));
    }
    if profile.network_full {
        lines.push(permission_profile_line("network", &["full".to_string()]));
    }
    if !profile.network_domains.is_empty() {
        lines.push(permission_profile_line("domains", &profile.network_domains));
    }
    if lines.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(palette().subtle)),
            Span::styled("none", Style::default().fg(palette().muted)),
        ]));
    }
    lines
}

fn permission_profile_line(label: &str, values: &[String]) -> Line<'static> {
    let clipped = values
        .iter()
        .map(|value| {
            if value.chars().count() > 88 {
                format!("{}...", value.chars().take(85).collect::<String>())
            } else {
                value.clone()
            }
        })
        .collect::<Vec<_>>();
    let preview = collapse_preview_lines(&clipped, 2, PreviewCollapse::HeadTail).join(" · ");
    Line::from(vec![
        Span::styled("  • ", Style::default().fg(palette().subtle)),
        Span::styled(format!("{label}: "), Style::default().fg(palette().muted)),
        Span::styled(preview, Style::default().fg(palette().text)),
    ])
}

fn build_user_input_composer_line(user_input: &UserInputView<'_>) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            "›",
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    let collecting_other_note = user_input
        .flow
        .is_some_and(|flow| flow.collecting_other_note);
    if collecting_other_note {
        spans.push(Span::styled(
            "other note",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        if user_input.input.is_empty() {
            spans.push(Span::styled(
                "type an alternate answer",
                Style::default().fg(palette().subtle),
            ));
        } else {
            spans.push(Span::styled(
                user_input.input.to_string(),
                Style::default().fg(palette().text),
            ));
        }
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "enter submit",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "esc options",
            Style::default().fg(palette().muted),
        ));
    } else {
        spans.push(Span::styled(
            "choose an option in the prompt above",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "1-9 select",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "0 other",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            "esc cancel",
            Style::default().fg(palette().muted),
        ));
    }
    Line::from(spans)
}

fn composer_uses_multiline_layout(
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> bool {
    composer_attachment_row_count(state, user_input) > 0
        || state.input.contains('\n')
            && user_input.is_none_or(|view| {
                view.flow
                    .is_none_or(|flow| flow.collecting_other_note || !state.input.is_empty())
            })
}

fn composer_text_line_count(state: &TuiState, user_input: Option<&UserInputView<'_>>) -> u16 {
    build_composer_text(state, user_input)
        .lines
        .len()
        .max(1)
        .min(u16::MAX as usize) as u16
}

fn composer_scroll(state: &TuiState, user_input: Option<&UserInputView<'_>>, height: u16) -> u16 {
    if !composer_uses_multiline_layout(state, user_input) {
        return 0;
    }
    let (cursor_line, _) = composer_cursor_metrics(&state.input, state.input_cursor);
    let cursor_line = cursor_line.saturating_add(composer_attachment_row_count(state, user_input));
    let total_lines = composer_text_line_count(state, user_input);
    let viewport_height = height.max(1);
    let max_scroll = total_lines.saturating_sub(viewport_height);
    cursor_line
        .saturating_sub(viewport_height.saturating_sub(1))
        .min(max_scroll)
}

fn build_multiline_composer_text(
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> Text<'static> {
    let mut lines = build_attachment_rows(state, user_input);
    lines.extend(build_multiline_input_lines(
        &state.input,
        first_input_line_lead(state, user_input),
    ));
    if let Some(hint_line) = multiline_hint_line(state, user_input) {
        lines.push(hint_line);
    }
    Text::from(lines)
}

fn composer_attachment_row_count(state: &TuiState, user_input: Option<&UserInputView<'_>>) -> u16 {
    if user_input.is_some() {
        return 0;
    }
    state
        .row_attachment_summaries()
        .len()
        .min(u16::MAX as usize) as u16
}

fn build_attachment_rows(
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> Vec<Line<'static>> {
    if user_input.is_some() {
        return Vec::new();
    }
    state
        .row_attachment_summaries()
        .into_iter()
        .map(|(index, summary, detail)| {
            let selected = state.selected_row_attachment == Some(index.saturating_sub(1));
            let marker = if selected { "›" } else { "·" };
            let index_style = if selected {
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().muted)
            };
            let summary_style = if selected {
                Style::default()
                    .fg(palette().text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette().header)
            };
            let detail_style = if selected {
                Style::default().fg(palette().text)
            } else {
                Style::default().fg(palette().muted)
            };
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(palette().accent)),
                Span::styled(" ", Style::default().fg(palette().subtle)),
                Span::styled(format!("#{index} "), index_style),
                Span::styled(summary, summary_style),
            ];
            if !detail.is_empty() {
                spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
                spans.push(Span::styled(detail, detail_style));
            }
            Line::from(spans)
        })
        .collect()
}

fn build_multiline_input_lines(input: &str, lead: Option<String>) -> Vec<Line<'static>> {
    input
        .split('\n')
        .enumerate()
        .map(|(index, segment)| {
            let mut spans = vec![
                Span::styled(
                    if index == 0 { "›" } else { " " },
                    Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ];
            if index == 0
                && let Some(lead) = lead.as_ref()
            {
                spans.push(Span::styled(
                    lead.clone(),
                    Style::default().fg(palette().muted),
                ));
            }
            spans.push(Span::styled(
                segment.to_string(),
                Style::default().fg(palette().text),
            ));
            Line::from(spans)
        })
        .collect()
}

fn first_input_line_lead(
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> Option<String> {
    if user_input
        .and_then(|view| view.flow)
        .is_some_and(|flow| flow.collecting_other_note)
    {
        return Some("other note · ".to_string());
    }

    state.editing_pending_control.as_ref().map(|editing| {
        format!(
            "{} · ",
            match editing.kind {
                PendingControlKind::Prompt => "edit queued prompt",
                PendingControlKind::Steer => "edit queued steer",
            }
        )
    })
}

fn first_input_line_lead_width(state: &TuiState, user_input: Option<&UserInputView<'_>>) -> u16 {
    first_input_line_lead(state, user_input)
        .map(|lead| super::shared::composer_cursor_width(&lead))
        .unwrap_or(0)
}

fn multiline_hint_line(
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) -> Option<Line<'static>> {
    if state.selected_row_attachment.is_some() {
        return Some(Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled("delete detach", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("up/down move", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("down draft", Style::default().fg(palette().muted)),
        ]));
    }

    if user_input
        .and_then(|view| view.flow)
        .is_some_and(|flow| flow.collecting_other_note)
    {
        return Some(Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled("enter submit", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("esc options", Style::default().fg(palette().muted)),
        ]));
    }

    state.editing_pending_control.as_ref().map(|_| {
        Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled("enter/tab save", Style::default().fg(palette().muted)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled("esc cancel", Style::default().fg(palette().muted)),
        ])
    })
}

fn composer_context_hint_spans(state: &TuiState, hint: &ComposerContextHint) -> Vec<Span<'static>> {
    match hint {
        ComposerContextHint::LiveTaskFinished { task_id, status } => {
            let status_label = status.to_string();
            let status_style = match status {
                agent::types::AgentStatus::Completed => Style::default().fg(palette().assistant),
                agent::types::AgentStatus::Failed => Style::default().fg(palette().error),
                agent::types::AgentStatus::Cancelled => Style::default().fg(palette().warn),
                _ => Style::default().fg(palette().header),
            };
            let mut spans = vec![
                Span::styled("task ", Style::default().fg(palette().muted)),
                Span::styled(preview_id(task_id), Style::default().fg(palette().header)),
                Span::styled(" ", Style::default().fg(palette().subtle)),
                Span::styled(status_label, status_style),
                Span::styled(" · ", Style::default().fg(palette().subtle)),
            ];
            if state.turn_running {
                spans.push(Span::styled("enter", Style::default().fg(palette().accent)));
                spans.push(Span::styled(" steer", Style::default().fg(palette().muted)));
                spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
                spans.push(Span::styled("tab", Style::default().fg(palette().header)));
                spans.push(Span::styled(" queue", Style::default().fg(palette().muted)));
            } else {
                spans.push(Span::styled(
                    "type follow-up",
                    Style::default().fg(palette().muted),
                ));
            }
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled("/task", Style::default().fg(palette().accent)));
            spans.push(Span::styled(
                " inspect",
                Style::default().fg(palette().muted),
            ));
            spans
        }
    }
}
