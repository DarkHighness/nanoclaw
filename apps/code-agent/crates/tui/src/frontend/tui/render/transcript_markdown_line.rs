use super::theme::palette;
use super::transcript::{
    TranscriptEntryKind, transcript_body_style, transcript_continuation_prefix,
};
use super::transcript_markdown_blocks::code_span;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

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
            Span::styled("  └ ", Style::default().fg(palette().subtle)),
            Span::styled(detail.to_string(), Style::default().fg(palette().muted)),
        ]);
    }
    if let Some(detail) = raw_line.strip_prefix("    ") {
        return Line::from(vec![
            Span::raw("    "),
            Span::styled(detail.to_string(), Style::default().fg(palette().muted)),
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
            vec![Span::styled(
                "┈".repeat(18),
                Style::default().fg(palette().subtle),
            )],
        );
    }
    if let Some(rest) = markdown_quote(raw_line) {
        let mut spans = vec![
            Span::styled("│", Style::default().fg(palette().subtle)),
            Span::raw(" "),
        ];
        spans.extend(markdown_inline_spans(
            rest,
            markdown_body_style(kind, Style::default().fg(palette().muted)),
        ));
        return line_with_indent(kind, is_first_visible, spans);
    }
    if let Some(rest) = raw_line
        .strip_prefix("- ")
        .or_else(|| raw_line.strip_prefix("* "))
    {
        let mut spans = vec![
            Span::styled("-", Style::default().fg(palette().muted)),
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
            Span::styled(format!("{ordinal}."), Style::default().fg(palette().muted)),
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
    let style = Style::default()
        .fg(palette().header)
        .add_modifier(Modifier::BOLD);
    if level <= 2 {
        style
    } else {
        style.fg(palette().text)
    }
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
                spans.push(Span::styled(
                    code.to_string(),
                    Style::default().fg(palette().user),
                ));
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
                    Style::default().fg(palette().subtle),
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

fn markdown_body_style(kind: TranscriptEntryKind, base: Style) -> Style {
    match kind {
        TranscriptEntryKind::AssistantMessage | TranscriptEntryKind::UserPrompt => {
            base.fg(palette().text)
        }
        TranscriptEntryKind::PlanUpdate | TranscriptEntryKind::ExecutionUpdate => {
            base.fg(palette().text)
        }
        TranscriptEntryKind::ShellSummary => base.fg(palette().muted),
        TranscriptEntryKind::SuccessSummary => base.fg(palette().assistant),
        TranscriptEntryKind::ErrorSummary => base.fg(palette().error),
        TranscriptEntryKind::WarningSummary => base.fg(palette().warn),
    }
}
