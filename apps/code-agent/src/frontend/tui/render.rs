use super::approval::ApprovalPrompt;
use super::state::{PaneFocus, TuiState, preview_text};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

const BG: Color = Color::Rgb(9, 13, 20);
const TOP_BG: Color = Color::Rgb(11, 15, 23);
const PANEL_BG: Color = Color::Rgb(13, 18, 28);
const PANEL_ALT_BG: Color = Color::Rgb(17, 23, 34);
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
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, vertical[0], state);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(vertical[1]);

    render_transcript(frame, body[0], state);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Min(8),
        ])
        .split(body[1]);

    render_session(frame, right[0], state);
    render_inspector(frame, right[1], state);
    render_activity(frame, right[2], state);
    render_composer(frame, vertical[2], state);
    render_footer(frame, vertical[3]);

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
    frame.render_widget(topbar_block(), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(inner);
    let git_branch = state.session.git.branch_label();
    let git_dirty = state.session.git.dirty_label();
    let queue_label = format!("queue {}", state.session.queued_commands);

    let title = Paragraph::new(Text::from(vec![
        Line::from(vec![
            badge("CODE", USER),
            Span::raw(" "),
            Span::styled(
                state.session.workspace_name.clone(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  workspace agent", Style::default().fg(MUTED)),
        ]),
        Line::from(Span::styled(
            state.session.workspace_root.display().to_string(),
            Style::default().fg(SUBTLE),
        )),
    ]))
    .style(Style::default().bg(TOP_BG));
    frame.render_widget(title, split[0]);

    let status = Paragraph::new(Text::from(vec![
        Line::from(vec![
            chip(
                if state.turn_running {
                    "ACTIVE"
                } else {
                    "READY"
                },
                status_color(&state.status),
            ),
            Span::raw(" "),
            chip(&state.session.provider_label, USER),
            Span::raw(" "),
            chip(&state.session.model, MUTED),
        ]),
        Line::from(vec![
            chip(&git_branch, USER),
            Span::raw(" "),
            chip(
                &git_dirty,
                if state.session.git.is_dirty() {
                    WARN
                } else {
                    ASSISTANT
                },
            ),
            Span::raw(" "),
            chip(
                &queue_label,
                if state.session.queued_commands > 0 {
                    WARN
                } else {
                    MUTED
                },
            ),
        ]),
    ]))
    .alignment(Alignment::Right)
    .style(Style::default().bg(TOP_BG));
    frame.render_widget(status, split[1]);
}

fn render_transcript(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let lines = build_transcript_lines(&state.transcript);
    let block = pane_block(
        "Conversation",
        state.focus == PaneFocus::Conversation,
        BORDER_ACTIVE,
    );
    let scroll = clamp_scroll(
        state.transcript_scroll,
        lines.len(),
        block.inner(area).height,
    );
    let transcript = Paragraph::new(Text::from(lines))
        .block(block)
        .scroll((scroll, 0))
        .alignment(if state.transcript.is_empty() {
            Alignment::Center
        } else {
            Alignment::Left
        })
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_BG));
    frame.render_widget(transcript, area);
}

fn render_session(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let session = Paragraph::new(build_key_value_text(&session_lines(state)))
        .block(pane_block("Session", false, BORDER))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_ALT_BG));
    frame.render_widget(session, area);
}

fn session_lines(state: &TuiState) -> Vec<String> {
    vec![
        format!("workspace: {}", state.session.workspace_name),
        format!(
            "path: {}",
            preview_text(&state.session.workspace_root.display().to_string(), 52)
        ),
        format!(
            "resources: tools {}  skills {}",
            state.session.tool_names.len(),
            if state.session.skill_names.is_empty() {
                "none".to_string()
            } else {
                state.session.skill_names.len().to_string()
            }
        ),
        session_context_line(state),
        session_last_token_line(state),
        session_total_token_line(state),
        format!("queue: {} pending", state.session.queued_commands),
        format!("branch: {}", state.session.git.branch_label()),
        format!("dirty: {}", state.session.git.dirty_label()),
    ]
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

fn render_inspector(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let title = if state.inspector_title.is_empty() {
        "Guide"
    } else {
        state.inspector_title.as_str()
    };
    let block = pane_block(title, state.focus == PaneFocus::Inspector, BORDER);
    let scroll = clamp_scroll(
        state.inspector_scroll,
        state.inspector.len().max(1),
        block.inner(area).height,
    );
    let inspector = Paragraph::new(build_key_value_text(&state.inspector))
        .block(block)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_ALT_BG));
    frame.render_widget(inspector, area);
}

fn render_activity(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let lines = if state.activity.is_empty() {
        Text::from(vec![Line::from(Span::styled(
            "No activity yet.",
            Style::default().fg(SUBTLE),
        ))])
    } else {
        let mut lines = Vec::new();
        for item in state.activity.iter().rev() {
            lines.push(Line::from(vec![
                Span::styled("• ", Style::default().fg(WARN)),
                Span::styled(item.clone(), Style::default().fg(TEXT)),
            ]));
        }
        Text::from(lines)
    };
    let block = pane_block("Activity Feed", state.focus == PaneFocus::Activity, BORDER);
    let scroll = clamp_scroll(
        state.activity_scroll,
        state.activity.len().max(1),
        block.inner(area).height,
    );
    let activity = Paragraph::new(lines)
        .block(block)
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(PANEL_BG));
    frame.render_widget(activity, area);
}

fn render_composer(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let block = composer_block();
    let inner = block.inner(area).inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(24), Constraint::Length(30)])
        .split(rows[0]);
    frame.render_widget(block, area);

    let input_text = if state.input.is_empty() {
        Line::from(vec![
            Span::styled(">", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
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
    frame.render_widget(Clear, top[0]);
    frame.render_widget(input_field, top[0]);

    let actions = Paragraph::new(Line::from(vec![
        Span::styled(
            "Enter",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" send  ", Style::default().fg(MUTED)),
        Span::styled(
            "/help",
            Style::default().fg(USER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" commands", Style::default().fg(MUTED)),
    ]))
    .alignment(Alignment::Right)
    .style(Style::default().fg(MUTED).bg(PANEL_ALT_BG));
    frame.render_widget(actions, top[1]);

    let helper_line = if state.input.is_empty() {
        Line::from(vec![
            Span::styled(
                "Ask for inspection, edits, tests, or an explanation of the codebase.",
                Style::default().fg(SUBTLE),
            ),
            Span::raw("  "),
            Span::styled(
                "/help",
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" commands  ", Style::default().fg(MUTED)),
            Span::styled(
                "/steer",
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" queued guidance", Style::default().fg(MUTED)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "/tools",
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" tool catalog  ", Style::default().fg(MUTED)),
            Span::styled(
                "/compact",
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" summarize history  ", Style::default().fg(MUTED)),
            Span::styled(
                "/steer",
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" guide next turn  ", Style::default().fg(MUTED)),
            Span::styled(
                "Tab",
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" pane", Style::default().fg(MUTED)),
        ])
    };
    let helper = Paragraph::new(helper_line).style(Style::default().fg(MUTED).bg(PANEL_ALT_BG));
    frame.render_widget(helper, rows[1]);
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            "Ctrl+C",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit  ", Style::default().fg(MUTED)),
        Span::styled(
            "Tab",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" switch pane  ", Style::default().fg(MUTED)),
        Span::styled(
            "↑↓ PgUp PgDn",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" scroll focused pane", Style::default().fg(MUTED)),
    ]))
    .style(Style::default().bg(BG));
    frame.render_widget(footer, area);
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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(24), Constraint::Length(30)])
        .split(rows[0])[0]
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

fn panel_block<'a>(title: &'a str, border_color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG))
}

fn pane_block<'a>(title: &'a str, focused: bool, base_color: Color) -> Block<'a> {
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

fn composer_block<'a>() -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER_ACTIVE))
        .title(Span::styled(
            " Composer ",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_ALT_BG))
}

fn topbar_block<'a>() -> Block<'a> {
    Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(TOP_BG))
}

fn chip<'a>(label: &'a str, color: Color) -> Span<'a> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(BG)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn badge<'a>(label: &'a str, color: Color) -> Span<'a> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(BG)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn build_transcript_lines(entries: &[String]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "No messages yet",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Ask the agent to inspect files, run tests, explain architecture, or make a patch.",
            Style::default().fg(SUBTLE),
        )));
        lines.push(Line::from(Span::styled(
            "The composer stays active below and approvals dock in place when risky tools are needed.",
            Style::default().fg(MUTED),
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
        ("USER", USER, body)
    } else if let Some(body) = entry.strip_prefix("assistant> ") {
        ("ASSISTANT", ASSISTANT, body)
    } else if let Some(body) = entry.strip_prefix("error> ") {
        ("ERROR", ERROR, body)
    } else {
        ("EVENT", WARN, entry)
    };

    let mut lines = Vec::new();
    let badge = Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(BG)
            .bg(accent)
            .add_modifier(Modifier::BOLD),
    );
    let mut in_code = false;
    for (index, raw_line) in body.lines().enumerate() {
        let prefix = if index == 0 {
            badge.clone()
        } else {
            Span::styled("      ", Style::default().fg(SUBTLE))
        };
        if raw_line.trim_start().starts_with("```") {
            in_code = !in_code;
            lines.push(Line::from(vec![
                prefix,
                Span::raw(" "),
                Span::styled(
                    if in_code { "code block" } else { "end code" },
                    Style::default()
                        .fg(MUTED)
                        .add_modifier(Modifier::ITALIC | Modifier::DIM),
                ),
            ]));
            continue;
        }
        lines.push(render_transcript_body_line(prefix, raw_line, in_code));
    }
    if lines.is_empty() {
        lines.push(Line::from(vec![
            badge,
            Span::raw(" "),
            Span::styled("<empty>", Style::default().fg(SUBTLE)),
        ]));
    }
    lines
}

fn render_transcript_body_line(
    prefix: Span<'static>,
    raw_line: &str,
    in_code: bool,
) -> Line<'static> {
    if raw_line.trim().is_empty() {
        return Line::from(vec![
            prefix,
            Span::raw(" "),
            Span::styled(" ", Style::default()),
        ]);
    }
    if in_code {
        return Line::from(vec![
            prefix,
            Span::raw(" "),
            Span::styled("│", Style::default().fg(SUBTLE).bg(PANEL_ALT_BG)),
            Span::raw(" "),
            code_span(raw_line),
        ]);
    }
    if let Some(rest) = raw_line
        .strip_prefix("- ")
        .or_else(|| raw_line.strip_prefix("* "))
    {
        return Line::from(vec![
            prefix,
            Span::raw(" "),
            Span::styled("•", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(rest.to_string(), Style::default().fg(TEXT)),
        ]);
    }
    Line::from(vec![
        prefix,
        Span::raw(" "),
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
        if let Some((key, value)) = line.split_once(':') {
            rendered.push(Line::from(vec![
                Span::styled(
                    format!("{key}:"),
                    Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(value.trim().to_string(), Style::default().fg(TEXT)),
            ]));
        } else if let Some(rest) = line.strip_prefix("  ") {
            rendered.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(rest.to_string(), Style::default().fg(TEXT)),
            ]));
        } else {
            rendered.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(TEXT),
            )));
        }
    }
    Text::from(rendered)
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
        session_context_line, session_last_token_line, session_lines, session_total_token_line,
    };
    use crate::frontend::tui::state::TuiState;
    use agent::types::{ContextWindowUsage, TokenLedgerSnapshot, TokenUsage};
    use std::path::PathBuf;

    #[test]
    fn session_token_lines_show_unknown_context_before_first_request() {
        let mut state = TuiState::default();
        state.session.workspace_name = "workspace".to_string();
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
}
