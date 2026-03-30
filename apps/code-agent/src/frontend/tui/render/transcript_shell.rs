use super::super::state::{TuiState, preview_text};
use super::statusline::status_color;
use super::theme::{ASSISTANT, ERROR, HEADER, MUTED, SUBTLE, TEXT, USER, WARN};
use super::transcript::TranscriptEntryKind;
use super::transcript_markdown::{render_shell_code_block, render_transcript_body_line};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::Instant;

pub(super) fn should_collapse_shell_details(
    kind: TranscriptEntryKind,
    body: &str,
    show_tool_details: bool,
) -> bool {
    // Keep the default transcript on a single readable timeline. Operators can
    // opt back into the full tool payload stream via `/details`.
    !show_tool_details
        && kind == TranscriptEntryKind::ShellSummary
        && body.lines().skip(1).any(|line| !line.trim().is_empty())
}

pub(super) fn render_collapsed_shell_summary(
    marker: &str,
    accent: Color,
    body: &str,
    kind: TranscriptEntryKind,
) -> Vec<Line<'static>> {
    let headline = body.lines().next().unwrap_or_default();
    let hidden_line_count = body
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .count();

    let mut rendered = render_shell_summary_body(headline, marker, kind);
    if hidden_line_count > 0 {
        rendered.push(Line::from(vec![
            transcript_continuation_prefix(kind),
            Span::styled(
                format!(
                    "{} hidden line{} · /details",
                    hidden_line_count,
                    if hidden_line_count == 1 { "" } else { "s" }
                ),
                Style::default().fg(SUBTLE),
            ),
        ]));
    }
    prefix_transcript_marker(&mut rendered, marker, accent, kind);
    rendered
}

pub(super) fn transcript_entry_kind(marker: &str, body: &str) -> TranscriptEntryKind {
    match marker {
        "›" => TranscriptEntryKind::UserPrompt,
        "✔" => TranscriptEntryKind::SuccessSummary,
        "✗" => TranscriptEntryKind::ErrorSummary,
        "⚠" => TranscriptEntryKind::WarningSummary,
        _ if body
            .lines()
            .any(|line| line.starts_with("  └ ") || line.starts_with("    ")) =>
        {
            TranscriptEntryKind::ShellSummary
        }
        _ => TranscriptEntryKind::AssistantMessage,
    }
}

pub(super) fn render_shell_summary_body(
    body: &str,
    marker: &str,
    kind: TranscriptEntryKind,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    let mut first_visible = true;
    let mut lines = body.lines();

    while let Some(raw_line) = lines.next() {
        let trimmed = raw_line.trim_start();
        if let Some(language) = trimmed.strip_prefix("```") {
            let mut code_lines = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code_lines.push(code_line);
            }
            let code = code_lines.join("\n");
            let block = render_shell_code_block(language.trim(), &code, kind, first_visible);
            if block.iter().any(line_has_visible_content) {
                first_visible = false;
            }
            rendered.extend(block);
            continue;
        }

        rendered.push(render_transcript_body_line(
            raw_line,
            marker,
            kind,
            false,
            first_visible,
        ));
        if !raw_line.trim().is_empty() {
            first_visible = false;
        }
    }

    if rendered.is_empty() {
        rendered.push(Line::from(Span::raw("")));
    }

    rendered
}

pub(super) fn prefix_transcript_marker(
    lines: &mut [Line<'static>],
    marker: &str,
    accent: Color,
    kind: TranscriptEntryKind,
) {
    let index = lines
        .iter()
        .position(line_has_visible_content)
        .unwrap_or_default();
    let mut spans = vec![
        Span::styled(
            marker.to_string(),
            transcript_marker_style(marker, accent, kind),
        ),
        Span::raw(" "),
    ];
    spans.extend(lines[index].spans.clone());
    lines[index] = Line::from(spans);
}

pub(super) fn line_has_visible_content(line: &Line<'static>) -> bool {
    !line_to_plain_text(line).trim().is_empty()
}

pub(super) fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

pub(super) fn transcript_continuation_prefix(kind: TranscriptEntryKind) -> Span<'static> {
    match kind {
        TranscriptEntryKind::ShellSummary
        | TranscriptEntryKind::SuccessSummary
        | TranscriptEntryKind::ErrorSummary
        | TranscriptEntryKind::WarningSummary => Span::styled("    ", Style::default().fg(SUBTLE)),
        _ => Span::raw("  "),
    }
}

pub(super) fn parse_prefixed_entry(entry: &str) -> Option<(&'static str, Color, &str)> {
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

pub(super) fn transcript_body_style(marker: &str, kind: TranscriptEntryKind, line: &str) -> Style {
    let style = match kind {
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage => {
            Style::default().fg(TEXT)
        }
        TranscriptEntryKind::ShellSummary => Style::default().fg(summary_color(line)),
        TranscriptEntryKind::SuccessSummary => Style::default().fg(ASSISTANT),
        TranscriptEntryKind::ErrorSummary => Style::default().fg(ERROR),
        TranscriptEntryKind::WarningSummary => Style::default().fg(WARN),
    };

    if marker == "›" {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn transcript_marker_style(marker: &str, accent: Color, kind: TranscriptEntryKind) -> Style {
    let color = match kind {
        TranscriptEntryKind::AssistantMessage => MUTED,
        TranscriptEntryKind::UserPrompt => USER,
        _ => accent,
    };
    let style = Style::default().fg(color);
    if matches!(marker, "›" | "✗" | "✔") {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

pub(super) fn live_progress_lines(state: &TuiState) -> Vec<Line<'static>> {
    if state.turn_running {
        let frame_time = Instant::now();
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
        ];
        let mut progress_label = preview_text(&status, 56);
        if let Some(tool_label) = state.active_tool_label.as_deref() {
            progress_label.push_str(" · ");
            progress_label.push_str(tool_label);
        }
        spans.extend(animated_progress_text_spans(
            &progress_label,
            animation_frame_ms(state.turn_started_at.unwrap_or(frame_time), frame_time),
        ));
        if state.session.queued_commands > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled(
                if state.active_tool_label.is_some() {
                    format!(
                        "{} queued behind current tool",
                        state.session.queued_commands
                    )
                } else {
                    format!("{} queued", state.session.queued_commands)
                },
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

pub(super) fn animated_progress_text_spans(text: &str, frame_ms: u128) -> Vec<Span<'static>> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return vec![Span::raw("")];
    }

    let glow_width = 7usize;
    let head = ((frame_ms / 75) as usize) % (chars.len() + glow_width);
    let head = head as isize - glow_width as isize;

    chars
        .into_iter()
        .enumerate()
        .map(|(index, ch)| {
            if ch.is_whitespace() {
                return Span::styled(ch.to_string(), Style::default().fg(SUBTLE));
            }

            let delta = index as isize - head;
            let (color, modifier) = match delta {
                0 => (HEADER, Modifier::BOLD),
                1 => (USER, Modifier::BOLD),
                2 | 3 => (TEXT, Modifier::BOLD),
                4 | 5 => (ASSISTANT, Modifier::empty()),
                _ => (MUTED, Modifier::empty()),
            };
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(modifier),
            )
        })
        .collect()
}

fn animation_frame_ms(started_at: Instant, now: Instant) -> u128 {
    now.duration_since(started_at).as_millis()
}

fn live_progress_summary(state: &TuiState) -> String {
    match state.status.as_str() {
        "Waiting for approval" => "Waiting for approval".to_string(),
        status if !status.is_empty() => status.to_string(),
        _ => "Working".to_string(),
    }
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
