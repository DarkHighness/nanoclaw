use super::super::approval::ApprovalPrompt;
use super::super::state::{MainPaneMode, TodoEntry, TuiState, preview_text};
use super::shell::bottom_band_inner_area;
use super::theme::{
    ACCENT, ASSISTANT, BOTTOM_PANE_BG, ERROR, FOOTER_BG, HEADER, MUTED, SUBTLE, TEXT, USER, WARN,
};
use super::transcript_markdown::code_span;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

pub(super) fn render_composer(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(FOOTER_BG)), area);
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(build_composer_line(state)).style(Style::default().fg(TEXT).bg(FOOTER_BG)),
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

pub(super) fn should_render_side_rail(state: &TuiState, area: Rect) -> bool {
    state.main_pane == MainPaneMode::Transcript
        && area.width >= 128
        && (lsp_side_rail_available(state) || !state.todo_items.is_empty())
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

    if !state.todo_items.is_empty() {
        lines.push(section_title_line("TODO", USER));
        let (active, pending, done) = todo_counts(&state.todo_items);
        lines.push(rail_summary_line(format!(
            "{active} active · {pending} pending · {done} done"
        )));
        let mut todo_items = state.todo_items.iter().collect::<Vec<_>>();
        todo_items.sort_by_key(|item| (todo_status_rank(&item.status), item.content.as_str()));
        let visible = todo_items.iter().take(5).copied().collect::<Vec<_>>();
        lines.extend(visible.iter().map(|item| render_todo_line(item)));
        if todo_items.len() > visible.len() {
            lines.push(rail_summary_line(format!(
                "+{} more",
                todo_items.len() - visible.len()
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
    if lines.len() <= 4 {
        return lines.to_vec();
    }

    let mut preview = lines.iter().take(2).cloned().collect::<Vec<_>>();
    preview.push("...".to_string());
    preview.extend(lines.iter().skip(lines.len().saturating_sub(1)).cloned());
    preview
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
        if state.pending_control_picker.is_some() {
            spans.push(Span::styled("pending queue", Style::default().fg(MUTED)));
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

fn todo_counts(items: &[TodoEntry]) -> (usize, usize, usize) {
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

fn todo_status_rank(status: &str) -> usize {
    match status {
        "in_progress" => 0,
        "pending" => 1,
        "completed" => 2,
        _ => 3,
    }
}

fn render_todo_line(item: &TodoEntry) -> Line<'static> {
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
