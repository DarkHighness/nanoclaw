use super::super::state::{
    ToolSelectionTarget, TranscriptCellMotionKind, TranscriptCellMotionState, TranscriptEntry,
    TranscriptToolEntry, TranscriptToolStatus, TuiState, preview_text,
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
use std::f32::consts::TAU;
use std::time::Duration;
use std::time::Instant;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

const WELCOME_SIDE_PADDING: u16 = 4;
const TRANSCRIPT_CELL_GAP_LINES: usize = 1;
const TRANSCRIPT_TURN_GAP_LINES: usize = 1;
const TRANSCRIPT_TOP_PADDING: u16 = 1;
const TYPEWRITER_SHIMMER_TRAIL_CHARS: usize = 4;

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
    let wrap_enabled = transcript_wrap_enabled(state);
    let scroll = shared::clamp_scroll(
        state.transcript_scroll,
        rendered_transcript_line_count(&lines, content_area.width, wrap_enabled),
        content_area.height,
    );
    let transcript = build_transcript_paragraph(lines, state, scroll, wrap_enabled);
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
                state.transcript_motion_state(index),
                frame_time,
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
                None,
                frame_time,
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
                None,
                frame_time,
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
                None,
                frame_time,
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

pub(super) fn active_turn_title_for_viewport(
    state: &TuiState,
    transcript_width: u16,
    transcript_height: u16,
) -> Option<String> {
    let (turn_starts, total_lines) = transcript_turn_starts_for_width(state, transcript_width);
    if turn_starts.is_empty() {
        return None;
    }
    let scroll = shared::clamp_scroll(state.transcript_scroll, total_lines, transcript_height);
    turn_starts
        .iter()
        .rev()
        .find(|(line_index, _)| *line_index <= usize::from(scroll))
        .or_else(|| turn_starts.first())
        .map(|(_, prompt)| prompt.clone())
}

fn transcript_turn_starts_for_width(
    state: &TuiState,
    transcript_width: u16,
) -> (Vec<(usize, String)>, usize) {
    // The global top title needs to follow the same turn boundaries the
    // transcript viewport currently shows, including blank spacer rows and live
    // timeline cells. Mirror the render-time line accounting here so scroll
    // position maps back to the correct user prompt anchor.
    let mut total_lines = 0usize;
    let mut turn_starts = Vec::new();
    let frame_time = Instant::now();
    let wrap_enabled = transcript_wrap_enabled(state);
    let mut pending_controls_embedded = false;
    let tool_timeline_animation = state
        .turn_running
        .then(|| animation_frame_ms(state.turn_started_at.unwrap_or(frame_time), frame_time));

    if should_render_transcript_context(&state.inspector_title) && !state.inspector.is_empty() {
        total_lines += 2;
        total_lines += rendered_transcript_line_count(
            &build_inspector_text(&state.inspector_title, &state.inspector, None).lines,
            transcript_width,
            wrap_enabled,
        );
        total_lines += 1;
    }

    if !state.transcript.is_empty() {
        for (index, entry) in state.transcript.iter().enumerate() {
            let selected = matches!(
                state.tool_selection.as_ref(),
                Some(ToolSelectionTarget::Transcript(selected)) if *selected == index
            );
            if index > 0 {
                total_lines += TRANSCRIPT_CELL_GAP_LINES;
                if entry_kind_from_cell(entry) == TranscriptEntryKind::UserPrompt {
                    turn_starts.push((total_lines, preview_text(entry.body(), 72)));
                    total_lines += TRANSCRIPT_TURN_GAP_LINES;
                    total_lines += 1;
                    total_lines += TRANSCRIPT_TURN_GAP_LINES;
                }
            } else if entry_kind_from_cell(entry) == TranscriptEntryKind::UserPrompt {
                turn_starts.push((total_lines, preview_text(entry.body(), 72)));
            }

            let cell_lines = format_transcript_cell_with_mode(
                entry,
                state.show_tool_details,
                (state.turn_running && index + 1 == state.transcript.len())
                    .then_some(tool_timeline_animation)
                    .flatten(),
                selected,
                state.transcript_motion_state(index),
                frame_time,
            );
            total_lines +=
                rendered_transcript_line_count(&cell_lines, transcript_width, wrap_enabled);
        }
    }

    if !state.active_tool_cells.is_empty() {
        if total_lines > 0 {
            total_lines += 1;
        }
        for (index, active) in state.active_tool_cells.iter().enumerate() {
            let active_entry = TranscriptEntry::Tool(active.entry.clone());
            let selected = matches!(
                state.tool_selection.as_ref(),
                Some(ToolSelectionTarget::LiveCell(selected)) if selected == &active.cell_id
            );
            let cell_lines = format_transcript_cell_with_mode(
                &active_entry,
                state.show_tool_details,
                state
                    .turn_running
                    .then_some(tool_timeline_animation)
                    .flatten(),
                selected,
                None,
                frame_time,
            );
            total_lines +=
                rendered_transcript_line_count(&cell_lines, transcript_width, wrap_enabled);
            if index + 1 < state.active_tool_cells.len() {
                total_lines += 1;
            }
        }
        if let Some(embedded) = pending_control_embedded_lines(state, tool_timeline_animation) {
            pending_controls_embedded = true;
            total_lines +=
                rendered_transcript_line_count(&embedded, transcript_width, wrap_enabled);
        } else if let Some(bridge) =
            pending_control_picker_embedded_lines(state, tool_timeline_animation)
        {
            pending_controls_embedded = true;
            total_lines += rendered_transcript_line_count(&bridge, transcript_width, wrap_enabled);
        }
    }

    if !state.active_monitors.is_empty() {
        if total_lines > 0 {
            total_lines += 1;
        }
        for (index, active) in state.active_monitors.iter().enumerate() {
            let active_entry = TranscriptEntry::ShellSummary(active.entry.clone());
            let cell_lines = format_transcript_cell_with_mode(
                &active_entry,
                state.show_tool_details,
                Some(animation_frame_ms(active.started_at, frame_time)),
                false,
                None,
                frame_time,
            );
            total_lines +=
                rendered_transcript_line_count(&cell_lines, transcript_width, wrap_enabled);
            if index + 1 < state.active_monitors.len() {
                total_lines += 1;
            }
        }
    }

    if !pending_controls_embedded {
        let queued_entry = pending_control_timeline_entry(state)
            .or_else(|| pending_control_picker_bridge_entry(state));
        if let Some(entry) = queued_entry {
            if total_lines > 0 {
                total_lines += 1;
            }
            let cell_lines = format_transcript_cell_with_mode(
                &entry,
                true,
                state
                    .turn_running
                    .then_some(tool_timeline_animation)
                    .flatten(),
                false,
                None,
                frame_time,
            );
            total_lines +=
                rendered_transcript_line_count(&cell_lines, transcript_width, wrap_enabled);
        }
    }

    let progress_lines = live_progress_lines(state);
    if !progress_lines.is_empty() {
        if total_lines > 0 {
            total_lines += 1;
        }
        if long_running_worked_divider(state, transcript_width).is_some() {
            total_lines += 2;
        }
        total_lines +=
            rendered_transcript_line_count(&progress_lines, transcript_width, wrap_enabled);
    }

    (turn_starts, total_lines)
}

fn transcript_wrap_enabled(state: &TuiState) -> bool {
    state.transcript_horizontal_scroll == 0
}

fn build_transcript_paragraph(
    lines: Vec<Line<'static>>,
    state: &TuiState,
    scroll: u16,
    wrap_enabled: bool,
) -> Paragraph<'static> {
    let paragraph = Paragraph::new(Text::from(lines))
        .scroll((scroll, state.transcript_horizontal_scroll))
        .alignment(Alignment::Left)
        .style(Style::default().fg(palette().text).bg(palette().main_bg));
    if wrap_enabled {
        paragraph.wrap(Wrap { trim: false })
    } else {
        paragraph
    }
}

fn rendered_transcript_line_count(
    lines: &[Line<'static>],
    width: u16,
    wrap_enabled: bool,
) -> usize {
    if width == 0 {
        return 0;
    }
    if !wrap_enabled {
        return lines.len();
    }
    lines
        .iter()
        .map(|line| wrapped_transcript_line_count(line, width))
        .sum()
}

fn wrapped_transcript_line_count(line: &Line<'static>, width: u16) -> usize {
    if line.spans.is_empty() {
        return 1;
    }

    let mut rendered_lines = 0usize;
    let mut current_width = 0u16;
    let mut pending_word_width = 0u16;
    let mut pending_whitespace_width = 0u16;
    let mut has_content = false;
    let mut non_whitespace_previous = false;

    for ch in line.spans.iter().flat_map(|span| span.content.chars()) {
        let symbol_width = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if symbol_width > width {
            continue;
        }
        let is_whitespace = ch.is_whitespace();
        let word_boundary = non_whitespace_previous && is_whitespace;
        let line_start_overflow = current_width == 0
            && pending_word_width + pending_whitespace_width + symbol_width > width;

        if word_boundary || line_start_overflow {
            current_width = current_width
                .saturating_add(pending_whitespace_width)
                .saturating_add(pending_word_width);
            pending_whitespace_width = 0;
            pending_word_width = 0;
        }

        let line_full = current_width >= width;
        let pending_overflow = symbol_width > 0
            && current_width + pending_whitespace_width + pending_word_width >= width;
        if line_full || pending_overflow {
            rendered_lines += 1;
            current_width = 0;
        }

        if is_whitespace {
            pending_whitespace_width = pending_whitespace_width.saturating_add(symbol_width);
        } else {
            pending_word_width = pending_word_width.saturating_add(symbol_width);
        }

        has_content = true;
        non_whitespace_previous = !is_whitespace;
    }

    if !has_content {
        return 1;
    }

    rendered_lines + usize::from(current_width + pending_whitespace_width + pending_word_width > 0)
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
    format_transcript_cell_with_mode(entry, true, None, false, None, Instant::now())
}

fn format_transcript_cell_with_mode(
    entry: &TranscriptEntry,
    show_tool_details: bool,
    animation_frame: Option<u128>,
    selected: bool,
    motion: Option<&TranscriptCellMotionState>,
    now: Instant,
) -> Vec<Line<'static>> {
    let marker = entry.marker();
    let kind = entry_kind_from_cell(entry);
    let accent = entry_accent(entry, kind);

    if should_collapse_tool_details(entry, show_tool_details) {
        return compose_rendered_transcript_cell(
            render_collapsed_tool_entry(entry, marker, accent, kind, animation_frame, selected),
            selected,
            accent,
            motion,
            now,
        );
    }
    if should_collapse_shell_details(entry, show_tool_details) {
        return compose_rendered_transcript_cell(
            render_collapsed_shell_summary(entry, marker, accent, kind, animation_frame),
            selected,
            accent,
            motion,
            now,
        );
    }
    compose_rendered_transcript_cell(
        render_transcript_body(entry, marker, kind, animation_frame, motion, now),
        selected,
        accent,
        None,
        now,
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
    motion: Option<&TranscriptCellMotionState>,
    now: Instant,
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
    if let Some(motion) = motion {
        apply_transcript_motion_chrome(&mut lines, Some(motion), now);
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
    motion: Option<&TranscriptCellMotionState>,
    now: Instant,
) -> RenderedTranscriptCell {
    if matches!(
        kind,
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage
    ) {
        let accent = entry_accent(entry, kind);
        let mut cell = RenderedTranscriptCell::with_body(render_markdown_body(
            &motion_trimmed_entry_body(entry, motion),
            kind,
        ));
        prefix_transcript_marker(&mut cell.body, marker, accent, kind);
        apply_transcript_motion_chrome(&mut cell.body, motion, now);
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

fn motion_trimmed_entry_body(
    entry: &TranscriptEntry,
    motion: Option<&TranscriptCellMotionState>,
) -> String {
    let Some(motion) = motion else {
        return entry.body().to_string();
    };
    if motion.kind != TranscriptCellMotionKind::Typewriter {
        return entry.body().to_string();
    }
    entry.body().chars().take(motion.visible_chars()).collect()
}

fn apply_transcript_motion_chrome(
    lines: &mut [Line<'static>],
    motion: Option<&TranscriptCellMotionState>,
    now: Instant,
) {
    let Some(motion) = motion else {
        return;
    };
    if motion.kind != TranscriptCellMotionKind::Typewriter
        || motion.visible_chars() == 0
        || motion.visible_chars() >= motion.target_chars
    {
        return;
    }

    let pulse = transcript_motion_pulse(motion, now);
    let bg = blend_color(
        palette().transcript_surface(),
        palette().elevated_surface(),
        0.18 + pulse * 0.16,
    );
    let mut remaining = TYPEWRITER_SHIMMER_TRAIL_CHARS.min(motion.visible_chars());
    for line in lines.iter_mut().rev() {
        if remaining == 0 {
            break;
        }
        remaining = apply_tail_shimmer_to_line(line, remaining, bg);
    }
}

fn transcript_motion_pulse(motion: &TranscriptCellMotionState, now: Instant) -> f32 {
    let anchor = motion.inserted_at;
    let phase = now.duration_since(anchor).as_millis() as f32 / 180.0;
    0.72 + 0.28 * ((phase * TAU).sin().abs())
}

fn apply_tail_shimmer_to_line(
    line: &mut Line<'static>,
    remaining: usize,
    bg: ratatui::style::Color,
) -> usize {
    if remaining == 0 || !line_has_visible_content(line) {
        return remaining;
    }

    let protected_spans = protected_transcript_prefix_span_count(line);
    let mut left = remaining;
    let mut index = line.spans.len();
    while index > protected_spans && left > 0 {
        index -= 1;
        let span = line.spans[index].clone();
        let span_width = span.content.chars().count();
        if span_width == 0 {
            continue;
        }
        let take = span_width.min(left);
        let keep = span_width - take;
        let prefix = span.content.chars().take(keep).collect::<String>();
        let suffix = span.content.chars().skip(keep).collect::<String>();
        let mut replacement = Vec::new();
        if !prefix.is_empty() {
            replacement.push(Span::styled(prefix, span.style));
        }
        if !suffix.is_empty() {
            replacement.push(Span::styled(
                suffix,
                span.style
                    .bg(bg)
                    .add_modifier(ratatui::style::Modifier::DIM),
            ));
        }
        line.spans.splice(index..=index, replacement);
        left -= take;
    }
    left
}

fn protected_transcript_prefix_span_count(line: &Line<'static>) -> usize {
    if line.spans.len() >= 2
        && line.spans[1].content.as_ref() == " "
        && matches!(
            line.spans[0].content.as_ref(),
            "•" | "›" | "✔" | "!" | "⚠" | "✖"
        )
    {
        return 2;
    }
    if !line.spans.is_empty() && line.spans[0].content.as_ref() == "  " {
        return 1;
    }
    0
}

fn blend_color(
    base: ratatui::style::Color,
    target: ratatui::style::Color,
    amount: f32,
) -> ratatui::style::Color {
    let amount = amount.clamp(0.0, 1.0);
    let (base_r, base_g, base_b) = color_channels(base);
    let (target_r, target_g, target_b) = color_channels(target);
    ratatui::style::Color::Rgb(
        interpolate_channel(base_r, target_r, amount),
        interpolate_channel(base_g, target_g, amount),
        interpolate_channel(base_b, target_b, amount),
    )
}

fn interpolate_channel(base: u8, target: u8, amount: f32) -> u8 {
    let blended = base as f32 + (target as f32 - base as f32) * amount;
    blended.round().clamp(0.0, 255.0) as u8
}

fn color_channels(color: ratatui::style::Color) -> (u8, u8, u8) {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        ratatui::style::Color::Black => (0, 0, 0),
        ratatui::style::Color::Red => (255, 0, 0),
        ratatui::style::Color::Green => (0, 255, 0),
        ratatui::style::Color::Yellow => (255, 255, 0),
        ratatui::style::Color::Blue => (0, 0, 255),
        ratatui::style::Color::Magenta => (255, 0, 255),
        ratatui::style::Color::Cyan => (0, 255, 255),
        ratatui::style::Color::Gray => (128, 128, 128),
        ratatui::style::Color::DarkGray => (64, 64, 64),
        ratatui::style::Color::LightRed => (255, 102, 102),
        ratatui::style::Color::LightGreen => (102, 255, 102),
        ratatui::style::Color::LightYellow => (255, 255, 102),
        ratatui::style::Color::LightBlue => (102, 102, 255),
        ratatui::style::Color::LightMagenta => (255, 102, 255),
        ratatui::style::Color::LightCyan => (102, 255, 255),
        ratatui::style::Color::White => (255, 255, 255),
        ratatui::style::Color::Indexed(index) => (index, index, index),
        ratatui::style::Color::Reset => color_channels(palette().text),
    }
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
