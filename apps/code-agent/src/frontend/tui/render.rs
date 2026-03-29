use super::approval::ApprovalPrompt;
use super::state::{MainPaneMode, TuiState, preview_text};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

const BG: Color = Color::Rgb(12, 13, 14);
const MAIN_BG: Color = Color::Rgb(14, 15, 17);
const FOOTER_BG: Color = Color::Rgb(16, 17, 19);
const FOOTER_ALT_BG: Color = Color::Rgb(19, 21, 23);
const OVERLAY_BG: Color = Color::Rgb(22, 24, 27);
const BORDER_ACTIVE: Color = Color::Rgb(142, 150, 132);
const TEXT: Color = Color::Rgb(229, 230, 226);
const MUTED: Color = Color::Rgb(149, 150, 146);
const SUBTLE: Color = Color::Rgb(106, 108, 105);
const USER: Color = Color::Rgb(207, 193, 161);
const ASSISTANT: Color = Color::Rgb(171, 192, 150);
const ERROR: Color = Color::Rgb(241, 133, 133);
const WARN: Color = Color::Rgb(235, 196, 94);
const HEADER: Color = Color::Rgb(236, 238, 232);

pub(crate) fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    approval: Option<&ApprovalPrompt>,
) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_main_pane(frame, vertical[0], state);
    render_composer(frame, vertical[1], state);
    render_status_line(frame, vertical[2], state);

    if let Some(approval) = approval {
        render_approval_overlay(frame, area, approval);
    }

    let composer_inner = composer_inner_area(vertical[1]);
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
    let lines = build_transcript_lines(state);
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
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
        Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::raw(""));
    lines.extend(build_inspector_text(title, &state.inspector).lines);
    let view = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(view, inner);
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
    frame.render_widget(
        Block::default().style(Style::default().bg(FOOTER_ALT_BG)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let input_line = if state.input.is_empty() {
        Line::from(vec![
            Span::styled("›", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled("Ask Code Agent to do anything", Style::default().fg(SUBTLE)),
        ])
    } else {
        Line::from(vec![
            Span::styled("›", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(state.input.clone(), Style::default().fg(TEXT)),
        ])
    };
    frame.render_widget(
        Paragraph::new(input_line).style(Style::default().fg(TEXT).bg(FOOTER_ALT_BG)),
        inner,
    );
}

fn render_approval_overlay(frame: &mut ratatui::Frame<'_>, area: Rect, approval: &ApprovalPrompt) {
    let popup = approval_sheet_rect(area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(build_approval_text(approval))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(OVERLAY_BG)),
        popup,
    );
}

fn composer_inner_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    })
}

fn approval_sheet_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(10).min(96).max(48);
    let height = area.height.min(14);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height.saturating_add(3)));
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn build_approval_text(approval: &ApprovalPrompt) -> Text<'static> {
    let question = if approval.tool_name == "bash" {
        "  Would you like to run the following command?"
    } else {
        "  Would you like Code Agent to continue with this tool call?"
    };
    let mut lines = vec![
        Line::from(Span::styled(
            question.to_string(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];

    if !approval.reasons.is_empty() {
        let mut reasons = approval.reasons.iter();
        if let Some(reason) = reasons.next() {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("  Reason: ", Style::default().fg(MUTED)),
                Span::styled(reason.clone(), Style::default().fg(TEXT)),
            ]));
        }
        for reason in reasons {
            lines.push(Line::from(vec![
                Span::raw("          "),
                Span::styled(reason.clone(), Style::default().fg(TEXT)),
            ]));
        }
    }

    lines.push(Line::raw(""));
    for line in &approval.arguments_preview {
        lines.push(Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(TEXT),
        )));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            "› 1. Yes, proceed",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (y)", Style::default().fg(MUTED)),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  2. No, deny"),
        Span::styled(" (n)", Style::default().fg(MUTED)),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  3. Dismiss and deny"),
        Span::styled(" (esc)", Style::default().fg(MUTED)),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Press y to approve, n to deny, or esc to dismiss",
        Style::default().fg(MUTED),
    )));
    Text::from(lines)
}

fn build_transcript_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if should_render_transcript_context(&state.inspector_title) && !state.inspector.is_empty() {
        lines.push(Line::from(Span::styled(
            state.inspector_title.clone(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
        lines.extend(build_inspector_text(&state.inspector_title, &state.inspector).lines);
        lines.push(Line::raw(""));
    }

    if state.transcript.is_empty() {
        lines.push(Line::from(Span::styled(
            "Ready.",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Type a prompt below or open /help.",
            Style::default().fg(SUBTLE),
        )));
    } else {
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

    if state.turn_running || state.session.queued_commands > 0 {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(live_progress_line(state));
    }

    lines
}

fn should_render_transcript_context(title: &str) -> bool {
    matches!(title, "Resume" | "Session" | "Task" | "Agent Session")
}

fn turn_divider() -> Line<'static> {
    Line::from(Span::styled("─".repeat(16), Style::default().fg(SUBTLE)))
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
        Span::styled(
            marker,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
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
            Span::styled("-", Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
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

fn live_progress_line(state: &TuiState) -> Line<'static> {
    if state.turn_running {
        let elapsed_secs = state
            .turn_started_at
            .map(|started| started.elapsed().as_secs())
            .unwrap_or(0);
        let status = live_progress_summary(state);
        Line::from(vec![
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
            Span::styled(
                format!(" ({}s • esc to interrupt)", elapsed_secs),
                Style::default().fg(MUTED),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("+", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(
                format!("{} queued command(s)", state.session.queued_commands),
                Style::default().fg(MUTED),
            ),
        ])
    }
}

fn live_progress_summary(state: &TuiState) -> String {
    match (state.status.as_str(), state.active_tool_label.as_deref()) {
        ("Waiting for approval", Some(tool_name)) => {
            format!("Waiting for approval to run {tool_name}")
        }
        ("Waiting for approval", None) => "Waiting for approval".to_string(),
        _ => "Working".to_string(),
    }
}

fn code_span(line: &str) -> Span<'static> {
    let trimmed = line.trim_start();
    let style = if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Style::default().fg(ASSISTANT).bg(FOOTER_BG)
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Style::default().fg(ERROR).bg(FOOTER_BG)
    } else if trimmed.starts_with("@@") {
        Style::default().fg(USER).bg(FOOTER_BG)
    } else {
        Style::default().fg(TEXT).bg(FOOTER_BG)
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
        state.session.model.clone(),
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
    spans.push(Span::styled(
        state.session.provider_label.clone(),
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
        preview_text(&state.session.workspace_root.display().to_string(), 44),
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
    use super::{build_collection_text, build_key_value_text, build_transcript_lines};
    use crate::frontend::tui::state::{MainPaneMode, TuiState};

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
                .is_some_and(|span| span.content.contains("─"))
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
}
