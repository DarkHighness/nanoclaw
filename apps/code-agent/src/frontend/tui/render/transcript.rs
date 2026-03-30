use super::super::state::{TuiState, preview_text};
use super::statusline::status_color;
use super::theme::{
    ASSISTANT, ERROR, HEADER, MAIN_BG, MUTED, NanoclawMarkdownStyleSheet, SUBTLE, TEXT, USER, WARN,
};
use super::view::build_inspector_text;
use super::welcome::build_welcome_lines;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui_core::layout::Alignment as CoreAlignment;
use ratatui_core::style::{Color as CoreColor, Modifier as CoreModifier, Style as CoreStyle};
use ratatui_core::text::{Line as CoreLine, Span as CoreSpan};
use tui_markdown::{Options as MarkdownOptions, from_str_with_options};

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
    let scroll = super::clamp_scroll(state.transcript_scroll, lines.len(), inner.height);
    let transcript = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(TEXT).bg(MAIN_BG));
    frame.render_widget(transcript, inner);
}

pub(super) fn build_transcript_lines(state: &TuiState) -> Vec<Line<'static>> {
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
pub(super) enum TranscriptEntryKind {
    UserPrompt,
    AssistantMessage,
    ShellSummary,
    SuccessSummary,
    ErrorSummary,
    WarningSummary,
}

pub(super) fn format_transcript_entry(entry: &str) -> Vec<Line<'static>> {
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

    render_shell_summary_body(body, marker, kind)
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

pub(super) fn render_transcript_body_line(
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

pub(super) fn code_span(line: &str) -> Span<'static> {
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
