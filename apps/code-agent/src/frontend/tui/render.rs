use super::approval::ApprovalPrompt;
use super::state::{MainPaneMode, PaneFocus, TuiState, preview_text};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const BG: Color = Color::Rgb(9, 13, 20);
const TOP_BG: Color = Color::Rgb(9, 13, 20);
const PANEL_BG: Color = Color::Rgb(10, 14, 21);
const PANEL_ALT_BG: Color = Color::Rgb(12, 17, 26);
const OVERLAY_BG: Color = Color::Rgb(20, 28, 42);
const BORDER: Color = Color::Rgb(37, 48, 66);
const BORDER_ACTIVE: Color = Color::Rgb(96, 180, 255);
const TEXT: Color = Color::Rgb(228, 233, 242);
const MUTED: Color = Color::Rgb(139, 152, 172);
const SUBTLE: Color = Color::Rgb(97, 111, 133);
const USER: Color = Color::Rgb(111, 203, 255);
const ASSISTANT: Color = Color::Rgb(151, 223, 181);
const ERROR: Color = Color::Rgb(255, 133, 133);
const WARN: Color = Color::Rgb(255, 196, 92);
const HEADER: Color = Color::Rgb(235, 240, 247);

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
            Constraint::Length(1),
            Constraint::Min(12),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, vertical[0], state);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(79), Constraint::Percentage(21)])
        .split(vertical[1]);

    render_main_pane(frame, body[0], state);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(5)])
        .split(body[1]);

    render_side_panel(frame, right[0], state);
    render_activity(frame, right[1], state);
    render_composer(frame, vertical[2], state);

    if let Some(approval) = approval {
        render_approval_overlay(frame, area, approval);
    }

    let composer_inner = composer_inner_area(vertical[2]);
    let prefix_width = 2_u16;
    frame.set_cursor_position(Position::new(
        composer_inner
            .x
            .saturating_add(prefix_width)
            .saturating_add(state.input.chars().count() as u16),
        composer_inner.y,
    ));
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let mut spans = vec![
        Span::styled(
            preview_text(&state.session.workspace_name, 18),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled("model ", Style::default().fg(MUTED)),
        Span::styled(
            preview_text(
                &format!("{} / {}", state.session.provider_label, state.session.model),
                24,
            ),
            Style::default().fg(TEXT),
        ),
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled("session ", Style::default().fg(MUTED)),
        Span::styled(
            preview_text(&state.session.active_session_ref, 10),
            Style::default().fg(USER),
        ),
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled("status ", Style::default().fg(MUTED)),
        Span::styled(
            preview_text(&state.status, 24),
            Style::default()
                .fg(status_color(&state.status))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if state.session.queued_commands > 0 {
        spans.extend([
            Span::styled("  ", Style::default().fg(MUTED)),
            Span::styled("queue ", Style::default().fg(MUTED)),
            Span::styled(
                state.session.queued_commands.to_string(),
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
        ]);
    }
    let status = Paragraph::new(Line::from(spans)).style(Style::default().fg(TEXT).bg(TOP_BG));
    frame.render_widget(status, area);
}

fn render_main_pane(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    match state.main_pane {
        MainPaneMode::Transcript => render_transcript(frame, area, state),
        MainPaneMode::View => render_main_view(frame, area, state),
    }
}

fn render_transcript(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let lines = build_transcript_lines(&state.transcript);
    let block = pane_block(
        "Conversation",
        state.focus == PaneFocus::Conversation,
        BORDER,
    );
    let scroll = clamp_scroll(
        state.transcript_scroll,
        lines.len(),
        block.inner(area).height,
    );
    let transcript = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_BG));
    frame.render_widget(transcript, area);
}

fn render_main_view(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let title = if state.inspector_title.is_empty() {
        "View"
    } else {
        state.inspector_title.as_str()
    };
    let item_count = inspector_collection_count(&state.inspector);
    let block_title = if is_collection_inspector(title) && item_count > 0 {
        format!("{title} · {item_count}")
    } else {
        title.to_string()
    };
    let block = pane_block(block_title, state.focus == PaneFocus::Conversation, BORDER);
    let scroll = clamp_scroll(
        state.inspector_scroll,
        state.inspector.len().max(1),
        block.inner(area).height,
    );
    let view = Paragraph::new(build_inspector_text(title, &state.inspector))
        .block(block)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_BG));
    frame.render_widget(view, area);
}

fn session_lines(state: &TuiState) -> Vec<String> {
    let mut lines = vec![
        "## Workspace".to_string(),
        format!("name: {}", state.session.workspace_name),
        format!(
            "path: {}",
            preview_text(&state.session.workspace_root.display().to_string(), 52)
        ),
        "## Session".to_string(),
        format!(
            "active ref: {}",
            preview_text(&state.session.active_session_ref, 28)
        ),
        format!(
            "agent session id: {}",
            preview_text(&state.session.root_agent_session_id, 28)
        ),
        format!("persisted sessions: {}", state.session.stored_session_count),
        "## Runtime".to_string(),
        format!(
            "primary lane: {} / {}",
            state.session.provider_label, state.session.model
        ),
        format!("queue: {} pending", state.session.queued_commands),
        format!(
            "resources: tools {}  skills {}",
            state.session.tool_names.len(),
            if state.session.skill_names.is_empty() {
                "none".to_string()
            } else {
                state.session.skill_names.len().to_string()
            }
        ),
        format!(
            "plugins: {} / {}",
            state.session.startup_diagnostics.enabled_plugin_count,
            state.session.startup_diagnostics.total_plugin_count
        ),
        format!(
            "mcp: {} servers",
            state.session.startup_diagnostics.mcp_servers.len()
        ),
        "## Store".to_string(),
        format!("store: {}", preview_text(&state.session.store_label, 20),),
        format!(
            "sandbox: {}",
            preview_text(&state.session.sandbox_summary, 28)
        ),
    ];
    if let Some(warning) = &state.session.store_warning {
        lines.push(format!("warning: {}", preview_text(warning, 36)));
    }
    lines.extend([
        "## Tokens".to_string(),
        session_context_line(state),
        session_last_token_line(state),
        session_total_token_line(state),
        "## Git".to_string(),
        format!("branch: {}", state.session.git.branch_label()),
        format!("dirty: {}", state.session.git.dirty_label()),
    ]);
    lines
}

fn session_context_line(state: &TuiState) -> String {
    state
        .session
        .token_ledger
        .context_window
        .map(|usage| format!("context: {} / {}", usage.used_tokens, usage.max_tokens))
        .unwrap_or_else(|| "context: unknown".to_string())
}

fn session_last_token_line(state: &TuiState) -> String {
    if let Some(last_usage) = state.session.token_ledger.last_usage {
        format_token_usage_line("last", last_usage)
    } else {
        "last: none yet".to_string()
    }
}

fn session_total_token_line(state: &TuiState) -> String {
    if state.session.token_ledger.cumulative_usage.is_zero() {
        "total: none yet".to_string()
    } else {
        format_token_usage_line("total", state.session.token_ledger.cumulative_usage)
    }
}

fn format_token_usage_line(label: &str, usage: agent::types::TokenUsage) -> String {
    format!(
        "{label}: in {}  out {}  prefill {}  decode {}  cache {}",
        usage.input_tokens,
        usage.output_tokens,
        usage.prefill_tokens,
        usage.decode_tokens,
        usage.cache_read_tokens,
    )
}

fn render_side_panel(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    match state.main_pane {
        MainPaneMode::Transcript => render_inspector(frame, area, state),
        MainPaneMode::View => render_side_info(frame, area, state),
    }
}

fn render_inspector(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let title = if state.inspector_title.is_empty() {
        "Info"
    } else {
        state.inspector_title.as_str()
    };
    let item_count = inspector_collection_count(&state.inspector);
    let block_title = if is_collection_inspector(title) && item_count > 0 {
        format!("{title} · {item_count}")
    } else {
        title.to_string()
    };
    let block = pane_block(block_title, state.focus == PaneFocus::Inspector, BORDER);
    let scroll = clamp_scroll(
        state.inspector_scroll,
        state.inspector.len().max(1),
        block.inner(area).height,
    );
    let inspector = Paragraph::new(build_inspector_text(title, &state.inspector))
        .block(block)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_ALT_BG));
    frame.render_widget(inspector, area);
}

fn render_side_info(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let info = Paragraph::new(build_key_value_text(&side_info_lines(state)))
        .block(pane_block(
            "Info",
            state.focus == PaneFocus::Inspector,
            BORDER,
        ))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_ALT_BG));
    frame.render_widget(info, area);
}

fn render_activity(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let lines = if state.activity.is_empty() {
        Text::from(vec![Line::from(Span::styled(
            "No log yet.",
            Style::default().fg(SUBTLE),
        ))])
    } else {
        let mut lines = Vec::new();
        for item in state.activity.iter().rev().take(8) {
            lines.push(Line::from(vec![
                Span::styled(
                    activity_marker(item),
                    Style::default()
                        .fg(activity_color(item))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(item.clone(), Style::default().fg(activity_color(item))),
            ]));
        }
        Text::from(lines)
    };
    let block = pane_block("Log", state.focus == PaneFocus::Activity, BORDER);
    let scroll = clamp_scroll(
        state.activity_scroll,
        state.activity.len().max(1),
        block.inner(area).height,
    );
    let activity = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(TEXT).bg(PANEL_BG));
    frame.render_widget(activity, area);
}

fn render_composer(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let block = composer_block();
    let inner = block.inner(area).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(18), Constraint::Length(18)])
        .split(inner);
    frame.render_widget(block, area);

    let input_text = if state.input.is_empty() {
        Line::from(vec![
            Span::styled(">", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled("type a prompt or /help", Style::default().fg(SUBTLE)),
        ])
    } else {
        Line::from(vec![
            Span::styled(">", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(state.input.clone(), Style::default().fg(TEXT)),
        ])
    };
    let input_field = Paragraph::new(Text::from(input_text))
        .style(Style::default().fg(TEXT).bg(TOP_BG))
        .alignment(Alignment::Left);
    frame.render_widget(Clear, columns[0]);
    frame.render_widget(input_field, columns[0]);

    let mode = Paragraph::new(Line::from(vec![
        Span::styled(
            if state.input.trim_start().starts_with('/') {
                "command"
            } else if state.turn_running {
                "follow-up"
            } else {
                "prompt"
            },
            Style::default()
                .fg(if state.input.trim_start().starts_with('/') {
                    USER
                } else if state.turn_running {
                    WARN
                } else {
                    ASSISTANT
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  Enter send", Style::default().fg(MUTED)),
    ]))
    .alignment(Alignment::Right)
    .style(Style::default().fg(MUTED).bg(PANEL_ALT_BG));
    frame.render_widget(mode, columns[1]);
}

fn render_approval_overlay(frame: &mut ratatui::Frame<'_>, area: Rect, approval: &ApprovalPrompt) {
    let popup = approval_sheet_rect(area);
    frame.render_widget(Clear, popup);

    let body = Paragraph::new(build_approval_text(approval))
        .block(panel_block("Approval Required", BORDER_ACTIVE))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(OVERLAY_BG));
    frame.render_widget(body, popup);
}

fn composer_inner_area(area: Rect) -> Rect {
    let inner = composer_block().inner(area).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(18), Constraint::Length(18)])
        .split(inner)[0]
}

fn approval_sheet_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).max(40);
    let height = area.height.min(14);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let bottom_inset = 5;
    let y = area
        .y
        .saturating_add(area.height.saturating_sub(height + bottom_inset));
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn build_approval_text(approval: &ApprovalPrompt) -> Text<'static> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "tool: ",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(approval.tool_name.clone(), Style::default().fg(TEXT)),
        ]),
        Line::from(vec![
            Span::styled(
                "origin: ",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(approval.origin.clone(), Style::default().fg(TEXT)),
        ]),
    ];
    if !approval.reasons.is_empty() {
        lines.push(Line::from(Span::styled(
            "reasons:",
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        )));
        for reason in &approval.reasons {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(WARN)),
                Span::styled(reason.clone(), Style::default().fg(TEXT)),
            ]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "arguments preview:",
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    )));
    for line in &approval.arguments_preview {
        lines.push(Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(TEXT),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            "y",
            Style::default().fg(ASSISTANT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" approve once   ", Style::default().fg(MUTED)),
        Span::styled("n", Style::default().fg(ERROR).add_modifier(Modifier::BOLD)),
        Span::styled(" deny once   ", Style::default().fg(MUTED)),
        Span::styled(
            "Esc",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" close", Style::default().fg(MUTED)),
    ]));
    Text::from(lines)
}

fn side_info_lines(state: &TuiState) -> Vec<String> {
    let mut lines = vec![
        "## Runtime".to_string(),
        format!(
            "summary: {}",
            preview_text(&state.session.summary_model, 18)
        ),
        format!("memory: {}", preview_text(&state.session.memory_model, 18)),
        "## Store".to_string(),
        format!("saved: {}", state.session.stored_session_count),
        format!("store: {}", preview_text(&state.session.store_label, 18)),
    ];
    if let Some(warning) = &state.session.store_warning {
        lines.push(format!("warning: {}", preview_text(warning, 18)));
    } else {
        lines.push(format!(
            "sandbox: {}",
            preview_text(&state.session.sandbox_summary, 18)
        ));
    }
    lines
}

fn panel_block(title: impl Into<String>, border_color: Color) -> Block<'static> {
    let title = title.into();
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG))
}

fn pane_block(title: impl Into<String>, focused: bool, base_color: Color) -> Block<'static> {
    let title = title.into();
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(base_color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(if focused { BORDER_ACTIVE } else { HEADER })
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG))
}

fn composer_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            " Input ",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_ALT_BG))
}

fn build_transcript_lines(entries: &[String]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "No conversation yet.",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Type a prompt below or open /help.",
            Style::default().fg(SUBTLE),
        )));
        return lines;
    }

    for entry in entries {
        lines.extend(format_transcript_entry(entry));
        lines.push(Line::raw(""));
    }
    lines
}

fn format_transcript_entry(entry: &str) -> Vec<Line<'static>> {
    let (label, accent, body) = if let Some(body) = entry.strip_prefix("user> ") {
        ("user", USER, body)
    } else if let Some(body) = entry.strip_prefix("assistant> ") {
        ("assistant", ASSISTANT, body)
    } else if let Some(body) = entry.strip_prefix("error> ") {
        ("error", ERROR, body)
    } else {
        ("event", WARN, entry)
    };

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    )));
    let mut in_code = false;
    for raw_line in body.lines() {
        if raw_line.trim_start().starts_with("```") {
            in_code = !in_code;
            lines.push(Line::from(vec![Span::styled(
                if in_code { "code block" } else { "end code" },
                Style::default().fg(MUTED).add_modifier(Modifier::DIM),
            )]));
            continue;
        }
        lines.push(render_transcript_body_line(raw_line, in_code));
    }
    if body.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "<empty>",
            Style::default().fg(SUBTLE),
        )));
    }
    lines
}

fn render_transcript_body_line(raw_line: &str, in_code: bool) -> Line<'static> {
    if raw_line.trim().is_empty() {
        return Line::from(Span::raw(""));
    }
    if in_code {
        return Line::from(vec![
            Span::raw("  "),
            Span::styled("│", Style::default().fg(SUBTLE)),
            Span::raw(" "),
            code_span(raw_line),
        ]);
    }
    if let Some(rest) = raw_line
        .strip_prefix("- ")
        .or_else(|| raw_line.strip_prefix("* "))
    {
        return Line::from(vec![
            Span::raw("  "),
            Span::styled("•", Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(rest.to_string(), Style::default().fg(TEXT)),
        ]);
    }
    Line::from(vec![
        Span::raw("  "),
        Span::styled(raw_line.to_string(), Style::default().fg(TEXT)),
    ])
}

fn code_span(line: &str) -> Span<'static> {
    let trimmed = line.trim_start();
    let style = if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Style::default().fg(ASSISTANT).bg(PANEL_ALT_BG)
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Style::default().fg(ERROR).bg(PANEL_ALT_BG)
    } else if trimmed.starts_with("@@") {
        Style::default().fg(USER).bg(PANEL_ALT_BG)
    } else {
        Style::default().fg(TEXT).bg(PANEL_ALT_BG)
    };
    Span::styled(line.to_string(), style)
}

fn build_key_value_text(lines: &[String]) -> Text<'static> {
    let mut rendered = Vec::new();
    for line in lines {
        if let Some(title) = line.strip_prefix("## ") {
            if !rendered.is_empty() {
                rendered.push(Line::raw(""));
            }
            rendered.push(Line::from(vec![Span::styled(
                title.to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )]));
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
        } else if let Some(rest) = line.strip_prefix("  ") {
            rendered.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
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
            rendered.push(Line::from(vec![Span::styled(
                section.to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )]));
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

fn inspector_collection_count(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| !line.starts_with("## ") && !line.trim().is_empty())
        .count()
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

fn activity_color(line: &str) -> Color {
    let lower = line.to_ascii_lowercase();
    if lower.contains("failed") || lower.contains("error") || lower.contains("denied") {
        ERROR
    } else if lower.contains("approval")
        || lower.contains("queued")
        || lower.contains("waiting")
        || lower.contains("blocked")
    {
        WARN
    } else if lower.contains("approved")
        || lower.contains("complete")
        || lower.contains("loaded")
        || lower.contains("ready")
        || lower.contains("listed")
    {
        ASSISTANT
    } else if lower.contains("session")
        || lower.contains("resume")
        || lower.contains("steer")
        || lower.contains("prompt")
    {
        USER
    } else {
        TEXT
    }
}

fn activity_marker(line: &str) -> &'static str {
    let lower = line.to_ascii_lowercase();
    if lower.contains("failed") || lower.contains("error") || lower.contains("denied") {
        "!"
    } else if lower.contains("approval")
        || lower.contains("queued")
        || lower.contains("waiting")
        || lower.contains("blocked")
    {
        "•"
    } else if lower.contains("approved")
        || lower.contains("complete")
        || lower.contains("loaded")
        || lower.contains("ready")
        || lower.contains("listed")
    {
        "✓"
    } else {
        "›"
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
        build_collection_text, build_key_value_text, build_transcript_lines, session_context_line,
        session_last_token_line, session_lines, session_total_token_line,
    };
    use crate::frontend::tui::state::TuiState;
    use agent::types::{ContextWindowUsage, TokenLedgerSnapshot, TokenUsage};
    use std::path::PathBuf;

    #[test]
    fn session_token_lines_show_unknown_context_before_first_request() {
        let mut state = TuiState::default();
        state.session.workspace_name = "workspace".to_string();
        state.session.active_session_ref = "run_123".to_string();
        state.session.root_agent_session_id = "session_123".to_string();
        state.session.workspace_root = PathBuf::from("/tmp/workspace");

        assert_eq!(session_context_line(&state), "context: unknown");
        assert_eq!(session_last_token_line(&state), "last: none yet");
        assert_eq!(session_total_token_line(&state), "total: none yet");
        assert!(session_lines(&state).contains(&"total: none yet".to_string()));
    }

    #[test]
    fn session_token_lines_show_cumulative_usage_after_runtime_updates() {
        let mut state = TuiState::default();
        state.session.workspace_name = "workspace".to_string();
        state.session.active_session_ref = "run_123".to_string();
        state.session.root_agent_session_id = "session_123".to_string();
        state.session.workspace_root = PathBuf::from("/tmp/workspace");
        state.session.token_ledger = TokenLedgerSnapshot {
            context_window: Some(ContextWindowUsage {
                used_tokens: 128_000,
                max_tokens: 400_000,
            }),
            last_usage: Some(TokenUsage::from_input_output(9_000, 600, 1_500)),
            cumulative_usage: TokenUsage::from_input_output(32_000, 2_400, 7_000),
        };

        assert_eq!(session_context_line(&state), "context: 128000 / 400000");
        assert_eq!(
            session_last_token_line(&state),
            "last: in 9000  out 600  prefill 7500  decode 600  cache 1500"
        );
        assert_eq!(
            session_total_token_line(&state),
            "total: in 32000  out 2400  prefill 25000  decode 2400  cache 7000"
        );
        assert!(session_lines(&state).contains(&"context: 128000 / 400000".to_string()));
    }

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
    fn transcript_entries_render_with_minimal_header_and_indented_body() {
        let lines = build_transcript_lines(&["assistant> hello world".to_string()]);

        assert_eq!(lines[0].spans[0].content.as_ref(), "assistant");
        assert_eq!(lines[1].spans[0].content.as_ref(), "  ");
        assert_eq!(lines[1].spans[1].content.as_ref(), "hello world");
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
