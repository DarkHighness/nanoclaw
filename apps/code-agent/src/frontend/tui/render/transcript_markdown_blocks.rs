use super::theme::{ASSISTANT, ERROR, MUTED, NanoclawMarkdownStyleSheet, SUBTLE, TEXT, USER};
use super::transcript::{
    TranscriptEntryKind, line_has_visible_content, line_to_plain_text,
    transcript_continuation_prefix,
};
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui_core::layout::Alignment as CoreAlignment;
use ratatui_core::style::{Color as CoreColor, Modifier as CoreModifier, Style as CoreStyle};
use ratatui_core::text::{Line as CoreLine, Span as CoreSpan};
use tui_markdown::{Options as MarkdownOptions, from_str_with_options};

pub(super) fn render_shell_code_block(
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

pub(super) fn render_markdown_lines(body: &str) -> Vec<Line<'static>> {
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

pub(super) fn apply_markdown_prefixes(
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

fn code_block_label_line(
    language: &str,
    kind: TranscriptEntryKind,
    is_first_visible: bool,
) -> Line<'static> {
    let mut spans = vec![
        Span::styled("···", Style::default().fg(SUBTLE)),
        Span::raw(" "),
        Span::styled(
            language.to_string(),
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ),
    ];
    if !is_first_visible {
        spans.insert(0, transcript_continuation_prefix(kind));
    }
    Line::from(spans)
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
