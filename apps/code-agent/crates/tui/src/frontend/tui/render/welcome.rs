use super::theme::palette;
use crate::frontend::tui::state::{TuiState, preview_text};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const COMPACT_BREAKPOINT_WIDTH: u16 = 104;
const COMPACT_BREAKPOINT_HEIGHT: u16 = 24;
const COMPACT_CONTENT_MAX_WIDTH: u16 = 74;
const WIDE_CONTENT_MAX_WIDTH: u16 = 104;
const COLUMN_GAP: usize = 5;
const LABEL_WIDTH: usize = 11;

pub(super) fn build_welcome_lines(
    state: &TuiState,
    viewport_width: u16,
    viewport_height: u16,
) -> Vec<Line<'static>> {
    let compact =
        viewport_height < COMPACT_BREAKPOINT_HEIGHT || viewport_width < COMPACT_BREAKPOINT_WIDTH;
    let content_width = welcome_content_width(viewport_width, compact);

    // Keep the idle screen oriented around workspace state and next actions
    // instead of behaving like a splash screen. That mirrors Codex-style shells
    // where the operator should understand the command center before typing.
    let mut core = build_welcome_title_lines(compact, content_width);
    core.push(Line::raw(""));
    core.push(build_divider_line(content_width));
    core.push(Line::raw(""));
    core.extend(build_summary_panels(state, compact, content_width));
    core.push(Line::raw(""));
    core.push(build_prompt_line(compact));
    core.push(Line::raw(""));
    core.push(build_shortcut_line(compact));

    let top_padding = usize::from(viewport_height.saturating_sub(core.len() as u16) / 2);
    let mut lines = vec![Line::raw(""); top_padding];
    lines.extend(core);
    lines
}

fn welcome_content_width(viewport_width: u16, compact: bool) -> usize {
    let max_width = if compact {
        COMPACT_CONTENT_MAX_WIDTH
    } else {
        WIDE_CONTENT_MAX_WIDTH
    };
    usize::from(viewport_width.saturating_sub(8).clamp(48, max_width))
}

fn build_welcome_title_lines(compact: bool, content_width: usize) -> Vec<Line<'static>> {
    let subtitle = if compact {
        "Focused coding work with one live transcript and queued follow-ups."
    } else {
        "Focused coding work with one live transcript, queued follow-ups, and explicit tool control."
    };
    vec![
        Line::from(vec![
            Span::styled(
                "NANOCLAW",
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" / ", Style::default().fg(palette().subtle)),
            Span::styled(
                "command center",
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        wrap_plain_line(
            subtitle,
            content_width,
            Style::default().fg(palette().muted),
        ),
    ]
}

fn build_prompt_line(compact: bool) -> Line<'static> {
    let detail = if compact {
        "Describe the next change in plain language, call a named skill with $skill_name or /skill_name, or run /help."
    } else {
        "Describe the next change in plain language, call a named skill with $skill_name or /skill_name, inspect task history, or run /help."
    };
    Line::from(vec![Span::styled(
        detail,
        Style::default().fg(palette().text),
    )])
}

fn build_shortcut_line(compact: bool) -> Line<'static> {
    let suffix = if compact {
        "Enter run · Tab queue · ↑ history · / commands · $ skills"
    } else {
        "Enter run · Tab queue · ↑ history · / commands · $ skills · ^T effort · ^O editor"
    };
    Line::from(vec![Span::styled(
        suffix,
        Style::default().fg(palette().accent),
    )])
}

fn build_summary_panels(
    state: &TuiState,
    compact: bool,
    content_width: usize,
) -> Vec<Line<'static>> {
    if compact {
        build_compact_summary_panels(state)
    } else {
        build_wide_summary_panels(state, content_width)
    }
}

fn build_wide_summary_panels(state: &TuiState, content_width: usize) -> Vec<Line<'static>> {
    let left_width = (content_width.saturating_sub(COLUMN_GAP)) / 2;
    let right_width = content_width.saturating_sub(COLUMN_GAP + left_width);
    let left = workspace_panel_lines(state, false);
    let right = launch_panel_lines(false);

    left.into_iter()
        .zip(right)
        .map(|(left_line, right_line)| join_columns(left_line, right_line, left_width, right_width))
        .collect()
}

fn build_compact_summary_panels(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = workspace_panel_lines(state, true);
    lines.push(Line::raw(""));
    lines.extend(launch_panel_lines(true));
    lines
}

fn workspace_panel_lines(state: &TuiState, compact: bool) -> Vec<Line<'static>> {
    let repo_value = repo_label(state);
    let runtime_value = runtime_label(state);
    let plugin_value = format!(
        "{}/{} enabled",
        state.session.startup_diagnostics.enabled_plugin_count,
        state.session.startup_diagnostics.total_plugin_count
    );

    vec![
        section_heading_line("workspace", "session + runtime"),
        fact_line(
            "workspace",
            &state.session.workspace_name,
            palette().subtle,
            palette().header,
            compact,
        ),
        fact_line(
            "model",
            &model_label(state),
            palette().subtle,
            palette().accent,
            compact,
        ),
        fact_line(
            "runtime",
            &runtime_value,
            palette().subtle,
            palette().muted,
            compact,
        ),
        fact_line(
            "repo",
            &repo_value,
            palette().subtle,
            palette().user,
            compact,
        ),
        fact_line(
            "plugins",
            &plugin_value,
            palette().subtle,
            palette().assistant,
            compact,
        ),
    ]
}

fn launch_panel_lines(compact: bool) -> Vec<Line<'static>> {
    vec![
        section_heading_line("launch", "next actions"),
        fact_line(
            "start",
            "Describe the change in plain language",
            palette().accent,
            palette().text,
            compact,
        ),
        fact_line(
            "queue",
            "Tab stages follow-ups behind the current run",
            palette().accent,
            palette().text,
            compact,
        ),
        fact_line(
            "control",
            "/ commands, $ skills, ↑ history",
            palette().accent,
            palette().text,
            compact,
        ),
        fact_line(
            "inspect",
            "/task, review history, or /help",
            palette().accent,
            palette().text,
            compact,
        ),
        fact_line(
            "edit",
            "^T effort · ^O external editor",
            palette().accent,
            palette().text,
            compact,
        ),
    ]
}

fn model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) => format!("{} · {}", state.session.model, effort),
        None => state.session.model.clone(),
    }
}

fn runtime_label(state: &TuiState) -> String {
    let diagnostics = &state.session.startup_diagnostics;
    format!(
        "{} tools · {} mcp · {} skills",
        diagnostics.local_tool_count + diagnostics.mcp_tool_count,
        diagnostics.mcp_servers.len(),
        state.session.skills.len()
    )
}

fn repo_label(state: &TuiState) -> String {
    if state.session.git.available && !state.session.git.repo_name.is_empty() {
        if state.session.git.branch.is_empty() {
            state.session.git.repo_name.clone()
        } else {
            format!(
                "{}@{}",
                state.session.git.repo_name, state.session.git.branch
            )
        }
    } else {
        "no git context yet".to_string()
    }
}

fn section_heading_line(title: &str, summary: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            title.to_string(),
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(summary.to_string(), Style::default().fg(palette().muted)),
    ])
}

fn fact_line(
    label: &str,
    value: &str,
    label_color: ratatui::style::Color,
    value_color: ratatui::style::Color,
    compact: bool,
) -> Line<'static> {
    let value_limit = if compact { 44 } else { 39 };
    Line::from(vec![
        Span::styled(
            format!("{label:<width$}", width = LABEL_WIDTH),
            Style::default().fg(label_color),
        ),
        Span::styled(
            preview_text(value, value_limit),
            Style::default().fg(value_color),
        ),
    ])
}

fn build_divider_line(content_width: usize) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(content_width.min(96)),
        Style::default().fg(palette().subtle),
    ))
}

fn wrap_plain_line(text: &str, width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(preview_text(text, width), style))
}

fn join_columns(
    left: Line<'static>,
    right: Line<'static>,
    left_width: usize,
    right_width: usize,
) -> Line<'static> {
    let mut spans = pad_line_to_width(left, left_width).spans;
    spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
    spans.extend(pad_line_to_width(right, right_width).spans);
    Line::from(spans)
}

fn pad_line_to_width(mut line: Line<'static>, width: usize) -> Line<'static> {
    let visible_width = line_width(&line);
    if visible_width < width {
        line.spans
            .push(Span::raw(" ".repeat(width.saturating_sub(visible_width))));
    }
    line
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}
