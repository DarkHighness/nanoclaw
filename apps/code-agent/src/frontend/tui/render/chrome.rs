use super::super::UserInputView;
use super::super::approval::ApprovalPrompt;
use super::super::state::{MainPaneMode, PlanEntry, TuiState, preview_text};
use super::shared::{pending_control_focus_label, pending_control_kind_label};
use super::shell::bottom_band_inner_area;
use super::theme::{
    ACCENT, ASSISTANT, BOTTOM_PANE_BG, ERROR, FOOTER_BG, HEADER, MUTED, SUBTLE, TEXT, USER, WARN,
};
use super::transcript_markdown::code_span;
use crate::preview::{PreviewCollapse, collapse_preview_lines};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

pub(super) fn render_composer(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &TuiState,
    user_input: Option<&UserInputView<'_>>,
) {
    frame.render_widget(Block::default().style(Style::default().bg(FOOTER_BG)), area);
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(match user_input {
            Some(view) => build_user_input_composer_line(view),
            None => build_composer_line(state),
        })
        .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        inner,
    );
}

pub(super) fn render_approval_band(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    approval: &ApprovalPrompt,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(build_approval_text(approval))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        inner,
    );
}

pub(super) fn render_user_input_band(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    user_input: &UserInputView<'_>,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(build_user_input_text(user_input))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        inner,
    );
}

pub(super) fn build_approval_text(approval: &ApprovalPrompt) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "approval",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            format!("Approve {}?", approval.tool_name),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
    ])];
    if let Some(context) = approval_context_line(approval) {
        lines.push(context);
    }
    lines.push(approval_section_label(&approval.content_label));
    for line in approval_preview_lines(&approval.content_preview) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(SUBTLE)),
            code_span(&line),
        ]));
    }
    if !approval.reasons.is_empty() {
        lines.push(approval_section_label("why"));
        lines.extend(approval.reasons.iter().take(2).map(|reason| {
            Line::from(vec![
                Span::styled("  • ", Style::default().fg(SUBTLE)),
                Span::styled(preview_text(reason, 96), Style::default().fg(MUTED)),
            ])
        }));
    }
    lines.push(Line::from(vec![
        Span::styled("y", Style::default().fg(ACCENT)),
        Span::styled(" approve", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("n", Style::default().fg(ERROR)),
        Span::styled(" deny", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("esc", Style::default().fg(HEADER)),
        Span::styled(" dismiss", Style::default().fg(MUTED)),
    ]));
    Text::from(lines)
}

pub(super) fn approval_band_height(approval: &ApprovalPrompt) -> u16 {
    build_approval_text(approval).lines.len().clamp(5, 10) as u16
}

pub(super) fn user_input_band_height(user_input: &UserInputView<'_>) -> u16 {
    build_user_input_text(user_input).lines.len().clamp(6, 12) as u16
}

pub(super) fn should_render_side_rail(state: &TuiState, area: Rect) -> bool {
    state.main_pane == MainPaneMode::Transcript
        && area.width >= 128
        && (lsp_side_rail_available(state) || !state.plan_items.is_empty())
}

pub(super) fn side_rail_width(total_width: u16) -> u16 {
    total_width.saturating_mul(22) / 100
}

pub(super) fn build_side_rail_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if lsp_side_rail_available(state) {
        lines.push(section_title_line("LSP", ACCENT));
        let degraded = state
            .session
            .startup_diagnostics
            .warnings
            .iter()
            .any(|warning| warning.contains("managed code-intel"));
        let warning_count = state.session.startup_diagnostics.warnings.len();
        let diagnostic_count = state.session.startup_diagnostics.diagnostics.len();
        lines.push(status_line(
            if degraded { "degraded" } else { "ready" },
            if degraded { WARN } else { ASSISTANT },
        ));
        lines.push(rail_summary_line(format!(
            "{warning_count} warnings · {diagnostic_count} diagnostics"
        )));
        let lsp_notes = state
            .session
            .startup_diagnostics
            .warnings
            .iter()
            .map(|warning| (preview_text(warning, 40), WARN))
            .chain(
                state
                    .session
                    .startup_diagnostics
                    .diagnostics
                    .iter()
                    .map(|diagnostic| (preview_text(diagnostic, 40), ACCENT)),
            )
            .take(3)
            .collect::<Vec<_>>();
        if lsp_notes.is_empty() {
            lines.push(rail_summary_line("No diagnostics yet."));
        } else {
            lines.extend(
                lsp_notes
                    .into_iter()
                    .map(|(note, color)| bullet_line(&note, color)),
            );
        }
        lines.push(Line::raw(""));
    }

    if !state.plan_items.is_empty() {
        lines.push(section_title_line("Plan", USER));
        let (active, pending, done) = plan_counts(&state.plan_items);
        lines.push(rail_summary_line(format!(
            "{active} active · {pending} pending · {done} done"
        )));
        let mut plan_items = state.plan_items.iter().collect::<Vec<_>>();
        plan_items.sort_by_key(|item| (plan_status_rank(&item.status), item.content.as_str()));
        let visible = plan_items.iter().take(5).copied().collect::<Vec<_>>();
        lines.extend(visible.iter().map(|item| render_plan_line(item)));
        if plan_items.len() > visible.len() {
            lines.push(rail_summary_line(format!(
                "+{} more",
                plan_items.len() - visible.len()
            )));
        }
    }

    if lines.is_empty() {
        lines.push(section_title_line("Context", MUTED));
        lines.push(Line::from(Span::styled(
            "No live side context.",
            Style::default().fg(SUBTLE),
        )));
    }

    lines
}

fn approval_section_label(label: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        label.to_string(),
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    )])
}

fn approval_context_line(approval: &ApprovalPrompt) -> Option<Line<'static>> {
    let mut spans = Vec::new();
    if approval.origin != "local" {
        spans.push(Span::styled(
            approval.origin.clone(),
            Style::default().fg(MUTED),
        ));
    }
    if let Some(working_directory) = approval.working_directory.as_deref() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        }
        spans.push(Span::styled(
            preview_text(working_directory, 56),
            Style::default().fg(TEXT),
        ));
    }
    if let Some(mode) = approval.mode.as_deref() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        }
        spans.push(Span::styled(mode.to_string(), Style::default().fg(ACCENT)));
    }
    (!spans.is_empty()).then(|| Line::from(spans))
}

pub(super) fn approval_preview_lines(lines: &[String]) -> Vec<String> {
    collapse_preview_lines(lines, 4, PreviewCollapse::Head)
}

pub(super) fn build_composer_line(state: &TuiState) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            "›",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if let Some(editing) = state.editing_pending_control.as_ref() {
        spans.push(Span::styled(
            match editing.kind {
                crate::backend::PendingControlKind::Prompt => "edit queued prompt",
                crate::backend::PendingControlKind::Steer => "edit queued steer",
            },
            Style::default().fg(MUTED),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
    }
    if state.input.is_empty() {
        if state.history_rollback_is_primed() {
            spans.push(Span::styled(
                "history rollback armed",
                Style::default().fg(HEADER),
            ));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("esc choose turn", Style::default().fg(MUTED)));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("type to cancel", Style::default().fg(MUTED)));
        } else if let Some(overlay) = state.history_rollback_overlay() {
            if let Some(selected) = state.selected_history_rollback_candidate() {
                spans.push(Span::styled("rollback ", Style::default().fg(MUTED)));
                spans.push(Span::styled(
                    format!("{}/{}", overlay.selected + 1, overlay.candidates.len()),
                    Style::default().fg(HEADER),
                ));
                spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled(
                    preview_text(&selected.prompt, 32),
                    Style::default().fg(TEXT),
                ));
            } else {
                spans.push(Span::styled("history rollback", Style::default().fg(MUTED)));
            }
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("enter rollback", Style::default().fg(MUTED)));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("esc/← older", Style::default().fg(MUTED)));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("→ newer", Style::default().fg(MUTED)));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("q cancel", Style::default().fg(MUTED)));
        } else if state.pending_control_picker.is_some() {
            if let (Some(selected), Some(picker)) = (
                state.selected_pending_control(),
                state.pending_control_picker.as_ref(),
            ) {
                spans.push(Span::styled("selected ", Style::default().fg(MUTED)));
                spans.push(Span::styled(
                    pending_control_kind_label(selected.kind),
                    Style::default().fg(HEADER),
                ));
                spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled(
                    pending_control_focus_label(picker.selected, state.pending_controls.len()),
                    Style::default().fg(ACCENT),
                ));
            } else {
                spans.push(Span::styled("pending queue", Style::default().fg(MUTED)));
            }
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("enter edit", Style::default().fg(MUTED)));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("del withdraw", Style::default().fg(MUTED)));
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled("esc close", Style::default().fg(MUTED)));
        } else {
            spans.push(Span::styled(
                "Type a prompt or /help",
                Style::default().fg(SUBTLE),
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
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        if let Some(tail) = tail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(tail.to_string(), Style::default().fg(TEXT)));
        }
    } else {
        spans.push(Span::styled(state.input.clone(), Style::default().fg(TEXT)));
    }

    if state.editing_pending_control.is_some() {
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("enter/tab save", Style::default().fg(MUTED)));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("esc cancel", Style::default().fg(MUTED)));
    }

    Line::from(spans)
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
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            format!(
                "Question {}/{}",
                current_index + 1,
                user_input.prompt.questions.len()
            ),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(question.header.clone(), Style::default().fg(TEXT)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{answered} answered"), Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(question.id.clone(), Style::default().fg(SUBTLE)),
    ]));
    lines.push(Line::from(vec![Span::styled(
        question.question.clone(),
        Style::default().fg(TEXT),
    )]));

    if collecting_other_note {
        lines.push(approval_section_label("other"));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(SUBTLE)),
            Span::styled(
                "Type the alternate answer and press Enter.",
                Style::default().fg(MUTED),
            ),
        ]));
        if !user_input.input.trim().is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                code_span(user_input.input.trim()),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("enter", Style::default().fg(ACCENT)),
            Span::styled(" submit", Style::default().fg(MUTED)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled("esc", Style::default().fg(HEADER)),
            Span::styled(" back to options", Style::default().fg(MUTED)),
        ]));
    } else {
        lines.push(approval_section_label("options"));
        for (index, option) in question.options.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(
                    format!("{}", index + 1),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ", Style::default().fg(SUBTLE)),
                Span::styled(option.label.clone(), Style::default().fg(TEXT)),
                Span::styled(" · ", Style::default().fg(SUBTLE)),
                Span::styled(option.description.clone(), Style::default().fg(MUTED)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(SUBTLE)),
            Span::styled(
                "0",
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(SUBTLE)),
            Span::styled("Other", Style::default().fg(TEXT)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                "Provide a different answer with a note.",
                Style::default().fg(MUTED),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("1-9", Style::default().fg(ACCENT)),
            Span::styled(" choose", Style::default().fg(MUTED)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled("0", Style::default().fg(HEADER)),
            Span::styled(" other", Style::default().fg(MUTED)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled("esc", Style::default().fg(ERROR)),
            Span::styled(" cancel", Style::default().fg(MUTED)),
        ]));
    }

    Text::from(lines)
}

fn build_user_input_composer_line(user_input: &UserInputView<'_>) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            "›",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    let collecting_other_note = user_input
        .flow
        .is_some_and(|flow| flow.collecting_other_note);
    if collecting_other_note {
        spans.push(Span::styled("other note", Style::default().fg(MUTED)));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        if user_input.input.is_empty() {
            spans.push(Span::styled(
                "type an alternate answer",
                Style::default().fg(SUBTLE),
            ));
        } else {
            spans.push(Span::styled(
                user_input.input.to_string(),
                Style::default().fg(TEXT),
            ));
        }
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("enter submit", Style::default().fg(MUTED)));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("esc options", Style::default().fg(MUTED)));
    } else {
        spans.push(Span::styled(
            "choose an option in the prompt above",
            Style::default().fg(MUTED),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("1-9 select", Style::default().fg(MUTED)));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("0 other", Style::default().fg(MUTED)));
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled("esc cancel", Style::default().fg(MUTED)));
    }
    Line::from(spans)
}

fn lsp_side_rail_available(state: &TuiState) -> bool {
    state.session.tool_names.iter().any(|tool| {
        matches!(
            tool.as_str(),
            "code_symbol_search" | "code_document_symbols" | "code_definitions" | "code_references"
        )
    })
}

fn section_title_line(title: &str, accent: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled("•", Style::default().fg(accent)),
        Span::styled(" ", Style::default().fg(SUBTLE)),
        Span::styled(title.to_string(), Style::default().fg(MUTED)),
    ])
}

fn bullet_line(body: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled("•", Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(body.to_string(), Style::default().fg(MUTED)),
    ])
}

fn status_line(body: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled("●", Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(body.to_string(), Style::default().fg(color)),
    ])
}

fn rail_summary_line(body: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(body.into(), Style::default().fg(SUBTLE)))
}

fn plan_counts(items: &[PlanEntry]) -> (usize, usize, usize) {
    items
        .iter()
        .fold((0, 0, 0), |(active, pending, done), item| {
            match item.status.as_str() {
                "in_progress" => (active + 1, pending, done),
                "completed" => (active, pending, done + 1),
                _ => (active, pending + 1, done),
            }
        })
}

fn plan_status_rank(status: &str) -> usize {
    match status {
        "in_progress" => 0,
        "pending" => 1,
        "completed" => 2,
        _ => 3,
    }
}

fn render_plan_line(item: &PlanEntry) -> Line<'static> {
    let (marker, color) = match item.status.as_str() {
        "completed" => ("x", ASSISTANT),
        "in_progress" => ("~", WARN),
        _ => ("·", MUTED),
    };
    Line::from(vec![
        Span::styled("[", Style::default().fg(SUBTLE)),
        Span::styled(
            marker,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("]", Style::default().fg(SUBTLE)),
        Span::raw(" "),
        Span::styled(
            preview_text(&item.content, 30),
            if item.status == "completed" {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(TEXT)
            },
        ),
    ])
}
