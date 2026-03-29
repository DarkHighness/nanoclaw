use super::approval::ApprovalPrompt;
use super::commands::{SlashCommandHint, slash_command_hint};
use super::state::{MainPaneMode, TodoEntry, TuiState, preview_text};
use crate::backend::preview_id;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

const BG: Color = Color::Rgb(14, 15, 17);
const MAIN_BG: Color = Color::Rgb(16, 17, 19);
const FOOTER_BG: Color = Color::Rgb(18, 19, 21);
const BOTTOM_PANE_BG: Color = Color::Rgb(24, 25, 28);
const BORDER_ACTIVE: Color = Color::Rgb(178, 176, 168);
const TEXT: Color = Color::Rgb(231, 231, 227);
const MUTED: Color = Color::Rgb(157, 158, 152);
const SUBTLE: Color = Color::Rgb(112, 114, 109);
const USER: Color = Color::Rgb(214, 197, 167);
const ASSISTANT: Color = Color::Rgb(196, 205, 197);
const ERROR: Color = Color::Rgb(224, 134, 130);
const WARN: Color = Color::Rgb(214, 183, 96);
const HEADER: Color = Color::Rgb(242, 242, 238);

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
            .saturating_add(state.input.chars().count() as u16),
        composer_inner.y,
    ));
}

fn render_main_pane(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    match state.main_pane {
        MainPaneMode::Transcript => render_transcript(frame, area, state),
        MainPaneMode::View => render_main_view(frame, area, state),
    }
}

fn render_transcript(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    if state.transcript.is_empty() && !state.turn_running && state.session.queued_commands == 0 {
        let lines = build_welcome_lines(state, inner.height);
        let empty = Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(MAIN_BG));
        frame.render_widget(empty, inner);
        return;
    }

    let lines = build_transcript_lines(state);
    let scroll = clamp_scroll(state.transcript_scroll, lines.len(), inner.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(transcript, inner);
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
    let scroll = clamp_scroll(
        state.inspector_scroll,
        state.inspector.len().saturating_add(2).max(1),
        inner.height,
    );
    let mut lines = vec![Line::from(Span::styled(
        title.to_string(),
        Style::default().fg(MUTED),
    ))];
    lines.push(Line::raw(""));
    lines.extend(build_inspector_text(title, &state.inspector).lines);
    let view = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(view, inner);
}

fn render_side_rail(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
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

fn render_status_line(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(FOOTER_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let status = Paragraph::new(format_footer_context(state))
        .style(Style::default().fg(TEXT).bg(FOOTER_BG))
        .wrap(Wrap { trim: true });
    frame.render_widget(status, inner);
}

fn render_composer(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(FOOTER_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    frame.render_widget(
        Paragraph::new(build_composer_line(state)).style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        inner,
    );
}

fn render_approval_band(frame: &mut ratatui::Frame<'_>, area: Rect, approval: &ApprovalPrompt) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_approval_text(approval))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        inner,
    );
}

fn render_command_hint_band(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    command_hint: &SlashCommandHint,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_command_hint_text(command_hint))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        inner,
    );
}

fn composer_inner_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    })
}

fn build_approval_text(approval: &ApprovalPrompt) -> Text<'static> {
    let headline = if approval.tool_name == "bash" {
        format!("Run this command from {}?", approval.origin)
    } else {
        format!("Continue this tool call from {}?", approval.origin)
    };
    let mut lines = vec![Line::from(Span::styled(
        headline,
        Style::default().fg(HEADER),
    ))];
    for line in approval_preview_lines(&approval.arguments_preview) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(SUBTLE)),
            code_span(&line),
        ]));
    }
    if !approval.reasons.is_empty() {
        lines.extend(approval.reasons.iter().take(2).map(|reason| {
            Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(preview_text(reason, 84), Style::default().fg(MUTED)),
            ])
        }));
    }
    lines.push(Line::from(vec![
        Span::styled("y", Style::default().fg(HEADER)),
        Span::styled(" approve", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("n", Style::default().fg(HEADER)),
        Span::styled(" deny", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("esc", Style::default().fg(HEADER)),
        Span::styled(" dismiss", Style::default().fg(MUTED)),
    ]));
    Text::from(lines)
}

fn approval_band_height(approval: &ApprovalPrompt) -> u16 {
    build_approval_text(approval).lines.len().clamp(2, 6) as u16
}

fn build_command_hint_text(command_hint: &SlashCommandHint) -> Text<'static> {
    let mut lines = Vec::new();
    lines.extend(
        command_hint
            .matches
            .iter()
            .filter(|spec| spec.name != command_hint.selected.name)
            .map(|spec| {
                Line::from(Span::styled(
                    format!("/{}", spec.usage),
                    Style::default().fg(MUTED),
                ))
            }),
    );
    lines.push(Line::from(vec![
        Span::styled(
            format!("/{}", command_hint.selected.usage),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default().fg(SUBTLE)),
        Span::styled(command_hint.selected.summary, Style::default().fg(MUTED)),
    ]));
    if let Some(arguments) = command_hint.arguments.as_ref() {
        let mut spans = Vec::new();
        if arguments.provided.is_empty() {
            if let Some(next) = arguments.next {
                spans.push(Span::styled("next ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled(next, Style::default().fg(MUTED)));
            }
        } else {
            for (index, argument) in arguments.provided.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                }
                spans.push(Span::styled(
                    argument.placeholder,
                    Style::default().fg(SUBTLE),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    argument.value.clone(),
                    Style::default().fg(TEXT),
                ));
            }
            if let Some(next) = arguments.next {
                spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled("next ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled(next, Style::default().fg(MUTED)));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }
    let tab_hint = if command_hint.exact {
        if command_hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some()
        {
            "keep typing"
        } else if command_hint.matches.len() > 1 {
            "tab next"
        } else {
            "enter run"
        }
    } else {
        "tab complete"
    };
    let enter_hint = if command_hint.exact {
        "enter run"
    } else if command_hint.matches.len() == 1 && !command_hint.selected.expects_arguments() {
        "enter run"
    } else {
        "enter accept"
    };
    lines.push(Line::from(vec![
        Span::styled(tab_hint, Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("shift+tab previous", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(enter_hint, Style::default().fg(MUTED)),
    ]));

    Text::from(lines)
}

fn command_hint_height(command_hint: &SlashCommandHint) -> u16 {
    build_command_hint_text(command_hint)
        .lines
        .len()
        .clamp(2, 6) as u16
}

fn bottom_layout_constraints(
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

fn approval_preview_lines(lines: &[String]) -> Vec<String> {
    if lines.len() <= 4 {
        return lines.to_vec();
    }

    let mut preview = lines.iter().take(2).cloned().collect::<Vec<_>>();
    preview.push("...".to_string());
    preview.extend(lines.iter().skip(lines.len().saturating_sub(1)).cloned());
    preview
}

fn build_transcript_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if should_render_transcript_context(&state.inspector_title) && !state.inspector.is_empty() {
        lines.push(Line::from(Span::styled(
            state.inspector_title.clone(),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::raw(""));
        lines.extend(build_inspector_text(&state.inspector_title, &state.inspector).lines);
        lines.push(Line::raw(""));
    }

    if !state.transcript.is_empty() {
        for (index, entry) in state.transcript.iter().enumerate() {
            if index > 0 && entry.starts_with("› ") {
                lines.push(turn_divider());
                lines.push(Line::raw(""));
            }
            lines.extend(format_transcript_entry(entry));
            if transcript_entry_needs_spacing(entry) {
                lines.push(Line::raw(""));
            }
        }
    }

    let progress_lines = live_progress_lines(state);
    if !progress_lines.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.extend(progress_lines);
    }

    lines
}

fn build_welcome_lines(state: &TuiState, viewport_height: u16) -> Vec<Line<'static>> {
    let core = vec![
        Line::from(Span::styled(
            "code-agent".to_string(),
            Style::default().fg(HEADER),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{} · {}", state.session.workspace_name, state.session.model),
            Style::default().fg(MUTED),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            "Ask for a change, a fix, or a summary.",
            Style::default().fg(TEXT),
        )),
        Line::from(Span::styled(
            "Type a prompt to begin. Use /help when needed.",
            Style::default().fg(SUBTLE),
        )),
    ];

    let top_padding = usize::from(viewport_height.saturating_sub(core.len() as u16) / 3);
    let mut lines = vec![Line::raw(""); top_padding];
    lines.extend(core);
    lines
}

fn should_render_transcript_context(title: &str) -> bool {
    matches!(title, "Resume" | "Session" | "Task" | "Agent Session")
}

fn turn_divider() -> Line<'static> {
    Line::from(Span::styled("┈".repeat(12), Style::default().fg(SUBTLE)))
}

fn transcript_entry_needs_spacing(entry: &str) -> bool {
    !entry.starts_with("  └ ")
}

fn format_transcript_entry(entry: &str) -> Vec<Line<'static>> {
    let Some((marker, accent, body)) = parse_prefixed_entry(entry) else {
        return vec![Line::from(Span::styled(
            entry.to_string(),
            Style::default().fg(TEXT),
        ))];
    };

    let mut body_lines = body.lines();
    let first_line = body_lines.next().unwrap_or_default();
    let mut rendered = vec![Line::from(vec![
        Span::styled(marker, transcript_marker_style(marker, accent)),
        Span::raw(" "),
        Span::styled(
            first_line.to_string(),
            transcript_body_style(marker, first_line),
        ),
    ])];

    let mut in_code = false;
    for raw_line in body_lines {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            rendered.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(trimmed.to_string(), Style::default().fg(SUBTLE)),
            ]));
            continue;
        }
        rendered.push(render_transcript_body_line(raw_line, in_code));
    }

    rendered
}

fn render_transcript_body_line(raw_line: &str, in_code: bool) -> Line<'static> {
    if raw_line.trim().is_empty() {
        return Line::from(Span::raw(""));
    }
    if let Some(detail) = raw_line.strip_prefix("  └ ") {
        return Line::from(vec![
            Span::styled("  └ ", Style::default().fg(SUBTLE)),
            Span::styled(detail.to_string(), Style::default().fg(MUTED)),
        ]);
    }
    if let Some(detail) = raw_line.strip_prefix("    ") {
        return Line::from(vec![
            Span::raw("    "),
            Span::styled(detail.to_string(), Style::default().fg(MUTED)),
        ]);
    }
    if in_code {
        return Line::from(vec![Span::raw("  "), code_span(raw_line)]);
    }
    if let Some(rest) = raw_line
        .strip_prefix("- ")
        .or_else(|| raw_line.strip_prefix("* "))
    {
        return Line::from(vec![
            Span::raw("  "),
            Span::styled("-", Style::default().fg(MUTED)),
            Span::raw(" "),
            Span::styled(rest.to_string(), Style::default().fg(TEXT)),
        ]);
    }
    Line::from(vec![
        Span::raw("  "),
        Span::styled(raw_line.to_string(), Style::default().fg(TEXT)),
    ])
}

fn parse_prefixed_entry(entry: &str) -> Option<(&'static str, Color, &str)> {
    if let Some(body) = entry.strip_prefix("› ") {
        Some(("›", USER, body))
    } else if let Some(body) = entry.strip_prefix("• ") {
        Some(("•", summary_color(body), body))
    } else if let Some(body) = entry.strip_prefix("✔ ") {
        Some(("✔", ASSISTANT, body))
    } else if let Some(body) = entry.strip_prefix("✗ ") {
        Some(("✗", ERROR, body))
    } else if let Some(body) = entry.strip_prefix("⚠ ") {
        Some(("⚠", WARN, body))
    } else {
        None
    }
}

fn transcript_body_style(marker: &str, line: &str) -> Style {
    match marker {
        "›" => Style::default().fg(TEXT),
        "✔" => Style::default().fg(ASSISTANT),
        "✗" => Style::default().fg(ERROR),
        "⚠" => Style::default().fg(WARN),
        _ => Style::default().fg(summary_color(line)),
    }
}

fn transcript_marker_style(marker: &str, accent: Color) -> Style {
    let style = Style::default().fg(accent);
    if matches!(marker, "›" | "✗") {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn live_progress_lines(state: &TuiState) -> Vec<Line<'static>> {
    if state.turn_running {
        if state.active_tool_label.is_some() {
            if state.session.queued_commands == 0 {
                return Vec::new();
            }
            return vec![Line::from(vec![
                Span::styled("+", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(
                    format!(
                        "{} queued while the current tool runs",
                        state.session.queued_commands
                    ),
                    Style::default().fg(MUTED),
                ),
            ])];
        }
        let elapsed_secs = state
            .turn_started_at
            .map(|started| started.elapsed().as_secs())
            .unwrap_or(0);
        let status = live_progress_summary(state);
        let mut spans = vec![
            Span::styled(
                progress_marker(state),
                Style::default()
                    .fg(status_color(&state.status))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                preview_text(&status, 56),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ];
        if state.session.queued_commands > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled(
                format!("{} queued", state.session.queued_commands),
                Style::default().fg(MUTED),
            ));
        }
        spans.push(Span::styled(
            format!(" ({}s · esc to interrupt)", elapsed_secs),
            Style::default().fg(MUTED),
        ));
        vec![Line::from(spans)]
    } else {
        vec![Line::from(vec![
            Span::styled("+", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(
                format!("{} queued command(s)", state.session.queued_commands),
                Style::default().fg(MUTED),
            ),
        ])]
    }
}

fn live_progress_summary(state: &TuiState) -> String {
    match state.status.as_str() {
        "Waiting for approval" => "Waiting for approval".to_string(),
        status if !status.is_empty() => status.to_string(),
        _ => "Working".to_string(),
    }
}

fn build_composer_line(state: &TuiState) -> Line<'static> {
    let mut spans = vec![
        Span::styled("›", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ];
    if state.input.is_empty() {
        spans.push(Span::styled(
            "Type a prompt or /help",
            Style::default().fg(SUBTLE),
        ));
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
            Style::default().fg(USER).add_modifier(Modifier::BOLD),
        ));
        if let Some(tail) = tail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(tail.to_string(), Style::default().fg(TEXT)));
        }
    } else {
        spans.push(Span::styled(state.input.clone(), Style::default().fg(TEXT)));
    }

    Line::from(spans)
}

fn should_render_side_rail(state: &TuiState, area: Rect) -> bool {
    state.main_pane == MainPaneMode::Transcript
        && area.width >= 128
        && (lsp_side_rail_available(state) || !state.todo_items.is_empty())
}

fn side_rail_width(total_width: u16) -> u16 {
    total_width.saturating_mul(22) / 100
}

fn lsp_side_rail_available(state: &TuiState) -> bool {
    state.session.tool_names.iter().any(|tool| {
        matches!(
            tool.as_str(),
            "code_symbol_search" | "code_document_symbols" | "code_definitions" | "code_references"
        )
    })
}

fn build_side_rail_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if lsp_side_rail_available(state) {
        lines.push(section_title_line("LSP"));
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
                    .map(|diagnostic| (preview_text(diagnostic, 40), USER)),
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
        lines.push(section_title_line("TODO"));
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
        lines.push(section_title_line("Context"));
        lines.push(Line::from(Span::styled(
            "No live side context.",
            Style::default().fg(SUBTLE),
        )));
    }

    lines
}

fn section_title_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), Style::default().fg(MUTED)))
}

fn bullet_line(body: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled("•", Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(body.to_string(), Style::default().fg(MUTED)),
    ])
}

fn status_line(body: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(body.to_string(), Style::default().fg(color)))
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

fn code_span(line: &str) -> Span<'static> {
    let trimmed = line.trim_start();
    let style = if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Style::default().fg(ASSISTANT)
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Style::default().fg(ERROR)
    } else if trimmed.starts_with("@@") {
        Style::default().fg(USER)
    } else {
        Style::default().fg(TEXT)
    };
    Span::styled(line.to_string(), style)
}

fn progress_marker(state: &TuiState) -> &'static str {
    if state.turn_running {
        "•"
    } else if state.session.queued_commands > 0 {
        "+"
    } else {
        "·"
    }
}

fn format_footer_context(state: &TuiState) -> Line<'static> {
    let mut spans = vec![Span::styled(
        state.session.workspace_name.clone(),
        Style::default().fg(TEXT),
    )];
    spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
    spans.push(Span::styled(
        state.session.model.clone(),
        Style::default().fg(MUTED),
    ));

    if state.session.git.available {
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled(
            state.session.git.branch.clone(),
            Style::default().fg(MUTED),
        ));
    }

    spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
    spans.push(Span::styled(
        preview_id(&state.session.active_session_ref),
        Style::default().fg(MUTED),
    ));

    Line::from(spans)
}

fn build_key_value_text(lines: &[String]) -> Text<'static> {
    let mut rendered = Vec::new();
    for line in lines {
        if let Some(title) = line.strip_prefix("## ") {
            if !rendered.is_empty() {
                rendered.push(Line::raw(""));
            }
            rendered.push(Line::from(Span::styled(
                title.to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if is_shell_summary_line(line) {
            rendered.extend(render_shell_summary_line(line));
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            rendered.push(Line::from(vec![
                Span::styled(
                    format!("{key}:"),
                    Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    value.trim().to_string(),
                    value_style(key.trim(), value.trim()),
                ),
            ]));
        } else if let Some((marker, accent, body)) = parse_prefixed_entry(line) {
            rendered.push(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(body.to_string(), transcript_body_style(marker, body)),
            ]));
        } else if let Some(detail) = line.strip_prefix("  └ ") {
            rendered.push(Line::from(vec![
                Span::styled("  └ ", Style::default().fg(SUBTLE)),
                Span::styled(detail.to_string(), Style::default().fg(MUTED)),
            ]));
        } else if let Some(detail) = line.strip_prefix("    ") {
            rendered.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(detail.to_string(), Style::default().fg(MUTED)),
            ]));
        } else if let Some(rest) = line.strip_prefix("  ") {
            rendered.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(rest.to_string(), Style::default().fg(TEXT)),
            ]));
        } else if line.starts_with('/') {
            rendered.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            )));
        } else {
            rendered.push(Line::from(Span::styled(
                line.to_string(),
                plain_text_style(line),
            )));
        }
    }
    Text::from(rendered)
}

fn is_shell_summary_line(line: &str) -> bool {
    parse_prefixed_entry(line).is_some()
        || line.starts_with("  └ ")
        || line.starts_with("    ")
        || line.starts_with("- ")
        || line.starts_with("* ")
}

fn render_shell_summary_line(line: &str) -> Vec<Line<'static>> {
    if parse_prefixed_entry(line).is_some() {
        format_transcript_entry(line)
    } else {
        vec![render_transcript_body_line(line, false)]
    }
}

fn build_inspector_text(title: &str, lines: &[String]) -> Text<'static> {
    if is_collection_inspector(title) {
        build_collection_text(title, lines)
    } else {
        build_key_value_text(lines)
    }
}

fn build_collection_text(title: &str, lines: &[String]) -> Text<'static> {
    let accent = inspector_accent(title);
    let mut rendered = Vec::new();
    for line in lines {
        if let Some(section) = line.strip_prefix("## ") {
            if !rendered.is_empty() {
                rendered.push(Line::raw(""));
            }
            rendered.push(Line::from(Span::styled(
                section.to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if line.starts_with("No ") || line.starts_with("no ") {
            rendered.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(SUBTLE),
            )));
            continue;
        }
        let (primary, secondary) = split_list_entry(line);
        rendered.push(collection_line(primary, secondary, accent));
    }
    Text::from(rendered)
}

fn collection_line(primary: &str, secondary: Option<&str>, accent: Color) -> Line<'static> {
    let mut spans = vec![
        Span::styled("-", Style::default().fg(MUTED)),
        Span::raw(" "),
        Span::styled(
            primary.to_string(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(secondary) = secondary
        && !secondary.trim().is_empty()
    {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            secondary.trim().to_string(),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn split_list_entry(line: &str) -> (&str, Option<&str>) {
    if let Some((primary, secondary)) = line.split_once("  ") {
        (primary.trim(), Some(secondary))
    } else {
        (line.trim(), None)
    }
}

fn is_collection_inspector(title: &str) -> bool {
    matches!(
        title,
        "Command Palette"
            | "Tool Catalog"
            | "Skill Catalog"
            | "MCP"
            | "Prompts"
            | "Resources"
            | "Live Tasks"
            | "Agent Sessions"
            | "Tasks"
            | "Sessions"
            | "Session Search"
    )
}

fn inspector_accent(title: &str) -> Color {
    match title {
        "Live Tasks" => USER,
        "Sessions" | "Session Search" | "Agent Sessions" | "Tasks" => ASSISTANT,
        "Command Palette" => HEADER,
        _ => BORDER_ACTIVE,
    }
}

fn value_style(key: &str, value: &str) -> Style {
    if key.contains("warning") {
        Style::default().fg(WARN)
    } else if key.contains("status") {
        if value.contains("completed") || value.contains("ready") {
            Style::default().fg(ASSISTANT)
        } else if value.contains("cancel") || value.contains("failed") {
            Style::default().fg(ERROR)
        } else {
            Style::default().fg(WARN)
        }
    } else if key.contains("action") {
        if value.contains("sent")
            || value.contains("cancelled")
            || value.contains("reattached")
            || value.contains("started")
        {
            Style::default().fg(ASSISTANT)
        } else {
            Style::default().fg(WARN)
        }
    } else if key.contains("sandbox") {
        Style::default().fg(USER)
    } else if key.contains("dirty") {
        if value.contains("modified 0")
            && value.contains("untracked 0")
            && value.contains("staged 0")
        {
            Style::default().fg(ASSISTANT)
        } else {
            Style::default().fg(WARN)
        }
    } else if key.contains("queue") {
        if value.starts_with('0') {
            Style::default().fg(ASSISTANT)
        } else {
            Style::default().fg(WARN)
        }
    } else if key.contains("active ref")
        || key.contains("runtime id")
        || key.contains("session ref")
        || key.contains("agent id")
        || key.contains("task id")
    {
        Style::default().fg(USER)
    } else if key.contains("summary") {
        Style::default().fg(HEADER)
    } else {
        Style::default().fg(TEXT)
    }
}

fn plain_text_style(line: &str) -> Style {
    if line.starts_with("Use /") {
        Style::default().fg(MUTED)
    } else if line.starts_with("warning:") {
        Style::default().fg(WARN)
    } else if line.starts_with("diagnostic:") {
        Style::default().fg(USER)
    } else if line.starts_with("No ") || line.starts_with("no ") {
        Style::default().fg(SUBTLE)
    } else {
        Style::default().fg(TEXT)
    }
}

fn summary_color(line: &str) -> Color {
    let lower = line.to_ascii_lowercase();
    if lower.contains("failed")
        || lower.contains("error")
        || lower.contains("denied")
        || lower.contains("cancelled")
    {
        ERROR
    } else if lower.contains("approved")
        || lower.contains("complete")
        || lower.contains("loaded")
        || lower.contains("ready")
        || lower.contains("called")
    {
        ASSISTANT
    } else if lower.contains("waiting")
        || lower.contains("blocked")
        || lower.contains("running")
        || lower.contains("applying")
    {
        WARN
    } else {
        TEXT
    }
}

fn status_color(status: &str) -> Color {
    let lower = status.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("failed") || lower.contains("denied") {
        ERROR
    } else if lower.contains("approval") || lower.contains("running") || lower.contains("waiting") {
        WARN
    } else if lower.contains("ready") || lower.contains("complete") || lower.contains("approved") {
        ASSISTANT
    } else {
        USER
    }
}

fn clamp_scroll(requested: u16, content_lines: usize, viewport_height: u16) -> u16 {
    let viewport = usize::from(viewport_height.max(1));
    let max_scroll = content_lines.saturating_sub(viewport);
    if requested == u16::MAX {
        max_scroll.min(u16::MAX as usize) as u16
    } else {
        usize::from(requested)
            .min(max_scroll)
            .min(u16::MAX as usize) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::{
        approval_preview_lines, build_approval_text, build_collection_text,
        build_command_hint_text, build_key_value_text, build_side_rail_lines,
        build_transcript_lines, build_welcome_lines, format_footer_context,
        should_render_side_rail,
    };
    use crate::frontend::tui::approval::ApprovalPrompt;
    use crate::frontend::tui::commands::{
        SlashCommandArgumentHint, SlashCommandArgumentValue, SlashCommandHint, SlashCommandSpec,
    };
    use crate::frontend::tui::state::{MainPaneMode, TodoEntry, TuiState};
    use ratatui::layout::Rect;

    #[test]
    fn key_value_text_renders_section_headers_without_treating_them_as_pairs() {
        let rendered = build_key_value_text(&[
            "## Session".to_string(),
            "session ref: abc123".to_string(),
            "/sessions [query]".to_string(),
        ]);
        let lines = rendered.lines;
        assert_eq!(lines[0].spans[0].content.as_ref(), "Session");
        assert_eq!(lines[1].spans[0].content.as_ref(), "session ref:");
        assert_eq!(lines[2].spans[0].content.as_ref(), "/sessions [query]");
    }

    #[test]
    fn key_value_text_preserves_prefixed_summary_blocks() {
        let rendered = build_key_value_text(&[
            "✔ Exported transcript text".to_string(),
            "  └ session-1".to_string(),
            "    Wrote 4 items to /workspace/out.txt".to_string(),
        ]);
        let lines = rendered.lines;
        assert_eq!(lines[0].spans[0].content.as_ref(), "✔");
        assert_eq!(
            lines[0].spans[2].content.as_ref(),
            "Exported transcript text"
        );
        assert_eq!(lines[1].spans[0].content.as_ref(), "  └ ");
        assert_eq!(
            lines[2].spans[1].content.as_ref(),
            "Wrote 4 items to /workspace/out.txt"
        );
    }

    #[test]
    fn key_value_text_reuses_transcript_rendering_for_shell_summary_lines() {
        let rendered = build_key_value_text(&[
            "• Reattached session".to_string(),
            "  └ session session-1".to_string(),
        ]);

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "•");
        assert_eq!(
            rendered.lines[0].spans[2].content.as_ref(),
            "Reattached session"
        );
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "  └ ");
        assert_eq!(
            rendered.lines[1].spans[1].content.as_ref(),
            "session session-1"
        );
    }

    #[test]
    fn transcript_entries_render_with_codex_like_prefixes() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• hello world".to_string()];

        let lines = build_transcript_lines(&state);

        assert_eq!(lines[0].spans[0].content.as_ref(), "•");
        assert_eq!(lines[0].spans[2].content.as_ref(), "hello world");
    }

    #[test]
    fn transcript_inserts_turn_dividers_between_user_turns() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            "› first".to_string(),
            "• reply".to_string(),
            "› second".to_string(),
        ];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.contains("┈"))
        }));
    }

    #[test]
    fn transcript_renders_resume_summary_above_history() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            inspector_title: "Resume".to_string(),
            inspector: vec![
                "✔ Reattached session".to_string(),
                "  └ session session-1".to_string(),
            ],
            ..TuiState::default()
        };
        state.transcript = vec!["• done".to_string()];

        let rendered = build_transcript_lines(&state);

        assert_eq!(rendered[0].spans[0].content.as_ref(), "Resume");
        assert_eq!(rendered[2].spans[0].content.as_ref(), "✔");
        assert_eq!(rendered[2].spans[2].content.as_ref(), "Reattached session");
    }

    #[test]
    fn welcome_lines_keep_the_start_screen_sparse() {
        let mut state = TuiState::default();
        state.session.workspace_name = "nanoclaw".to_string();
        state.session.model = "gpt-5.4".to_string();

        let lines = build_welcome_lines(&state, 20);

        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("code-agent"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("Ask for a change, a fix, or a summary.")
            })
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Type a prompt to begin."))
        }));
    }

    #[test]
    fn collection_text_promotes_primary_column_for_catalog_rows() {
        let rendered = build_collection_text(
            "Sessions",
            &[
                "## Sessions".to_string(),
                "sess_123  msgs=12 ev=40  no prompt yet".to_string(),
            ],
        );

        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "-");
        assert_eq!(rendered.lines[1].spans[2].content.as_ref(), "sess_123");
        assert_eq!(
            rendered.lines[1].spans[4].content.as_ref(),
            "msgs=12 ev=40  no prompt yet"
        );
    }

    #[test]
    fn transcript_renders_compact_live_progress_line() {
        let state = TuiState {
            main_pane: MainPaneMode::Transcript,
            turn_running: true,
            status: "Working (2)".to_string(),
            ..TuiState::default()
        };

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Working (2)"))
        }));
        assert!(!rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("$ cargo test"))
        }));
    }

    #[test]
    fn transcript_hides_progress_line_while_tool_cell_is_active() {
        let state = TuiState {
            main_pane: MainPaneMode::Transcript,
            turn_running: true,
            status: "Working".to_string(),
            active_tool_label: Some("bash".to_string()),
            transcript: vec!["• Running bash\n  └ $ cargo test".to_string()],
            ..TuiState::default()
        };

        let rendered = build_transcript_lines(&state);

        let running_count = rendered
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.as_ref().contains("Running bash"))
            })
            .count();
        assert_eq!(running_count, 1);
    }

    #[test]
    fn side_rail_surfaces_todos_and_lsp_summary() {
        let mut state = TuiState::default();
        state.main_pane = MainPaneMode::Transcript;
        state.session.tool_names = vec!["code_symbol_search".to_string()];
        state.session.startup_diagnostics.diagnostics = vec!["rust-analyzer attached".to_string()];
        state.todo_items = vec![
            TodoEntry {
                id: "t1".to_string(),
                content: "Refine transcript".to_string(),
                status: "in_progress".to_string(),
            },
            TodoEntry {
                id: "t2".to_string(),
                content: "Tighten command palette".to_string(),
                status: "pending".to_string(),
            },
            TodoEntry {
                id: "t3".to_string(),
                content: "Finish diagnostics".to_string(),
                status: "completed".to_string(),
            },
        ];

        let lines = build_side_rail_lines(&state);

        assert_eq!(lines[0].spans[0].content.as_ref(), "LSP");
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("0 warnings · 1 diagnostics"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("rust-analyzer attached"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("1 active · 1 pending · 1 done")
            })
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Refine transcript"))
        }));
    }

    #[test]
    fn side_rail_stays_hidden_for_non_transcript_views() {
        let mut state = TuiState::default();
        state.main_pane = MainPaneMode::View;
        state.session.tool_names = vec!["code_symbol_search".to_string()];

        assert!(!should_render_side_rail(
            &state,
            Rect {
                x: 0,
                y: 0,
                width: 140,
                height: 20,
            }
        ));
    }

    #[test]
    fn approval_band_uses_inline_prompt_language() {
        let text = build_approval_text(&ApprovalPrompt {
            tool_name: "bash".to_string(),
            origin: "local".to_string(),
            reasons: vec!["sandbox policy requires approval".to_string()],
            arguments_preview: vec!["$ cargo test".to_string()],
        });

        assert_eq!(
            text.lines[0].spans[0].content.as_ref(),
            "Run this command from local?"
        );
        assert!(text.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("y"))
        }));
        assert!(text.lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("sandbox policy requires approval")
            })
        }));
    }

    #[test]
    fn approval_preview_lines_collapse_long_argument_blocks() {
        let lines = approval_preview_lines(&[
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
            "five".to_string(),
        ]);

        assert_eq!(lines, vec!["one", "two", "...", "five"]);
    }

    #[test]
    fn command_hint_text_surfaces_selected_usage_and_matches() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "History",
                name: "sessions",
                usage: "sessions [query]",
                summary: "browse persisted sessions",
            },
            matches: vec![
                SlashCommandSpec {
                    section: "History",
                    name: "sessions",
                    usage: "sessions [query]",
                    summary: "browse persisted sessions",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "session",
                    usage: "session <session-ref>",
                    summary: "open persisted session",
                },
            ],
            arguments: None,
            exact: false,
        });

        assert_eq!(
            rendered.lines[0].spans[0].content.as_ref(),
            "/session <session-ref>"
        );
        assert_eq!(
            rendered.lines[1].spans[0].content.as_ref(),
            "/sessions [query]"
        );
        assert_eq!(
            rendered.lines[1].spans[2].content.as_ref(),
            "browse persisted sessions"
        );
        assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "tab complete");
        assert_eq!(rendered.lines[2].spans[4].content.as_ref(), "enter accept");
    }

    #[test]
    fn command_hint_text_surfaces_argument_progress() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "Agents",
                name: "spawn_task",
                usage: "spawn_task <role> <prompt>",
                summary: "launch child agent",
            },
            matches: vec![SlashCommandSpec {
                section: "Agents",
                name: "spawn_task",
                usage: "spawn_task <role> <prompt>",
                summary: "launch child agent",
            }],
            arguments: Some(SlashCommandArgumentHint {
                provided: vec![SlashCommandArgumentValue {
                    placeholder: "<role>",
                    value: "reviewer".to_string(),
                }],
                next: Some("<prompt>"),
            }),
            exact: true,
        });

        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "<role>");
        assert_eq!(rendered.lines[1].spans[2].content.as_ref(), "reviewer");
        assert!(
            rendered.lines[1]
                .spans
                .iter()
                .any(|span| span.content.as_ref().contains("<prompt>"))
        );
        assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "keep typing");
        assert_eq!(rendered.lines[2].spans[4].content.as_ref(), "enter run");
    }

    #[test]
    fn footer_context_prefers_workspace_and_session_ref() {
        let mut state = TuiState::default();
        state.session.workspace_name = "nanoclaw".to_string();
        state.session.model = "gpt-5.4".to_string();
        state.session.active_session_ref = "session_123456".to_string();

        let footer = format_footer_context(&state);

        assert_eq!(footer.spans[0].content.as_ref(), "nanoclaw");
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("gpt-5.4") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("session_") })
        );
    }
}
