use super::super::state::{TranscriptEntry, TuiState};
use super::transcript_markdown::render_markdown_body;
use super::transcript_shell::{
    animation_frame_ms, live_progress_lines, pending_control_embedded_lines,
    pending_control_picker_bridge_entry, pending_control_picker_embedded_lines,
    pending_control_timeline_entry, prefix_transcript_marker, render_collapsed_shell_summary,
    render_shell_summary_body, should_collapse_shell_details,
};
pub(super) use super::transcript_shell::{
    line_has_visible_content, line_to_plain_text, parse_prefixed_entry, transcript_body_style,
    transcript_continuation_prefix, transcript_entry_kind,
};
use super::view::build_inspector_text;
use super::welcome::build_welcome_lines;
use super::{shared, theme::*};
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use std::time::Instant;

pub(super) fn render_transcript(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    if state.transcript.is_empty() && !state.turn_running && state.session.queued_commands == 0 {
        let lines = build_welcome_lines(state, inner.width, inner.height);
        let empty = Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(MAIN_BG));
        frame.render_widget(empty, inner);
        return;
    }

    let lines = build_transcript_lines_for_width(state, inner.width);
    let scroll = shared::clamp_scroll(state.transcript_scroll, lines.len(), inner.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(transcript, inner);
}

pub(super) fn build_transcript_lines(state: &TuiState) -> Vec<Line<'static>> {
    build_transcript_lines_for_width(state, 80)
}

pub(super) fn build_transcript_lines_for_width(
    state: &TuiState,
    transcript_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let frame_time = Instant::now();
    let mut pending_controls_embedded = false;
    let tool_timeline_animation = state
        .turn_running
        .then(|| animation_frame_ms(state.turn_started_at.unwrap_or(frame_time), frame_time));

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
            let active_tool_entry = state.turn_running
                && index + 1 == state.transcript.len()
                && entry_kind_from_cell(entry) == TranscriptEntryKind::ShellSummary;
            if index > 0 {
                lines.push(Line::raw(""));
                if entry_kind_from_cell(entry) == TranscriptEntryKind::UserPrompt {
                    lines.push(turn_divider(transcript_width));
                    lines.push(Line::raw(""));
                }
            }
            lines.extend(format_transcript_cell_with_mode(
                entry,
                state.show_tool_details,
                (state.turn_running && index + 1 == state.transcript.len())
                    .then_some(tool_timeline_animation)
                    .flatten(),
            ));
            if active_tool_entry {
                if let Some(embedded) =
                    pending_control_embedded_lines(state, tool_timeline_animation)
                {
                    pending_controls_embedded = true;
                    lines.extend(embedded);
                } else if let Some(bridge) =
                    pending_control_picker_embedded_lines(state, tool_timeline_animation)
                {
                    pending_controls_embedded = true;
                    lines.extend(bridge);
                }
            }
        }
    }

    if !pending_controls_embedded {
        let queued_entry = pending_control_timeline_entry(state)
            .or_else(|| pending_control_picker_bridge_entry(state));
        if let Some(entry) = queued_entry {
            let entry = TranscriptEntry::from(entry);
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.extend(format_transcript_cell_with_mode(
                &entry,
                true,
                state
                    .turn_running
                    .then_some(tool_timeline_animation)
                    .flatten(),
            ));
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

fn should_render_transcript_context(title: &str) -> bool {
    matches!(title, "Resume" | "Session" | "Task" | "Agent Session")
}

fn turn_divider(width: u16) -> Line<'static> {
    // Keep user-turn boundaries locked to the live viewport width so the break
    // reads as a full transcript section boundary instead of a floating marker.
    let width = usize::from(width.max(1));
    Line::from(Span::styled("─".repeat(width), Style::default().fg(SUBTLE)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TranscriptEntryKind {
    UserPrompt,
    AssistantMessage,
    ShellSummary,
    SuccessSummary,
    ErrorSummary,
    WarningSummary,
}

pub(super) fn format_transcript_entry(entry: &str) -> Vec<Line<'static>> {
    let Some((marker, accent, body)) = parse_prefixed_entry(entry) else {
        return vec![Line::from(Span::styled(
            entry.to_string(),
            Style::default().fg(TEXT),
        ))];
    };

    let kind = transcript_entry_kind(marker, body);
    let parsed = TranscriptEntry::from(entry.to_string());
    if should_collapse_shell_details(&parsed, true) {
        return render_collapsed_shell_summary(&parsed, marker, accent, kind, None);
    }
    let mut rendered = render_transcript_body(&parsed, marker, kind, None);
    prefix_transcript_marker(&mut rendered, marker, accent, kind);
    rendered
}

fn format_transcript_cell_with_mode(
    entry: &TranscriptEntry,
    show_tool_details: bool,
    animation_frame: Option<u128>,
) -> Vec<Line<'static>> {
    let marker = entry.marker();
    let kind = entry_kind_from_cell(entry);
    let accent = match kind {
        TranscriptEntryKind::AssistantMessage => MUTED,
        TranscriptEntryKind::UserPrompt => USER,
        TranscriptEntryKind::ShellSummary => super::transcript_shell::summary_color(entry.body()),
        TranscriptEntryKind::SuccessSummary => ASSISTANT,
        TranscriptEntryKind::ErrorSummary => ERROR,
        TranscriptEntryKind::WarningSummary => WARN,
    };

    if should_collapse_shell_details(entry, show_tool_details) {
        return render_collapsed_shell_summary(entry, marker, accent, kind, animation_frame);
    }
    let mut rendered = render_transcript_body(entry, marker, kind, animation_frame);
    prefix_transcript_marker(&mut rendered, marker, accent, kind);
    rendered
}

fn render_transcript_body(
    entry: &TranscriptEntry,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Vec<Line<'static>> {
    if matches!(
        kind,
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage
    ) {
        return render_markdown_body(entry.body(), kind);
    }

    let summary = entry
        .shell_summary()
        .expect("non-markdown transcript entries should expose shell summary payloads");
    render_shell_summary_body(&summary.serialized_body(), marker, kind, animation_frame)
}

fn entry_kind_from_cell(entry: &TranscriptEntry) -> TranscriptEntryKind {
    match entry {
        TranscriptEntry::UserPrompt(_) => TranscriptEntryKind::UserPrompt,
        TranscriptEntry::AssistantMessage(_) => TranscriptEntryKind::AssistantMessage,
        TranscriptEntry::ShellSummary(_) => TranscriptEntryKind::ShellSummary,
        TranscriptEntry::SuccessSummary(_) => TranscriptEntryKind::SuccessSummary,
        TranscriptEntry::ErrorSummary(_) => TranscriptEntryKind::ErrorSummary,
        TranscriptEntry::WarningSummary(_) => TranscriptEntryKind::WarningSummary,
    }
}
