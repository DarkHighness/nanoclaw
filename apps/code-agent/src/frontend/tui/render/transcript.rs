use super::super::state::TuiState;
use super::transcript_markdown::render_markdown_body;
use super::transcript_shell::{
    animation_frame_ms, live_progress_lines, pending_control_embedded_lines,
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
        let lines = build_welcome_lines(state, inner.height);
        let empty = Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(MAIN_BG));
        frame.render_widget(empty, inner);
        return;
    }

    let lines = build_transcript_lines(state);
    let scroll = shared::clamp_scroll(state.transcript_scroll, lines.len(), inner.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(transcript, inner);
}

pub(super) fn build_transcript_lines(state: &TuiState) -> Vec<Line<'static>> {
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
                && transcript_entry_kind_for_entry(entry)
                    == Some(TranscriptEntryKind::ShellSummary);
            let entry = entry.clone();
            if index > 0 {
                lines.push(Line::raw(""));
                if transcript_entry_kind_for_entry(&entry) == Some(TranscriptEntryKind::UserPrompt)
                {
                    lines.push(turn_divider());
                    lines.push(Line::raw(""));
                }
            }
            lines.extend(format_transcript_entry_with_mode(
                &entry,
                state.show_tool_details,
                (state.turn_running && index + 1 == state.transcript.len())
                    .then_some(tool_timeline_animation)
                    .flatten(),
            ));
            if active_tool_entry && let Some(embedded) = pending_control_embedded_lines(state) {
                pending_controls_embedded = true;
                lines.extend(embedded);
            }
        }
    }

    if !pending_controls_embedded && let Some(entry) = pending_control_timeline_entry(state) {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.extend(format_transcript_entry_with_mode(
            &entry,
            true,
            state
                .turn_running
                .then_some(tool_timeline_animation)
                .flatten(),
        ));
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

fn turn_divider() -> Line<'static> {
    Line::from(Span::styled("┈".repeat(12), Style::default().fg(SUBTLE)))
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
    format_transcript_entry_with_mode(entry, true, None)
}

fn format_transcript_entry_with_mode(
    entry: &str,
    show_tool_details: bool,
    animation_frame: Option<u128>,
) -> Vec<Line<'static>> {
    let Some((marker, accent, body)) = parse_prefixed_entry(entry) else {
        return vec![Line::from(Span::styled(
            entry.to_string(),
            Style::default().fg(TEXT),
        ))];
    };

    let kind = transcript_entry_kind(marker, body);
    if should_collapse_shell_details(kind, body, show_tool_details) {
        return render_collapsed_shell_summary(marker, accent, body, kind, animation_frame);
    }
    let mut rendered = render_transcript_body(body, marker, kind, animation_frame);
    prefix_transcript_marker(&mut rendered, marker, accent, kind);
    rendered
}

fn transcript_entry_kind_for_entry(entry: &str) -> Option<TranscriptEntryKind> {
    let (marker, _, body) = parse_prefixed_entry(entry)?;
    Some(transcript_entry_kind(marker, body))
}

fn render_transcript_body(
    body: &str,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Vec<Line<'static>> {
    if matches!(
        kind,
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage
    ) {
        return render_markdown_body(body, kind);
    }

    render_shell_summary_body(body, marker, kind, animation_frame)
}
