use super::super::state::{
    ToolSelectionTarget, TranscriptEntry, TranscriptToolEntry, TranscriptToolStatus, TuiState,
};
use super::transcript_markdown::render_markdown_body;
use super::transcript_shell::{
    animation_frame_ms, live_progress_lines, pending_control_embedded_lines,
    pending_control_picker_bridge_entry, pending_control_picker_embedded_lines,
    pending_control_timeline_entry, prefix_tool_marker, prefix_transcript_marker,
    render_collapsed_shell_summary, render_collapsed_tool_entry, render_shell_summary_entry,
    render_tool_entry, should_collapse_shell_details, should_collapse_tool_details,
};
pub(super) use super::transcript_shell::{
    line_has_visible_content, line_to_plain_text, transcript_body_style,
    transcript_continuation_prefix,
};
use super::view::build_inspector_text;
use super::welcome::build_welcome_lines;
use super::{shared, theme::palette};
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use std::time::Instant;

const TRANSCRIPT_SIDE_PADDING: u16 = 3;
const WELCOME_SIDE_PADDING: u16 = 4;

pub(super) fn render_transcript(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().main_bg)),
        area,
    );
    if state.transcript.is_empty() && !state.turn_running && state.session.queued_commands == 0 {
        let inner = area.inner(Margin {
            vertical: 0,
            horizontal: WELCOME_SIDE_PADDING,
        });
        let lines = build_welcome_lines(state, inner.width, inner.height);
        let empty = Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().main_bg));
        frame.render_widget(empty, inner);
        return;
    }

    let content_width = area
        .width
        .saturating_sub(TRANSCRIPT_SIDE_PADDING.saturating_mul(2));
    let lines = pad_transcript_lines(
        build_transcript_lines_for_width(state, content_width),
        usize::from(TRANSCRIPT_SIDE_PADDING),
    );
    let scroll = shared::clamp_scroll(state.transcript_scroll, lines.len(), area.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(palette().text).bg(palette().main_bg));
    frame.render_widget(transcript, area);
}

#[cfg(test)]
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
            Style::default().fg(palette().muted),
        )));
        lines.push(Line::raw(""));
        lines.extend(build_inspector_text(&state.inspector_title, &state.inspector, None).lines);
        lines.push(Line::raw(""));
    }

    if !state.transcript.is_empty() {
        for (index, entry) in state.transcript.iter().enumerate() {
            let selected = matches!(
                state.tool_selection.as_ref(),
                Some(ToolSelectionTarget::Transcript(selected)) if *selected == index
            );
            if index > 0 {
                lines.push(Line::raw(""));
                if entry_kind_from_cell(entry) == TranscriptEntryKind::UserPrompt {
                    lines.push(turn_divider(transcript_width));
                    lines.push(Line::raw(""));
                }
            }
            let mut cell_lines = format_transcript_cell_with_mode(
                entry,
                state.show_tool_details,
                (state.turn_running && index + 1 == state.transcript.len())
                    .then_some(tool_timeline_animation)
                    .flatten(),
                selected,
            );
            if selected {
                highlight_transcript_cell(&mut cell_lines);
            }
            lines.extend(cell_lines);
        }
    }

    if !state.active_tool_cells.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        for (index, active) in state.active_tool_cells.iter().enumerate() {
            let active_entry = TranscriptEntry::Tool(active.entry.clone());
            let selected = matches!(
                state.tool_selection.as_ref(),
                Some(ToolSelectionTarget::LiveCell(selected)) if selected == &active.cell_id
            );
            lines.extend(format_transcript_cell_with_mode(
                &active_entry,
                state.show_tool_details,
                state
                    .turn_running
                    .then_some(tool_timeline_animation)
                    .flatten(),
                selected,
            ));
            if index + 1 < state.active_tool_cells.len() {
                lines.push(Line::raw(""));
            }
        }
        if let Some(embedded) = pending_control_embedded_lines(state, tool_timeline_animation) {
            pending_controls_embedded = true;
            lines.extend(embedded);
        } else if let Some(bridge) =
            pending_control_picker_embedded_lines(state, tool_timeline_animation)
        {
            pending_controls_embedded = true;
            lines.extend(bridge);
        }
    }

    if !state.active_monitors.is_empty() {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        for (index, active) in state.active_monitors.iter().enumerate() {
            let active_entry = TranscriptEntry::ShellSummary(active.entry.clone());
            lines.extend(format_transcript_cell_with_mode(
                &active_entry,
                state.show_tool_details,
                Some(animation_frame_ms(active.started_at, frame_time)),
                false,
            ));
            if index + 1 < state.active_monitors.len() {
                lines.push(Line::raw(""));
            }
        }
    }

    if !pending_controls_embedded {
        let queued_entry = pending_control_timeline_entry(state)
            .or_else(|| pending_control_picker_bridge_entry(state));
        if let Some(entry) = queued_entry {
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
                false,
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
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(palette().subtle),
    ))
}

fn pad_transcript_lines(lines: Vec<Line<'static>>, padding: usize) -> Vec<Line<'static>> {
    if padding == 0 {
        return lines;
    }
    lines
        .into_iter()
        .map(|mut line| {
            if line_text_is_divider(&line) || line.spans.is_empty() {
                return line;
            }
            line.spans.insert(0, Span::raw(" ".repeat(padding)));
            line
        })
        .collect()
}

fn line_text_is_divider(line: &Line<'static>) -> bool {
    !line.spans.is_empty()
        && line
            .spans
            .iter()
            .all(|span| span.content.chars().all(|ch| ch == '─'))
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

pub(super) fn format_transcript_cell(entry: &TranscriptEntry) -> Vec<Line<'static>> {
    format_transcript_cell_with_mode(entry, true, None, false)
}

fn format_transcript_cell_with_mode(
    entry: &TranscriptEntry,
    show_tool_details: bool,
    animation_frame: Option<u128>,
    selected: bool,
) -> Vec<Line<'static>> {
    let marker = entry.marker();
    let kind = entry_kind_from_cell(entry);
    let accent = entry_accent(entry, kind);

    if should_collapse_tool_details(entry, show_tool_details) {
        return render_collapsed_tool_entry(entry, marker, accent, kind, animation_frame, selected);
    }
    if should_collapse_shell_details(entry, show_tool_details) {
        return render_collapsed_shell_summary(entry, marker, accent, kind, animation_frame);
    }
    let mut rendered = render_transcript_body(entry, marker, kind, animation_frame);
    if let Some(tool) = entry.tool_entry() {
        prefix_tool_marker(&mut rendered, tool, kind, animation_frame);
    } else {
        prefix_transcript_marker(&mut rendered, marker, accent, kind);
    }
    rendered
}

fn highlight_transcript_cell(lines: &mut [Line<'static>]) {
    for line in lines.iter_mut() {
        for span in &mut line.spans {
            span.style = span.style.bg(palette().footer_bg);
        }
    }
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

    if let Some(tool) = entry.tool_entry() {
        return render_tool_entry(tool, marker, kind, animation_frame);
    }

    let summary = entry
        .shell_summary()
        .expect("non-markdown transcript entries should expose shell summary payloads");
    render_shell_summary_entry(summary, marker, kind, animation_frame)
}

fn entry_accent(entry: &TranscriptEntry, kind: TranscriptEntryKind) -> ratatui::style::Color {
    if let Some(tool) = entry.tool_entry() {
        return match kind {
            TranscriptEntryKind::ShellSummary => {
                super::transcript_shell::tool_status_accent(tool.status, tool.completion)
            }
            TranscriptEntryKind::SuccessSummary => palette().assistant,
            TranscriptEntryKind::ErrorSummary => palette().error,
            TranscriptEntryKind::WarningSummary => palette().warn,
            TranscriptEntryKind::AssistantMessage => palette().muted,
            TranscriptEntryKind::UserPrompt => palette().user,
        };
    }

    match kind {
        TranscriptEntryKind::AssistantMessage => palette().muted,
        TranscriptEntryKind::UserPrompt => palette().user,
        TranscriptEntryKind::ShellSummary => entry
            .shell_summary()
            .map(|summary| super::transcript_shell::shell_status_accent(summary.status))
            .unwrap_or_else(|| palette().muted),
        TranscriptEntryKind::SuccessSummary => palette().assistant,
        TranscriptEntryKind::ErrorSummary => palette().error,
        TranscriptEntryKind::WarningSummary => palette().warn,
    }
}

fn entry_kind_from_cell(entry: &TranscriptEntry) -> TranscriptEntryKind {
    match entry {
        TranscriptEntry::UserPrompt(_) => TranscriptEntryKind::UserPrompt,
        TranscriptEntry::AssistantMessage(_) => TranscriptEntryKind::AssistantMessage,
        TranscriptEntry::Tool(tool) => entry_kind_from_tool(tool),
        TranscriptEntry::ShellSummary(_) => TranscriptEntryKind::ShellSummary,
        TranscriptEntry::SuccessSummary(_) => TranscriptEntryKind::SuccessSummary,
        TranscriptEntry::ErrorSummary(_) => TranscriptEntryKind::ErrorSummary,
        TranscriptEntry::WarningSummary(_) => TranscriptEntryKind::WarningSummary,
    }
}

fn entry_kind_from_tool(tool: &TranscriptToolEntry) -> TranscriptEntryKind {
    match tool.status {
        TranscriptToolStatus::Approved => TranscriptEntryKind::SuccessSummary,
        TranscriptToolStatus::Denied
        | TranscriptToolStatus::Failed
        | TranscriptToolStatus::Cancelled => TranscriptEntryKind::ErrorSummary,
        TranscriptToolStatus::Requested
        | TranscriptToolStatus::WaitingApproval
        | TranscriptToolStatus::Running
        | TranscriptToolStatus::Finished => TranscriptEntryKind::ShellSummary,
    }
}
