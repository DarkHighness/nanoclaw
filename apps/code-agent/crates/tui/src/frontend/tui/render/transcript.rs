use super::super::state::{
    ToolSelectionTarget, TranscriptEntry, TranscriptToolEntry, TranscriptToolStatus, TuiState,
};
use super::transcript_markdown::render_markdown_body;
use super::transcript_shell::{
    RenderedTranscriptCell, animation_frame_ms, format_elapsed_duration, live_progress_lines,
    pending_control_embedded_lines, pending_control_picker_bridge_entry,
    pending_control_picker_embedded_lines, pending_control_timeline_entry, prefix_tool_marker,
    prefix_transcript_marker, render_collapsed_shell_summary, render_collapsed_tool_entry,
    render_shell_summary_sections, render_tool_entry_sections, should_collapse_shell_details,
    should_collapse_tool_details,
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
use std::time::Duration;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

const WELCOME_SIDE_PADDING: u16 = 4;
const TRANSCRIPT_CELL_GAP_LINES: usize = 1;
const TRANSCRIPT_TURN_GAP_LINES: usize = 1;
const TRANSCRIPT_TOP_PADDING: u16 = 1;

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

    // The transcript width must match the actual visible pane width. Global
    // string padding shrinks the effective line budget and then `Paragraph`
    // wraps the already-padded text a second time, which causes early wraps
    // and short dividers. Keep spacing in the cell renderer instead.
    let content_area = transcript_content_area(area);
    let lines = build_transcript_lines_for_width(state, content_area.width);
    let scroll = shared::clamp_scroll(state.transcript_scroll, lines.len(), content_area.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, state.transcript_horizontal_scroll))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(palette().text).bg(palette().main_bg));
    frame.render_widget(transcript, content_area);
}

pub(super) fn transcript_content_area(area: Rect) -> Rect {
    if area.height <= TRANSCRIPT_TOP_PADDING {
        area
    } else {
        Rect::new(
            area.x,
            area.y + TRANSCRIPT_TOP_PADDING,
            area.width,
            area.height - TRANSCRIPT_TOP_PADDING,
        )
    }
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
                lines.extend(
                    std::iter::repeat_with(|| Line::raw("")).take(TRANSCRIPT_CELL_GAP_LINES),
                );
                if entry_kind_from_cell(entry) == TranscriptEntryKind::UserPrompt {
                    lines.extend(
                        std::iter::repeat_with(|| Line::raw("")).take(TRANSCRIPT_TURN_GAP_LINES),
                    );
                    lines.push(turn_divider(transcript_width));
                    lines.extend(
                        std::iter::repeat_with(|| Line::raw("")).take(TRANSCRIPT_TURN_GAP_LINES),
                    );
                }
            }
            let cell_lines = format_transcript_cell_with_mode(
                entry,
                state.show_tool_details,
                (state.turn_running && index + 1 == state.transcript.len())
                    .then_some(tool_timeline_animation)
                    .flatten(),
                selected,
            );
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
        if let Some(divider) = long_running_worked_divider(state, transcript_width) {
            lines.push(divider);
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

fn long_running_worked_divider(state: &TuiState, width: u16) -> Option<Line<'static>> {
    let elapsed = state.turn_started_at?.elapsed();
    (elapsed >= Duration::from_secs(60)).then(|| {
        labeled_divider(
            width,
            &format!("Worked {}", format_elapsed_duration(elapsed)),
        )
    })
}

fn labeled_divider(width: u16, label: &str) -> Line<'static> {
    let width = usize::from(width.max(1));
    let label = format!(" {label} ");
    let label_width = UnicodeWidthStr::width(label.as_str());
    if label_width >= width {
        return Line::from(Span::styled(
            label.chars().take(width).collect::<String>(),
            Style::default().fg(palette().muted),
        ));
    }

    let remaining = width - label_width;
    let left = remaining / 2;
    let right = remaining - left;
    Line::from(vec![
        Span::styled("─".repeat(left), Style::default().fg(palette().subtle)),
        Span::styled(label, Style::default().fg(palette().muted)),
        Span::styled("─".repeat(right), Style::default().fg(palette().subtle)),
    ])
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
        return compose_rendered_transcript_cell(
            render_collapsed_tool_entry(entry, marker, accent, kind, animation_frame, selected),
            selected,
            accent,
        );
    }
    if should_collapse_shell_details(entry, show_tool_details) {
        return compose_rendered_transcript_cell(
            render_collapsed_shell_summary(entry, marker, accent, kind, animation_frame),
            selected,
            accent,
        );
    }
    compose_rendered_transcript_cell(
        render_transcript_body(entry, marker, kind, animation_frame),
        selected,
        accent,
    )
}

fn apply_selected_transcript_cell_chrome(
    lines: &mut [Line<'static>],
    accent: ratatui::style::Color,
) {
    let selection_bg = palette().elevated_surface();
    for line in lines.iter_mut() {
        for span in &mut line.spans {
            span.style = span.style.bg(selection_bg);
        }
    }
    if let Some(index) = lines.iter().position(line_has_visible_content) {
        let mut spans = vec![
            Span::styled(
                "▌",
                Style::default()
                    .fg(accent)
                    .bg(selection_bg)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().bg(selection_bg)),
        ];
        spans.extend(lines[index].spans.clone());
        lines[index] = Line::from(spans);
    }
}

fn compose_rendered_transcript_cell(
    cell: RenderedTranscriptCell,
    selected: bool,
    accent: ratatui::style::Color,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    append_cell_section(&mut lines, cell.header, false);
    let has_content = !lines.is_empty();
    append_cell_section(&mut lines, cell.body, has_content);
    let has_content = !lines.is_empty();
    append_cell_section(&mut lines, cell.meta, has_content);
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("")));
    }
    if selected {
        apply_selected_transcript_cell_chrome(&mut lines, accent);
    }
    lines
}

fn append_cell_section(lines: &mut Vec<Line<'static>>, section: Vec<Line<'static>>, spaced: bool) {
    if section.is_empty() {
        return;
    }
    if spaced {
        lines.push(Line::raw(""));
    }
    lines.extend(section);
}

fn render_transcript_body(
    entry: &TranscriptEntry,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> RenderedTranscriptCell {
    if matches!(
        kind,
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage
    ) {
        let accent = entry_accent(entry, kind);
        let mut cell = RenderedTranscriptCell::with_body(render_markdown_body(entry.body(), kind));
        prefix_transcript_marker(&mut cell.body, marker, accent, kind);
        return cell;
    }

    if let Some(tool) = entry.tool_entry() {
        let mut cell = render_tool_entry_sections(tool, marker, kind, animation_frame);
        prefix_tool_marker(&mut cell.header, tool, kind, animation_frame);
        return cell;
    }

    let summary = entry
        .shell_summary()
        .expect("non-markdown transcript entries should expose shell summary payloads");
    let mut cell = render_shell_summary_sections(summary, marker, kind, animation_frame);
    let accent = entry_accent(entry, kind);
    prefix_transcript_marker(&mut cell.header, marker, accent, kind);
    cell
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
