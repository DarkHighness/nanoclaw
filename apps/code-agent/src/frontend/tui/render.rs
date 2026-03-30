mod chrome;
mod picker;
mod theme;
mod welcome;

use super::approval::ApprovalPrompt;
use super::commands::slash_command_hint;
use super::state::{MainPaneMode, TuiState, preview_text};
use crate::backend::preview_id;
use chrome::{
    approval_band_height, approval_preview_lines, build_approval_text, build_side_rail_lines,
    format_footer_context, render_approval_band, render_composer, render_status_line,
    should_render_side_rail, side_rail_width, status_color,
};
use picker::{build_command_hint_text, command_hint_height, render_command_hint_band};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui_core::layout::Alignment as CoreAlignment;
use ratatui_core::style::{Color as CoreColor, Modifier as CoreModifier, Style as CoreStyle};
use ratatui_core::text::{Line as CoreLine, Span as CoreSpan};
use theme::*;
use tui_markdown::{Options as MarkdownOptions, from_str_with_options};
use welcome::build_welcome_lines;

pub(crate) fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    approval: Option<&ApprovalPrompt>,
) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let approval_height = approval.map(approval_band_height);
    let command_hint = approval
        .is_none()
        .then(|| slash_command_hint(&state.input, state.command_completion_index))
        .flatten();
    let command_hint_height = command_hint.as_ref().map(command_hint_height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            approval_height,
            command_hint_height,
        ))
        .split(area);
    let mut next_index = 0;
    let main_area = vertical[next_index];
    next_index += 1;
    let approval_area = approval_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let command_hint_area = command_hint_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let composer_area = vertical[next_index];
    let status_area = vertical[next_index + 1];

    if should_render_side_rail(state, main_area) {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(10),
                Constraint::Length(side_rail_width(main_area.width)),
            ])
            .split(main_area);
        render_main_pane(frame, horizontal[0], state);
        render_side_rail(frame, horizontal[1], state);
    } else {
        render_main_pane(frame, main_area, state);
    }
    if let Some(approval) = approval {
        render_approval_band(frame, approval_area.expect("approval area"), approval);
    }
    if let Some(command_hint) = command_hint.as_ref() {
        render_command_hint_band(
            frame,
            command_hint_area.expect("command hint area"),
            command_hint,
        );
    }
    render_composer(frame, composer_area, state);
    render_status_line(frame, status_area, state);

    let composer_inner = composer_inner_area(composer_area);
    let prefix_width = 2_u16;
    frame.set_cursor_position(Position::new(
        composer_inner
            .x
            .saturating_add(prefix_width)
            .saturating_add(state.input.chars().count() as u16),
        composer_inner.y,
    ));
}

fn render_main_pane(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    match state.main_pane {
        MainPaneMode::Transcript => render_transcript(frame, area, state),
        MainPaneMode::View => render_main_view(frame, area, state),
    }
}

fn render_transcript(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
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
    let scroll = clamp_scroll(state.transcript_scroll, lines.len(), inner.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(transcript, inner);
}

fn render_main_view(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    let title = if state.inspector_title.is_empty() {
        "View"
    } else {
        state.inspector_title.as_str()
    };
    let scroll = clamp_scroll(
        state.inspector_scroll,
        state.inspector.len().saturating_add(2).max(1),
        inner.height,
    );
    let mut lines = Vec::new();
    if should_render_view_title(title, &state.inspector) {
        lines.push(Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(MUTED),
        )));
        lines.push(Line::raw(""));
    }
    lines.extend(build_inspector_text(title, &state.inspector).lines);
    let view = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(view, inner);
}

fn render_side_rail(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    frame.render_widget(Block::default().style(Style::default().bg(MAIN_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let rail = Paragraph::new(Text::from(build_side_rail_lines(state)))
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(rail, inner);
}

fn composer_inner_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    })
}

fn bottom_layout_constraints(
    approval_height: Option<u16>,
    command_hint_height: Option<u16>,
) -> Vec<Constraint> {
    let mut constraints = vec![Constraint::Min(10)];
    if let Some(height) = approval_height {
        constraints.push(Constraint::Length(height));
    }
    if let Some(height) = command_hint_height {
        constraints.push(Constraint::Length(height));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    constraints
}

fn build_transcript_lines(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

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
            if index > 0 {
                lines.push(Line::raw(""));
                if transcript_entry_kind_for_entry(entry) == Some(TranscriptEntryKind::UserPrompt) {
                    lines.push(turn_divider());
                    lines.push(Line::raw(""));
                }
            }
            lines.extend(format_transcript_entry_with_mode(
                entry,
                state.show_tool_details,
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

fn turn_divider() -> Line<'static> {
    Line::from(Span::styled("┈".repeat(12), Style::default().fg(SUBTLE)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptEntryKind {
    UserPrompt,
    AssistantMessage,
    ShellSummary,
    SuccessSummary,
    ErrorSummary,
    WarningSummary,
}

fn format_transcript_entry(entry: &str) -> Vec<Line<'static>> {
    format_transcript_entry_with_mode(entry, true)
}

fn format_transcript_entry_with_mode(entry: &str, show_tool_details: bool) -> Vec<Line<'static>> {
    let Some((marker, accent, body)) = parse_prefixed_entry(entry) else {
        return vec![Line::from(Span::styled(
            entry.to_string(),
            Style::default().fg(TEXT),
        ))];
    };

    let kind = transcript_entry_kind(marker, body);
    if should_collapse_shell_details(kind, body, show_tool_details) {
        return render_collapsed_shell_summary(marker, accent, body, kind);
    }
    let mut rendered = render_transcript_body(body, marker, kind);
    prefix_transcript_marker(&mut rendered, marker, accent, kind);
    rendered
}

fn should_collapse_shell_details(
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

fn render_collapsed_shell_summary(
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

    let mut rendered = render_transcript_body(headline, marker, kind);
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

fn transcript_entry_kind_for_entry(entry: &str) -> Option<TranscriptEntryKind> {
    let (marker, _, body) = parse_prefixed_entry(entry)?;
    Some(transcript_entry_kind(marker, body))
}

fn transcript_entry_kind(marker: &str, body: &str) -> TranscriptEntryKind {
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

fn render_transcript_body(
    body: &str,
    marker: &str,
    kind: TranscriptEntryKind,
) -> Vec<Line<'static>> {
    if matches!(
        kind,
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage
    ) {
        return render_markdown_body(body, kind);
    }

    return render_shell_summary_body(body, marker, kind);
}

fn render_shell_summary_body(
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

fn render_markdown_body(body: &str, kind: TranscriptEntryKind) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    let mut text_chunk = Vec::new();
    let mut is_first_visible = true;
    let mut lines = body.lines();

    while let Some(raw_line) = lines.next() {
        if let Some(language) = raw_line.trim_start().strip_prefix("```") {
            if !text_chunk.is_empty() {
                let chunk = render_markdown_chunk(&text_chunk.join("\n"), kind, is_first_visible);
                if chunk.iter().any(line_has_visible_content) {
                    is_first_visible = false;
                }
                rendered.extend(chunk);
                text_chunk.clear();
            }

            let mut code_lines = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code_lines.push(code_line);
            }
            let block = render_shell_code_block(
                language.trim(),
                &code_lines.join("\n"),
                kind,
                is_first_visible,
            );
            if block.iter().any(line_has_visible_content) {
                is_first_visible = false;
            }
            rendered.extend(block);
            continue;
        }
        text_chunk.push(raw_line);
    }

    if !text_chunk.is_empty() {
        rendered.extend(render_markdown_chunk(
            &text_chunk.join("\n"),
            kind,
            is_first_visible,
        ));
    }

    if rendered.is_empty() {
        vec![Line::from(Span::raw(""))]
    } else {
        rendered
    }
}

fn render_markdown_chunk(
    body: &str,
    kind: TranscriptEntryKind,
    is_first_visible: bool,
) -> Vec<Line<'static>> {
    let mut compact = render_markdown_lines(body);
    apply_markdown_prefixes(&mut compact, kind, !is_first_visible);
    compact
}

fn render_shell_code_block(
    language: &str,
    code: &str,
    kind: TranscriptEntryKind,
    is_first_visible: bool,
) -> Vec<Line<'static>> {
    let fence = if language.is_empty() {
        "text"
    } else {
        language
    };
    let mut rendered = vec![code_block_label_line(fence, kind, is_first_visible)];
    let mut compact = render_markdown_lines(&format!("```{fence}\n{code}\n```"));
    // The label line already occupies the first visible slot for this block, so the
    // rendered code lines should always behave like continuation lines.
    apply_markdown_prefixes(&mut compact, kind, true);
    rendered.extend(compact);
    if rendered.iter().any(line_has_visible_content) {
        rendered
    } else {
        vec![Line::from(Span::raw(""))]
    }
}

fn code_block_label_line(
    language: &str,
    kind: TranscriptEntryKind,
    is_first_visible: bool,
) -> Line<'static> {
    line_with_indent(
        kind,
        is_first_visible,
        vec![
            Span::styled("···", Style::default().fg(SUBTLE)),
            Span::raw(" "),
            Span::styled(
                language.to_string(),
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
        ],
    )
}

fn render_markdown_lines(body: &str) -> Vec<Line<'static>> {
    let options = MarkdownOptions::new(NanoclawMarkdownStyleSheet);
    let rendered = from_str_with_options(body, &options);
    trim_blank_markdown_lines(
        rendered
            .lines
            .into_iter()
            .filter(|line| !is_markdown_fence_line(line))
            .map(own_line)
            .map(normalize_markdown_line)
            .collect::<Vec<_>>(),
    )
}

fn is_markdown_fence_line(line: &CoreLine<'_>) -> bool {
    core_line_to_plain_text(line)
        .trim_start()
        .starts_with("```")
}

fn own_line(line: CoreLine<'_>) -> Line<'static> {
    let mut owned = Line::from(line.spans.into_iter().map(own_span).collect::<Vec<_>>());
    owned.style = style_from_core(line.style);
    owned.alignment = line.alignment.map(alignment_from_core);
    owned
}

fn own_span(span: CoreSpan<'_>) -> Span<'static> {
    Span::styled(span.content.into_owned(), style_from_core(span.style))
}

fn normalize_markdown_line(mut line: Line<'static>) -> Line<'static> {
    let plain = line_to_plain_text(&line);
    if plain.is_empty() {
        return line;
    }

    let heading_level = plain.chars().take_while(|char| *char == '#').count();
    if heading_level > 0
        && plain.chars().nth(heading_level) == Some(' ')
        && line.style.add_modifier.contains(Modifier::BOLD)
    {
        strip_line_prefix_chars(&mut line, heading_level + 1);
        return line;
    }

    if line.style.fg == Some(MUTED) && (plain.starts_with("> ") || plain == ">") {
        let prefix_len = usize::from(plain.starts_with("> ")) + 1;
        strip_line_prefix_chars(&mut line, prefix_len);
        line.spans.insert(
            0,
            Span::styled("│ ".to_string(), Style::default().fg(SUBTLE)),
        );
    }

    line
}

fn strip_line_prefix_chars(line: &mut Line<'static>, prefix_len: usize) {
    let mut remaining = prefix_len;
    while remaining > 0 && !line.spans.is_empty() {
        let span_len = line.spans[0].content.chars().count();
        if span_len <= remaining {
            remaining -= span_len;
            line.spans.remove(0);
            continue;
        }
        let trimmed = line.spans[0]
            .content
            .chars()
            .skip(remaining)
            .collect::<String>();
        line.spans[0].content = trimmed.into();
        remaining = 0;
    }
}

fn style_from_core(style: CoreStyle) -> Style {
    Style {
        fg: style.fg.map(color_from_core),
        bg: style.bg.map(color_from_core),
        underline_color: None,
        add_modifier: modifier_from_core(style.add_modifier),
        sub_modifier: modifier_from_core(style.sub_modifier),
    }
}

fn color_from_core(color: CoreColor) -> Color {
    match color {
        CoreColor::Reset => Color::Reset,
        CoreColor::Black => Color::Black,
        CoreColor::Red => Color::Red,
        CoreColor::Green => Color::Green,
        CoreColor::Yellow => Color::Yellow,
        CoreColor::Blue => Color::Blue,
        CoreColor::Magenta => Color::Magenta,
        CoreColor::Cyan => Color::Cyan,
        CoreColor::Gray => Color::Gray,
        CoreColor::DarkGray => Color::DarkGray,
        CoreColor::LightRed => Color::LightRed,
        CoreColor::LightGreen => Color::LightGreen,
        CoreColor::LightYellow => Color::LightYellow,
        CoreColor::LightBlue => Color::LightBlue,
        CoreColor::LightMagenta => Color::LightMagenta,
        CoreColor::LightCyan => Color::LightCyan,
        CoreColor::White => Color::White,
        CoreColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        CoreColor::Indexed(index) => Color::Indexed(index),
    }
}

fn modifier_from_core(modifier: CoreModifier) -> Modifier {
    Modifier::from_bits_truncate(modifier.bits())
}

fn alignment_from_core(alignment: CoreAlignment) -> Alignment {
    match alignment {
        CoreAlignment::Left => Alignment::Left,
        CoreAlignment::Center => Alignment::Center,
        CoreAlignment::Right => Alignment::Right,
    }
}

fn core_line_to_plain_text(line: &CoreLine<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn trim_blank_markdown_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let start = lines
        .iter()
        .position(line_has_visible_content)
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(line_has_visible_content)
        .map(|index| index + 1)
        .unwrap_or(start);
    lines[start..end].to_vec()
}

fn apply_markdown_prefixes(
    lines: &mut [Line<'static>],
    kind: TranscriptEntryKind,
    prefix_first_visible: bool,
) {
    let Some(first_visible_index) = lines.iter().position(line_has_visible_content) else {
        return;
    };
    for (index, line) in lines.iter_mut().enumerate() {
        if !line_has_visible_content(line) {
            continue;
        }
        if index < first_visible_index || (index == first_visible_index && !prefix_first_visible) {
            continue;
        }
        line.spans.insert(0, transcript_continuation_prefix(kind));
    }
}

fn prefix_transcript_marker(
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

fn line_has_visible_content(line: &Line<'static>) -> bool {
    !line_to_plain_text(line).trim().is_empty()
}

fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn render_transcript_body_line(
    raw_line: &str,
    marker: &str,
    kind: TranscriptEntryKind,
    in_code: bool,
    is_first_visible: bool,
) -> Line<'static> {
    if raw_line.trim().is_empty() {
        return Line::from(Span::raw(""));
    }
    if let Some(detail) = raw_line.strip_prefix("  └ ") {
        return Line::from(vec![
            Span::styled("  └ ", Style::default().fg(SUBTLE)),
            Span::styled(detail.to_string(), Style::default().fg(MUTED)),
        ]);
    }
    if let Some(detail) = raw_line.strip_prefix("    ") {
        return Line::from(vec![
            Span::raw("    "),
            Span::styled(detail.to_string(), Style::default().fg(MUTED)),
        ]);
    }
    if in_code {
        return line_with_indent(kind, is_first_visible, vec![code_span(raw_line)]);
    }
    if let Some((level, heading)) = markdown_heading(raw_line) {
        return line_with_indent(
            kind,
            is_first_visible,
            vec![Span::styled(
                heading.to_string(),
                markdown_heading_style(level),
            )],
        );
    }
    if is_markdown_rule(raw_line) {
        return line_with_indent(
            kind,
            is_first_visible,
            vec![Span::styled("┈".repeat(18), Style::default().fg(SUBTLE))],
        );
    }
    if let Some(rest) = markdown_quote(raw_line) {
        let mut spans = vec![
            Span::styled("│", Style::default().fg(SUBTLE)),
            Span::raw(" "),
        ];
        spans.extend(markdown_inline_spans(
            rest,
            markdown_body_style(kind, Style::default().fg(MUTED)),
        ));
        return line_with_indent(kind, is_first_visible, spans);
    }
    if let Some(rest) = raw_line
        .strip_prefix("- ")
        .or_else(|| raw_line.strip_prefix("* "))
    {
        let mut spans = vec![
            Span::styled("-", Style::default().fg(MUTED)),
            Span::raw(" "),
        ];
        spans.extend(markdown_inline_spans(
            rest,
            transcript_body_style(marker, kind, rest),
        ));
        return line_with_indent(kind, is_first_visible, spans);
    }
    if let Some((ordinal, rest)) = markdown_ordered_item(raw_line) {
        let mut spans = vec![
            Span::styled(format!("{ordinal}."), Style::default().fg(MUTED)),
            Span::raw(" "),
        ];
        spans.extend(markdown_inline_spans(
            rest,
            transcript_body_style(marker, kind, rest),
        ));
        return line_with_indent(kind, is_first_visible, spans);
    }
    line_with_indent(
        kind,
        is_first_visible,
        markdown_inline_spans(raw_line, transcript_body_style(marker, kind, raw_line)),
    )
}

fn line_with_indent(
    kind: TranscriptEntryKind,
    is_first_visible: bool,
    mut spans: Vec<Span<'static>>,
) -> Line<'static> {
    if !is_first_visible {
        spans.insert(0, transcript_continuation_prefix(kind));
    }
    Line::from(spans)
}

fn transcript_continuation_prefix(kind: TranscriptEntryKind) -> Span<'static> {
    match kind {
        TranscriptEntryKind::ShellSummary
        | TranscriptEntryKind::SuccessSummary
        | TranscriptEntryKind::ErrorSummary
        | TranscriptEntryKind::WarningSummary => Span::styled("    ", Style::default().fg(SUBTLE)),
        _ => Span::raw("  "),
    }
}

fn markdown_heading(raw_line: &str) -> Option<(usize, &str)> {
    let trimmed = raw_line.trim_start();
    let level = trimmed.chars().take_while(|char| *char == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let heading = trimmed[level..].trim_start();
    (!heading.is_empty()).then_some((level, heading))
}

fn markdown_heading_style(level: usize) -> Style {
    let style = Style::default().fg(HEADER).add_modifier(Modifier::BOLD);
    if level <= 2 { style } else { style.fg(TEXT) }
}

fn is_markdown_rule(raw_line: &str) -> bool {
    let trimmed = raw_line.trim();
    trimmed.len() >= 3
        && matches!(trimmed.chars().next(), Some('-' | '*' | '_'))
        && trimmed.chars().all(|char| matches!(char, '-' | '*' | '_'))
}

fn markdown_quote(raw_line: &str) -> Option<&str> {
    raw_line.trim_start().strip_prefix("> ").map(str::trim_end)
}

fn markdown_ordered_item(raw_line: &str) -> Option<(usize, &str)> {
    let trimmed = raw_line.trim_start();
    let (digits, rest) = trimmed.split_once(". ")?;
    let ordinal = digits.parse::<usize>().ok()?;
    Some((ordinal, rest))
}

fn markdown_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix('`')
            && let Some(end) = rest.find('`')
        {
            let code = &rest[..end];
            if !code.is_empty() {
                spans.push(Span::styled(code.to_string(), Style::default().fg(USER)));
            }
            remaining = &rest[end + 1..];
            continue;
        }

        if let Some(rest) = remaining.strip_prefix("**")
            && let Some(end) = rest.find("**")
        {
            let value = &rest[..end];
            if !value.is_empty() {
                spans.push(Span::styled(
                    value.to_string(),
                    base_style.add_modifier(Modifier::BOLD),
                ));
            }
            remaining = &rest[end + 2..];
            continue;
        }

        if let Some(rest) = remaining.strip_prefix('*')
            && let Some(end) = rest.find('*')
        {
            let value = &rest[..end];
            if !value.is_empty() {
                spans.push(Span::styled(
                    value.to_string(),
                    base_style.add_modifier(Modifier::ITALIC),
                ));
            }
            remaining = &rest[end + 1..];
            continue;
        }

        if let Some(rest) = remaining.strip_prefix('[')
            && let Some(label_end) = rest.find("](")
            && let Some(url_end) = rest[label_end + 2..].find(')')
        {
            let label = &rest[..label_end];
            let url = &rest[label_end + 2..label_end + 2 + url_end];
            if !label.is_empty() {
                spans.push(Span::styled(
                    label.to_string(),
                    base_style.add_modifier(Modifier::UNDERLINED),
                ));
            }
            if !url.is_empty() {
                spans.push(Span::styled(
                    format!(" ({url})"),
                    Style::default().fg(SUBTLE),
                ));
            }
            remaining = &rest[label_end + 2 + url_end + 1..];
            continue;
        }

        let next_index = markdown_token_index(remaining).unwrap_or(remaining.len());
        let (plain, rest) = remaining.split_at(next_index);
        if !plain.is_empty() {
            spans.push(Span::styled(plain.to_string(), base_style));
        }
        if rest.is_empty() {
            break;
        }
        let mut chars = rest.chars();
        let next = chars.next().expect("rest is not empty");
        spans.push(Span::styled(next.to_string(), base_style));
        remaining = chars.as_str();
    }

    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), base_style));
    }

    spans
}

fn markdown_token_index(text: &str) -> Option<usize> {
    ["`", "*", "["]
        .into_iter()
        .filter_map(|token| text.find(token))
        .min()
}

fn parse_prefixed_entry(entry: &str) -> Option<(&'static str, Color, &str)> {
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

fn transcript_body_style(marker: &str, kind: TranscriptEntryKind, line: &str) -> Style {
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

fn markdown_body_style(kind: TranscriptEntryKind, base: Style) -> Style {
    match kind {
        TranscriptEntryKind::AssistantMessage | TranscriptEntryKind::UserPrompt => base.fg(TEXT),
        TranscriptEntryKind::ShellSummary => base.fg(MUTED),
        TranscriptEntryKind::SuccessSummary => base.fg(ASSISTANT),
        TranscriptEntryKind::ErrorSummary => base.fg(ERROR),
        TranscriptEntryKind::WarningSummary => base.fg(WARN),
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

fn live_progress_lines(state: &TuiState) -> Vec<Line<'static>> {
    if state.turn_running {
        if state.active_tool_label.is_some() {
            if state.session.queued_commands == 0 {
                return Vec::new();
            }
            return vec![Line::from(vec![
                Span::styled("+", Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(
                    format!(
                        "{} queued while the current tool runs",
                        state.session.queued_commands
                    ),
                    Style::default().fg(MUTED),
                ),
            ])];
        }
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
            Span::styled(
                preview_text(&status, 56),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ];
        if state.session.queued_commands > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
            spans.push(Span::styled(
                format!("{} queued", state.session.queued_commands),
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

fn live_progress_summary(state: &TuiState) -> String {
    match state.status.as_str() {
        "Waiting for approval" => "Waiting for approval".to_string(),
        status if !status.is_empty() => status.to_string(),
        _ => "Working".to_string(),
    }
}

fn code_span(line: &str) -> Span<'static> {
    let trimmed = line.trim_start();
    let style = if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Style::default().fg(ASSISTANT)
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Style::default().fg(ERROR)
    } else if trimmed.starts_with("@@") {
        Style::default().fg(USER)
    } else {
        Style::default().fg(TEXT)
    };
    Span::styled(line.to_string(), style)
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

fn build_key_value_text(lines: &[String]) -> Text<'static> {
    let mut rendered = Vec::new();
    for line in lines {
        if let Some(title) = line.strip_prefix("## ") {
            if !rendered.is_empty() {
                rendered.push(Line::raw(""));
            }
            rendered.push(Line::from(Span::styled(
                title.to_string(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )));
            continue;
        }
        if is_shell_summary_line(line) {
            rendered.extend(render_shell_summary_line(line));
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
        } else if let Some((marker, accent, body)) = parse_prefixed_entry(line) {
            let kind = transcript_entry_kind(marker, body);
            rendered.push(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(body.to_string(), transcript_body_style(marker, kind, body)),
            ]));
        } else if let Some(detail) = line.strip_prefix("  └ ") {
            rendered.push(Line::from(vec![
                Span::styled("  └ ", Style::default().fg(SUBTLE)),
                Span::styled(detail.to_string(), Style::default().fg(MUTED)),
            ]));
        } else if let Some(detail) = line.strip_prefix("    ") {
            rendered.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(detail.to_string(), Style::default().fg(MUTED)),
            ]));
        } else if let Some(rest) = line.strip_prefix("  ") {
            rendered.push(Line::from(vec![
                Span::raw("  "),
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

fn is_shell_summary_line(line: &str) -> bool {
    parse_prefixed_entry(line).is_some()
        || line.starts_with("  └ ")
        || line.starts_with("    ")
        || line.starts_with("- ")
        || line.starts_with("* ")
}

fn render_shell_summary_line(line: &str) -> Vec<Line<'static>> {
    if parse_prefixed_entry(line).is_some() {
        format_transcript_entry(line)
    } else {
        vec![render_transcript_body_line(
            line,
            "•",
            TranscriptEntryKind::ShellSummary,
            false,
            false,
        )]
    }
}

fn build_inspector_text(title: &str, lines: &[String]) -> Text<'static> {
    if is_command_palette_title(title) {
        build_command_palette_text(lines)
    } else if is_collection_inspector(title) {
        build_collection_text(title, lines)
    } else {
        build_key_value_text(lines)
    }
}

fn build_command_palette_text(lines: &[String]) -> Text<'static> {
    let mut rendered = Vec::new();
    for line in lines {
        if let Some(section) = line.strip_prefix("## ") {
            if !rendered.is_empty() {
                rendered.push(Line::raw(""));
            }
            rendered.push(Line::from(Span::styled(
                section.to_string(),
                Style::default().fg(MUTED),
            )));
            continue;
        }
        if line.starts_with("No ") {
            rendered.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(SUBTLE),
            )));
            continue;
        }
        if let Some((command, summary)) = line.split_once("  ") {
            rendered.push(Line::from(vec![
                Span::styled("›", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(
                    command.to_string(),
                    Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(summary.to_string(), Style::default().fg(MUTED)),
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

fn build_collection_text(title: &str, lines: &[String]) -> Text<'static> {
    let accent = inspector_accent(title);
    let mut rendered = Vec::new();
    for line in lines {
        if let Some(section) = line.strip_prefix("## ") {
            rendered.push(Line::from(Span::styled(
                section.to_string(),
                Style::default().fg(MUTED),
            )));
            continue;
        }
        if line.starts_with("No ") || line.starts_with("no ") {
            rendered.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(SUBTLE),
            )));
            continue;
        }
        if is_shell_summary_block(line) {
            rendered.extend(render_collection_summary_block(line, accent));
            continue;
        }
        let (primary, secondary) = split_list_entry(line);
        rendered.push(collection_line(primary, secondary, accent));
    }
    Text::from(rendered)
}

fn render_collection_summary_block(entry: &str, accent: Color) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    for (index, raw_line) in entry.lines().enumerate() {
        if index == 0
            && let Some((_, _, body)) = parse_prefixed_entry(raw_line)
        {
            rendered.push(Line::from(vec![
                Span::styled(
                    "›",
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    body.to_string(),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
            ]));
            continue;
        }
        if let Some(detail) = raw_line.strip_prefix("  └ ") {
            rendered.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(detail.to_string(), Style::default().fg(MUTED)),
            ]));
            continue;
        }
        if let Some(detail) = raw_line.strip_prefix("    ") {
            rendered.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(detail.to_string(), Style::default().fg(SUBTLE)),
            ]));
            continue;
        }
        rendered.extend(render_shell_summary_line(raw_line));
    }
    rendered
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
        "Tool Catalog"
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

fn is_shell_summary_block(entry: &str) -> bool {
    entry
        .lines()
        .all(|line| line.trim().is_empty() || is_shell_summary_line(line))
}

fn is_command_palette_title(title: &str) -> bool {
    title.starts_with("Command Palette")
}

fn should_render_view_title(title: &str, lines: &[String]) -> bool {
    let Some(first_non_empty) = lines.iter().find(|line| !line.trim().is_empty()) else {
        return true;
    };
    if let Some(section) = first_non_empty.strip_prefix("## ") {
        return section != title;
    }
    !is_command_palette_title(title)
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
        approval_preview_lines, build_approval_text, build_collection_text,
        build_command_hint_text, build_command_palette_text, build_key_value_text,
        build_side_rail_lines, build_transcript_lines, build_welcome_lines, format_footer_context,
        should_render_side_rail, should_render_view_title,
    };
    use crate::frontend::tui::approval::ApprovalPrompt;
    use crate::frontend::tui::commands::{
        SlashCommandArgumentHint, SlashCommandArgumentSpec, SlashCommandArgumentValue,
        SlashCommandHint, SlashCommandSpec,
    };
    use crate::frontend::tui::state::{MainPaneMode, TodoEntry, TuiState};
    use ratatui::layout::Rect;

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
    fn key_value_text_preserves_prefixed_summary_blocks() {
        let rendered = build_key_value_text(&[
            "✔ Exported transcript text".to_string(),
            "  └ session-1".to_string(),
            "    Wrote 4 items to /workspace/out.txt".to_string(),
        ]);
        let lines = rendered.lines;
        assert_eq!(lines[0].spans[0].content.as_ref(), "✔");
        assert_eq!(
            lines[0].spans[2].content.as_ref(),
            "Exported transcript text"
        );
        assert_eq!(lines[1].spans[0].content.as_ref(), "  └ ");
        assert_eq!(
            lines[2].spans[1].content.as_ref(),
            "Wrote 4 items to /workspace/out.txt"
        );
    }

    #[test]
    fn key_value_text_reuses_transcript_rendering_for_shell_summary_lines() {
        let rendered = build_key_value_text(&[
            "• Reattached session".to_string(),
            "  └ session session-1".to_string(),
        ]);

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "•");
        assert_eq!(
            rendered.lines[0].spans[2].content.as_ref(),
            "Reattached session"
        );
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "  └ ");
        assert_eq!(
            rendered.lines[1].spans[1].content.as_ref(),
            "session session-1"
        );
    }

    #[test]
    fn transcript_entries_render_with_codex_like_prefixes() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• hello world".to_string()];

        let lines = build_transcript_lines(&state);

        assert_eq!(lines[0].spans[0].content.as_ref(), "•");
        assert_eq!(lines[0].spans[2].content.as_ref(), "hello world");
    }

    #[test]
    fn transcript_inserts_turn_dividers_between_user_turns() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            "› first".to_string(),
            "• reply".to_string(),
            "› second".to_string(),
        ];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.contains("┈"))
        }));
    }

    #[test]
    fn transcript_separates_assistant_and_tool_entries_with_breathing_room() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            "• assistant reply".to_string(),
            "• Running bash\n  └ $ cargo test".to_string(),
            "› next prompt".to_string(),
        ];

        let rendered = build_transcript_lines(&state);

        assert_eq!(line_text_for(&rendered[0]), "• assistant reply");
        assert!(line_text_for(&rendered[1]).is_empty());
        assert_eq!(line_text_for(&rendered[2]), "• Running bash");
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("hidden line"))
        );
        assert!(rendered.iter().any(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.contains("┈"))
        }));
    }

    #[test]
    fn transcript_collapses_tool_details_by_default() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• Finished bash\n  └ exit 0\n```text\nok\n```".to_string()];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Finished bash"))
        }));
        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("hidden lines"))
        }));
        assert!(!rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("exit 0"))
        }));
    }

    #[test]
    fn transcript_expands_tool_details_when_enabled() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            show_tool_details: true,
            ..TuiState::default()
        };
        state.transcript = vec!["• Finished bash\n  └ exit 0\n```text\nok\n```".to_string()];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("exit 0"))
        }));
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("ok"))
        );
    }

    #[test]
    fn transcript_renders_resume_summary_above_history() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            inspector_title: "Resume".to_string(),
            inspector: vec![
                "✔ Reattached session".to_string(),
                "  └ session session-1".to_string(),
            ],
            ..TuiState::default()
        };
        state.transcript = vec!["• done".to_string()];

        let rendered = build_transcript_lines(&state);

        assert_eq!(rendered[0].spans[0].content.as_ref(), "Resume");
        assert_eq!(rendered[2].spans[0].content.as_ref(), "✔");
        assert_eq!(rendered[2].spans[2].content.as_ref(), "Reattached session");
    }

    #[test]
    fn welcome_lines_keep_the_start_screen_sparse() {
        let mut state = TuiState::default();
        state.session.workspace_name = "nanoclaw".to_string();
        state.session.model = "gpt-5.4".to_string();

        let lines = build_welcome_lines(&state, 20);

        assert!(
            lines
                .iter()
                .any(|line| { line_text_for(line).contains("N   N  AAA  N   N  OOO   CCCC") })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line_text_for(line).contains("nanoclaw · gpt-5.4") })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line_text_for(line).contains("Type a prompt or /help.") })
        );
    }

    #[test]
    fn collection_text_renders_shell_summary_blocks_for_history_rows() {
        let rendered = build_collection_text(
            "Sessions",
            &[
                "## Sessions".to_string(),
                "• sess_123  no prompt yet\n  └ 12 messages · 40 events · 2 agent sessions · resume attached"
                    .to_string(),
            ],
        );

        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
        assert_eq!(
            rendered.lines[1].spans[2].content.as_ref(),
            "sess_123  no prompt yet"
        );
        assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "  ");
        assert_eq!(
            rendered.lines[2].spans[1].content.as_ref(),
            "12 messages · 40 events · 2 agent sessions · resume attached"
        );
    }

    #[test]
    fn collection_text_keeps_history_rows_compact() {
        let rendered = build_collection_text(
            "Sessions",
            &[
                "• sess_123  no prompt yet\n  └ 12 messages · 40 events".to_string(),
                "• sess_456  resume prompt\n  └ 4 messages · 9 events".to_string(),
            ],
        );

        assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "›");
        assert_eq!(
            rendered.lines[2].spans[2].content.as_ref(),
            "sess_456  resume prompt"
        );
    }

    #[test]
    fn command_palette_text_matches_picker_style() {
        let rendered = build_command_palette_text(&[
            "## Session".to_string(),
            "/help [query]  browse commands".to_string(),
            "/sessions [query]  browse persisted sessions".to_string(),
        ]);

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "Session");
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
        assert_eq!(rendered.lines[1].spans[2].content.as_ref(), "/help [query]");
        assert_eq!(
            rendered.lines[1].spans[4].content.as_ref(),
            "browse commands"
        );
        assert_eq!(
            rendered.lines[2].spans[2].content.as_ref(),
            "/sessions [query]"
        );
    }

    #[test]
    fn transcript_renders_compact_live_progress_line() {
        let state = TuiState {
            main_pane: MainPaneMode::Transcript,
            turn_running: true,
            status: "Working (2)".to_string(),
            ..TuiState::default()
        };

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Working (2)"))
        }));
        assert!(!rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("$ cargo test"))
        }));
    }

    #[test]
    fn transcript_hides_progress_line_while_tool_cell_is_active() {
        let state = TuiState {
            main_pane: MainPaneMode::Transcript,
            turn_running: true,
            status: "Working".to_string(),
            active_tool_label: Some("bash".to_string()),
            transcript: vec!["• Running bash\n  └ $ cargo test".to_string()],
            ..TuiState::default()
        };

        let rendered = build_transcript_lines(&state);

        let running_count = rendered
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.as_ref().contains("Running bash"))
            })
            .count();
        assert_eq!(running_count, 1);
    }

    #[test]
    fn transcript_renders_markdown_blocks_without_fence_noise() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            concat!(
                "• # Plan\n",
                "- inspect output\n",
                "1. rerun tests\n",
                "> keep the diff readable\n",
                "Use `rg` for search\n",
                "```diff\n",
                "+ added line\n",
                "- removed line\n",
                "@@ hunk\n",
                "```"
            )
            .to_string(),
        ];

        let rendered = build_transcript_lines(&state);
        assert_eq!(rendered[0].spans[0].content.as_ref(), "•");
        assert_eq!(rendered[0].spans[2].content.as_ref(), "Plan");
        assert!(rendered.iter().all(|line| {
            line.spans
                .iter()
                .all(|span| !span.content.as_ref().contains("```"))
        }));
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("inspect output"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("rerun tests"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("keep the diff readable"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.as_ref() == "rg") })
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("+ added line"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("- removed line"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("@@ hunk"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("··· diff"))
        );
    }

    #[test]
    fn transcript_keeps_fenced_block_label_as_first_visible_line() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• ```rust\nfn main() {}\n```".to_string()];

        let rendered = build_transcript_lines(&state);

        let first_visible = rendered
            .iter()
            .find(|line| !line_text_for(line).trim().is_empty())
            .expect("expected visible transcript line");
        assert_eq!(line_text_for(first_visible), "• ··· rust");
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("fn main() {}"))
        );
    }

    fn line_text_for(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn side_rail_surfaces_todos_and_lsp_summary() {
        let mut state = TuiState::default();
        state.main_pane = MainPaneMode::Transcript;
        state.session.tool_names = vec!["code_symbol_search".to_string()];
        state.session.startup_diagnostics.diagnostics = vec!["rust-analyzer attached".to_string()];
        state.todo_items = vec![
            TodoEntry {
                id: "t1".to_string(),
                content: "Refine transcript".to_string(),
                status: "in_progress".to_string(),
            },
            TodoEntry {
                id: "t2".to_string(),
                content: "Tighten command palette".to_string(),
                status: "pending".to_string(),
            },
            TodoEntry {
                id: "t3".to_string(),
                content: "Finish diagnostics".to_string(),
                status: "completed".to_string(),
            },
        ];

        let lines = build_side_rail_lines(&state);

        assert!(line_text_for(&lines[0]).contains("LSP"));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("0 warnings · 1 diagnostics"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("rust-analyzer attached"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("1 active · 1 pending · 1 done")
            })
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Refine transcript"))
        }));
    }

    #[test]
    fn side_rail_stays_hidden_for_non_transcript_views() {
        let mut state = TuiState::default();
        state.main_pane = MainPaneMode::View;
        state.session.tool_names = vec!["code_symbol_search".to_string()];

        assert!(!should_render_side_rail(
            &state,
            Rect {
                x: 0,
                y: 0,
                width: 140,
                height: 20,
            }
        ));
    }

    #[test]
    fn approval_band_uses_structured_command_preview() {
        let text = build_approval_text(&ApprovalPrompt {
            tool_name: "bash".to_string(),
            origin: "local".to_string(),
            mode: Some("run".to_string()),
            working_directory: Some("/workspace/apps/code-agent".to_string()),
            content_label: "command".to_string(),
            content_preview: vec!["$ cargo test".to_string()],
            reasons: vec!["sandbox policy requires approval".to_string()],
        });

        assert!(line_text_for(&text.lines[0]).contains("Approve bash?"));
        assert!(
            text.lines[1]
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("/workspace/apps/code-agent") })
        );
        assert_eq!(text.lines[2].spans[0].content.as_ref(), "command");
        assert_eq!(text.lines[4].spans[0].content.as_ref(), "why");
        assert!(text.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("$ cargo test"))
        }));
        assert!(text.lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("sandbox policy requires approval")
            })
        }));
    }

    #[test]
    fn approval_preview_lines_collapse_long_argument_blocks() {
        let lines = approval_preview_lines(&[
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
            "five".to_string(),
        ]);

        assert_eq!(lines, vec!["one", "two", "...", "five"]);
    }

    #[test]
    fn command_hint_text_surfaces_selected_usage_and_matches() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "History",
                name: "sessions",
                usage: "sessions [query]",
                summary: "browse persisted sessions",
            },
            matches: vec![
                SlashCommandSpec {
                    section: "History",
                    name: "sessions",
                    usage: "sessions [query]",
                    summary: "browse persisted sessions",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "session",
                    usage: "session <session-ref>",
                    summary: "open persisted session",
                },
            ],
            selected_match_index: 0,
            arguments: None,
            exact: false,
        });

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "commands");
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
        assert_eq!(
            rendered.lines[1].spans[2].content.as_ref(),
            "/sessions [query]"
        );
        assert_eq!(
            rendered.lines[1].spans[4].content.as_ref(),
            "browse persisted sessions"
        );
        assert_eq!(
            rendered.lines[2].spans[1].content.as_ref(),
            "/session <session-ref>"
        );
        assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "tab complete");
        assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "enter accept");
    }

    #[test]
    fn command_hint_text_surfaces_argument_progress() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "Agents",
                name: "spawn_task",
                usage: "spawn_task <role> <prompt>",
                summary: "launch child agent",
            },
            matches: vec![SlashCommandSpec {
                section: "Agents",
                name: "spawn_task",
                usage: "spawn_task <role> <prompt>",
                summary: "launch child agent",
            }],
            selected_match_index: 0,
            arguments: Some(SlashCommandArgumentHint {
                provided: vec![SlashCommandArgumentValue {
                    placeholder: "<role>",
                    value: "reviewer".to_string(),
                }],
                next: Some(SlashCommandArgumentSpec {
                    placeholder: "<prompt>",
                    required: true,
                }),
            }),
            exact: true,
        });

        assert_eq!(rendered.lines[2].spans[1].content.as_ref(), "<role>");
        assert_eq!(rendered.lines[2].spans[3].content.as_ref(), "reviewer");
        assert!(
            rendered.lines[2]
                .spans
                .iter()
                .any(|span| span.content.as_ref().contains("<prompt>"))
        );
        assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "keep typing");
        assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "keep typing");
    }

    #[test]
    fn command_hint_text_keeps_enter_run_for_optional_arguments() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "Session",
                name: "help",
                usage: "help [query]",
                summary: "browse commands",
            },
            matches: vec![SlashCommandSpec {
                section: "Session",
                name: "help",
                usage: "help [query]",
                summary: "browse commands",
            }],
            selected_match_index: 0,
            arguments: Some(SlashCommandArgumentHint {
                provided: Vec::new(),
                next: Some(SlashCommandArgumentSpec {
                    placeholder: "[query]",
                    required: false,
                }),
            }),
            exact: true,
        });

        assert_eq!(rendered.lines[2].spans[1].content.as_ref(), "[query]");
        assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "enter run");
        assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "enter run");
    }

    #[test]
    fn command_hint_text_shows_browse_window_ellipsis() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "History",
                name: "resume",
                usage: "resume <agent-session-ref>",
                summary: "reattach agent session",
            },
            matches: vec![
                SlashCommandSpec {
                    section: "Session",
                    name: "help",
                    usage: "help",
                    summary: "browse commands",
                },
                SlashCommandSpec {
                    section: "Session",
                    name: "status",
                    usage: "status",
                    summary: "session overview",
                },
                SlashCommandSpec {
                    section: "Session",
                    name: "new",
                    usage: "new",
                    summary: "fresh top-level session",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "sessions",
                    usage: "sessions [query]",
                    summary: "browse persisted sessions",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "session",
                    usage: "session <session-ref>",
                    summary: "open persisted session",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "resume",
                    usage: "resume <agent-session-ref>",
                    summary: "reattach agent session",
                },
                SlashCommandSpec {
                    section: "Agents",
                    name: "live_tasks",
                    usage: "live_tasks",
                    summary: "list live child agents",
                },
            ],
            selected_match_index: 5,
            arguments: None,
            exact: false,
        });

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "commands");
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "… 2 earlier");
        assert_eq!(
            rendered.lines[5].spans[2].content.as_ref(),
            "/resume <agent-session-ref>"
        );
        assert_eq!(rendered.lines[6].spans[0].content.as_ref(), "… 1 more");
    }

    #[test]
    fn footer_context_prefers_workspace_and_session_ref() {
        let mut state = TuiState::default();
        state.status = "Ready".to_string();
        state.session.workspace_name = "nanoclaw".to_string();
        state.session.model = "gpt-5.4".to_string();
        state.session.active_session_ref = "session_123456".to_string();

        let footer = format_footer_context(&state);

        assert_eq!(footer.spans[0].content.as_ref(), "•");
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("Ready") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("nanoclaw") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("details off") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("session_") })
        );
    }

    #[test]
    fn view_title_is_suppressed_when_the_collection_already_has_one() {
        assert!(!should_render_view_title(
            "Sessions",
            &["## Sessions".to_string(), "• sess_123  prompt".to_string()]
        ));
        assert!(should_render_view_title(
            "Export",
            &["## Session".to_string(), "path: out.txt".to_string()]
        ));
    }
}
