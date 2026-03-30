use super::super::state::StatusLinePickerState;
use super::theme::{ASSISTANT, BORDER_ACTIVE, ERROR, HEADER, MUTED, SUBTLE, TEXT, USER, WARN};
use crate::statusline::{StatusLineConfig, status_line_fields};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

pub(super) fn build_inspector_text(title: &str, lines: &[String]) -> Text<'static> {
    if is_command_palette_title(title) {
        build_command_palette_text(lines)
    } else if is_collection_inspector(title) {
        build_collection_text(title, lines)
    } else {
        build_key_value_text(lines)
    }
}

pub(super) fn should_render_view_title(title: &str, lines: &[String]) -> bool {
    let Some(first_non_empty) = lines.iter().find(|line| !line.trim().is_empty()) else {
        return true;
    };
    if let Some(section) = first_non_empty.strip_prefix("## ") {
        return section != title;
    }
    !is_command_palette_title(title)
}

pub(super) fn build_key_value_text(lines: &[String]) -> Text<'static> {
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
        } else if let Some((marker, accent, body)) = super::transcript::parse_prefixed_entry(line) {
            let kind = super::transcript::transcript_entry_kind(marker, body);
            rendered.push(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    body.to_string(),
                    super::transcript::transcript_body_style(marker, kind, body),
                ),
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

pub(super) fn build_command_palette_text(lines: &[String]) -> Text<'static> {
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

pub(super) fn build_statusline_picker_text(
    config: &StatusLineConfig,
    picker: &StatusLinePickerState,
) -> Text<'static> {
    let enabled_count = status_line_fields()
        .iter()
        .filter(|spec| config.enabled(spec.field))
        .count();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("status line", Style::default().fg(HEADER)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                format!("{enabled_count}/{} visible", status_line_fields().len()),
                Style::default().fg(USER),
            ),
        ]),
        Line::from(Span::styled(
            "space toggle · enter close · esc close",
            Style::default().fg(SUBTLE),
        )),
        Line::raw(""),
    ];

    for (index, spec) in status_line_fields().iter().enumerate() {
        let enabled = config.enabled(spec.field);
        let selected = picker.selected == index;
        let marker = if selected { "›" } else { " " };
        let checkbox = if enabled { "[x]" } else { "[ ]" };
        lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(if selected { USER } else { SUBTLE }),
            ),
            Span::raw(" "),
            Span::styled(
                checkbox,
                Style::default().fg(if enabled { ASSISTANT } else { SUBTLE }),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:<8}", spec.label),
                Style::default()
                    .fg(if selected { HEADER } else { TEXT })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(spec.summary, Style::default().fg(MUTED)),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Changes apply immediately for this TUI session.",
        Style::default().fg(SUBTLE),
    )));
    Text::from(lines)
}

pub(super) fn build_collection_text(title: &str, lines: &[String]) -> Text<'static> {
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
            && let Some((_, _, body)) = super::transcript::parse_prefixed_entry(raw_line)
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

fn is_shell_summary_line(line: &str) -> bool {
    super::transcript::parse_prefixed_entry(line).is_some()
        || line.starts_with("  └ ")
        || line.starts_with("    ")
        || line.starts_with("- ")
        || line.starts_with("* ")
}

fn render_shell_summary_line(line: &str) -> Vec<Line<'static>> {
    if super::transcript::parse_prefixed_entry(line).is_some() {
        super::transcript::format_transcript_entry(line)
    } else {
        vec![super::transcript_markdown::render_transcript_body_line(
            line,
            "•",
            super::transcript::TranscriptEntryKind::ShellSummary,
            false,
            false,
        )]
    }
}

fn is_command_palette_title(title: &str) -> bool {
    title.starts_with("Command Palette")
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
