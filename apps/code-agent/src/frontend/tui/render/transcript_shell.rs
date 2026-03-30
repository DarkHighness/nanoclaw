use super::super::state::{TranscriptEntry, TranscriptShellEntry, TuiState, preview_text};
use super::shared::{
    pending_control_focus_label, pending_control_kind_label, pending_control_reason_label,
};
use super::statusline::status_color;
use super::theme::{ASSISTANT, ERROR, HEADER, MUTED, SUBTLE, TEXT, USER, WARN};
use super::transcript::TranscriptEntryKind;
use super::transcript_markdown::{render_shell_code_block, render_transcript_body_line};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::Instant;

const COLLAPSED_SHELL_PREVIEW_DETAIL_LINES: usize = 2;

pub(super) fn should_collapse_shell_details(
    entry: &TranscriptEntry,
    show_tool_details: bool,
) -> bool {
    // Keep the default transcript on a single readable timeline. Operators can
    // opt back into the full tool payload stream via `/details`.
    !show_tool_details && entry.is_shell_summary() && hidden_shell_detail_line_count(entry) > 0
}

pub(super) fn render_collapsed_shell_summary(
    entry: &TranscriptEntry,
    marker: &str,
    accent: Color,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Vec<Line<'static>> {
    let summary = entry
        .shell_summary()
        .expect("collapsed shell summaries require structured details");
    let preview_body = collapsed_shell_preview_body(summary);
    let hidden_line_count = hidden_shell_detail_line_count(entry);

    let mut rendered = render_shell_summary_body(&preview_body, marker, kind, animation_frame);
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

fn collapsed_shell_preview_body(summary: &TranscriptShellEntry) -> String {
    TranscriptShellEntry {
        headline: summary.headline.clone(),
        detail_lines: summary
            .detail_lines
            .iter()
            .take(COLLAPSED_SHELL_PREVIEW_DETAIL_LINES)
            .cloned()
            .collect(),
    }
    .serialized_body()
}

fn hidden_shell_detail_line_count(entry: &TranscriptEntry) -> usize {
    entry
        .shell_summary()
        .map(|summary| summary.detail_lines.len())
        .unwrap_or_default()
        .saturating_sub(COLLAPSED_SHELL_PREVIEW_DETAIL_LINES)
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
    animation_frame: Option<u128>,
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

        if first_visible
            && let Some(animated) =
                render_animated_shell_status_line(raw_line, marker, kind, animation_frame)
        {
            rendered.push(animated);
        } else {
            rendered.push(render_transcript_body_line(
                raw_line,
                marker,
                kind,
                false,
                first_visible,
            ));
        }
        if !raw_line.trim().is_empty() {
            first_visible = false;
        }
    }

    if rendered.is_empty() {
        rendered.push(Line::from(Span::raw("")));
    }

    rendered
}

fn render_animated_shell_status_line(
    raw_line: &str,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Option<Line<'static>> {
    let frame_ms = animation_frame?;
    let (status, remainder, accent) = shell_status_phrase(raw_line)?;
    let mut spans = animated_status_phrase_spans(status, frame_ms, accent);
    if !remainder.is_empty() {
        spans.push(Span::styled(
            remainder.to_string(),
            transcript_body_style(marker, kind, raw_line),
        ));
    }
    Some(Line::from(spans))
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
        if state.session.queued_commands > 0 && state.pending_control_picker.is_none() {
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
    } else if state.pending_control_picker.is_none() {
        vec![Line::from(vec![
            Span::styled("+", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(
                format!("{} queued command(s)", state.session.queued_commands),
                Style::default().fg(MUTED),
            ),
        ])]
    } else {
        Vec::new()
    }
}

pub(super) fn pending_control_timeline_entry(state: &TuiState) -> Option<TranscriptEntry> {
    let timeline = pending_control_timeline(state)?;
    let mut detail_lines = Vec::new();
    if timeline.older_hidden_count > 0 {
        detail_lines.push(format!("  └ {} older pending", timeline.older_hidden_count));
    }
    detail_lines.extend(
        timeline
            .recent
            .iter()
            .map(pending_control_timeline_detail_text),
    );
    Some(TranscriptEntry::shell_summary_entry(
        format!("Queued follow-ups · {}", state.pending_controls.len()),
        &detail_lines,
    ))
}

pub(super) fn pending_control_picker_bridge_entry(state: &TuiState) -> Option<TranscriptEntry> {
    pending_control_picker_bridge_label(state)
        .map(|label| TranscriptEntry::shell_summary_entry(label, &[]))
}

pub(super) fn pending_control_embedded_lines(
    state: &TuiState,
    animation_frame: Option<u128>,
) -> Option<Vec<Line<'static>>> {
    let timeline = pending_control_timeline(state)?;
    let mut lines = render_shell_summary_body(
        &format!("Queued follow-ups · {}", state.pending_controls.len()),
        "•",
        TranscriptEntryKind::ShellSummary,
        animation_frame,
    )
    .into_iter()
    .map(|line| {
        let mut spans = vec![transcript_continuation_prefix(
            TranscriptEntryKind::ShellSummary,
        )];
        spans.extend(line.spans);
        Line::from(spans)
    })
    .collect::<Vec<_>>();
    if timeline.older_hidden_count > 0 {
        lines.push(Line::from(vec![
            transcript_continuation_prefix(TranscriptEntryKind::ShellSummary),
            Span::styled("  └ ", Style::default().fg(SUBTLE)),
            Span::styled(
                format!("{} older pending", timeline.older_hidden_count),
                Style::default().fg(MUTED),
            ),
        ]));
    }
    lines.extend(
        timeline
            .recent
            .iter()
            .map(render_pending_control_embedded_detail),
    );
    Some(lines)
}

pub(super) fn pending_control_picker_embedded_lines(
    state: &TuiState,
    animation_frame: Option<u128>,
) -> Option<Vec<Line<'static>>> {
    let label = pending_control_picker_bridge_label(state)?;
    Some(
        render_shell_summary_body(
            &label,
            "•",
            TranscriptEntryKind::ShellSummary,
            animation_frame,
        )
        .into_iter()
        .map(|line| {
            let mut spans = vec![transcript_continuation_prefix(
                TranscriptEntryKind::ShellSummary,
            )];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect(),
    )
}

struct PendingControlTimeline {
    older_hidden_count: usize,
    recent: Vec<PendingControlTimelineItem>,
}

struct PendingControlTimelineItem {
    relative_label: &'static str,
    kind: crate::backend::PendingControlKind,
    preview: String,
    reason: Option<String>,
    editing: bool,
}

fn render_pending_control_embedded_detail(item: &PendingControlTimelineItem) -> Line<'static> {
    let (kind_label, kind_color) = pending_control_timeline_kind_label(item.kind, item.editing);
    let mut spans = vec![
        transcript_continuation_prefix(TranscriptEntryKind::ShellSummary),
        Span::styled("  └ ", Style::default().fg(SUBTLE)),
    ];
    if !item.editing {
        spans.push(Span::styled(
            format!("{} ", item.relative_label),
            Style::default().fg(MUTED),
        ));
    }
    spans.extend([
        Span::styled(
            kind_label,
            Style::default().fg(kind_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(item.preview.clone(), Style::default().fg(TEXT)),
    ]);
    if let Some(reason) = item.reason.as_deref() {
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled(reason.to_string(), Style::default().fg(MUTED)));
    }
    Line::from(spans)
}

fn pending_control_timeline_detail_text(item: &PendingControlTimelineItem) -> String {
    let (kind_label, _) = pending_control_timeline_kind_label(item.kind, item.editing);
    let mut detail = if item.editing {
        format!("  └ {} · {}", kind_label, item.preview)
    } else {
        format!(
            "  └ {} {} · {}",
            item.relative_label, kind_label, item.preview
        )
    };
    if let Some(reason) = item.reason.as_deref() {
        detail.push_str(" · ");
        detail.push_str(reason);
    }
    detail
}

fn pending_control_timeline_kind_label(
    kind: crate::backend::PendingControlKind,
    editing: bool,
) -> (&'static str, Color) {
    match (kind, editing) {
        (crate::backend::PendingControlKind::Prompt, true) => ("editing queued prompt", USER),
        (crate::backend::PendingControlKind::Steer, true) => ("editing queued steer", ASSISTANT),
        (crate::backend::PendingControlKind::Prompt, false) => ("queued prompt", USER),
        (crate::backend::PendingControlKind::Steer, false) => ("pending steer", ASSISTANT),
    }
}

fn pending_control_timeline(state: &TuiState) -> Option<PendingControlTimeline> {
    if state.pending_controls.is_empty() || state.pending_control_picker.is_some() {
        return None;
    }

    // Keep the queued-control summary model shared between the standalone block
    // and the embedded tool continuation so both surfaces describe the same
    // runtime-owned queue ordering.
    let total = state.pending_controls.len();
    let mut visible_indices = if total <= 2 {
        (0..total).collect::<Vec<_>>()
    } else {
        vec![total - 2, total - 1]
    };
    if let Some(editing) = state.editing_pending_control.as_ref()
        && let Some(editing_index) = state
            .pending_controls
            .iter()
            .position(|control| control.id == editing.id)
        && !visible_indices.contains(&editing_index)
    {
        // Editing a queued control should keep that exact item visible in the
        // transcript, even when it is older than the normal "latest two"
        // summary window. Otherwise the operator loses the connection between
        // the composer edit state and the runtime-owned pending queue.
        visible_indices = vec![editing_index, total - 1];
        visible_indices.sort_unstable();
    }

    let visible_total = visible_indices.len();
    let recent = visible_indices
        .into_iter()
        .enumerate()
        .map(|(index, control_index)| {
            let control = &state.pending_controls[control_index];
            let relative_label = if visible_total == 1 {
                "next"
            } else if index + 1 == visible_total {
                "latest"
            } else {
                "older"
            };
            PendingControlTimelineItem {
                relative_label,
                kind: control.kind,
                preview: preview_text(&control.preview, 72),
                reason: pending_control_reason_label(control.reason.as_deref())
                    .map(|reason| preview_text(&reason, 28)),
                editing: state
                    .editing_pending_control
                    .as_ref()
                    .is_some_and(|editing| editing.id == control.id),
            }
        })
        .collect();

    Some(PendingControlTimeline {
        older_hidden_count: total.saturating_sub(2),
        recent,
    })
}

fn pending_control_picker_bridge_label(state: &TuiState) -> Option<String> {
    if state.pending_controls.is_empty() || state.pending_control_picker.is_none() {
        return None;
    }
    if let (Some(selected), Some(picker)) = (
        state.selected_pending_control(),
        state.pending_control_picker.as_ref(),
    ) {
        return Some(format!(
            "Queued follow-ups below · selected {} · {}",
            pending_control_kind_label(selected.kind),
            pending_control_focus_label(picker.selected, state.pending_controls.len()),
        ));
    }
    Some(format!(
        "Queued follow-ups below · {}",
        state.pending_controls.len()
    ))
}

pub(super) fn animated_progress_text_spans(text: &str, frame_ms: u128) -> Vec<Span<'static>> {
    animated_emphasis_text_spans(text, frame_ms, HEADER, USER, TEXT, ASSISTANT, MUTED)
}

fn animated_status_phrase_spans(text: &str, frame_ms: u128, accent: Color) -> Vec<Span<'static>> {
    animated_emphasis_text_spans(text, frame_ms, HEADER, accent, TEXT, accent, MUTED)
}

fn animated_emphasis_text_spans(
    text: &str,
    frame_ms: u128,
    head_color: Color,
    leading_color: Color,
    mid_color: Color,
    trailing_color: Color,
    base_color: Color,
) -> Vec<Span<'static>> {
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
                0 => (head_color, Modifier::BOLD),
                1 => (leading_color, Modifier::BOLD),
                2 | 3 => (mid_color, Modifier::BOLD),
                4 | 5 => (trailing_color, Modifier::empty()),
                _ => (base_color, Modifier::empty()),
            };
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(modifier),
            )
        })
        .collect()
}

pub(super) fn animation_frame_ms(started_at: Instant, now: Instant) -> u128 {
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

pub(super) fn summary_color(line: &str) -> Color {
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
        || lower.contains("queued")
        || lower.contains("applying")
    {
        WARN
    } else {
        TEXT
    }
}

fn shell_status_phrase(line: &str) -> Option<(&str, &str, Color)> {
    if line.starts_with("Awaiting approval for ") {
        let phrase = "Awaiting approval";
        return Some((phrase, &line[phrase.len()..], WARN));
    }
    if line.starts_with("Requested ") {
        let phrase = "Requested";
        return Some((phrase, &line[phrase.len()..], WARN));
    }
    if line.starts_with("Queued ") {
        let phrase = "Queued";
        return Some((phrase, &line[phrase.len()..], WARN));
    }
    if line.starts_with("Running ") {
        let phrase = "Running";
        return Some((phrase, &line[phrase.len()..], USER));
    }
    if line.starts_with("Finished ") {
        let phrase = "Finished";
        return Some((phrase, &line[phrase.len()..], ASSISTANT));
    }
    if line.starts_with("Approved ") {
        let phrase = "Approved";
        return Some((phrase, &line[phrase.len()..], ASSISTANT));
    }
    if line.starts_with("Denied ") {
        let phrase = "Denied";
        return Some((phrase, &line[phrase.len()..], ERROR));
    }
    None
}
