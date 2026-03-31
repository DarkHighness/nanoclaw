use super::super::state::{InspectorEntry, StatusLinePickerState, ThinkingEffortPickerState};
use super::theme::{ASSISTANT, BORDER_ACTIVE, ERROR, HEADER, MUTED, SUBTLE, TEXT, USER, WARN};
use crate::statusline::{StatusLineConfig, status_line_fields};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

pub(super) fn build_inspector_text(title: &str, lines: &[InspectorEntry]) -> Text<'static> {
    if is_command_palette_title(title) {
        build_command_palette_text(lines)
    } else if is_collection_inspector(title) {
        build_collection_text(title, lines)
    } else {
        build_key_value_text(lines)
    }
}

pub(super) fn should_render_view_title(title: &str, lines: &[InspectorEntry]) -> bool {
    let Some(first_non_empty) = lines
        .iter()
        .find(|line| !inspector_entry_text(line).trim().is_empty())
    else {
        return true;
    };
    if let InspectorEntry::Section(section) = first_non_empty {
        return section != title;
    }
    !is_command_palette_title(title)
}

pub(super) fn build_key_value_text(lines: &[InspectorEntry]) -> Text<'static> {
    let mut rendered = Vec::new();
    for entry in lines {
        rendered.extend(render_key_value_entry(entry, rendered.is_empty()));
    }
    Text::from(rendered)
}

pub(super) fn build_command_palette_text(lines: &[InspectorEntry]) -> Text<'static> {
    let mut rendered = Vec::new();
    for entry in lines {
        match entry {
            InspectorEntry::Section(section) => {
                if !rendered.is_empty() {
                    rendered.push(Line::raw(""));
                }
                rendered.push(Line::from(Span::styled(
                    section.clone(),
                    Style::default().fg(MUTED),
                )));
            }
            InspectorEntry::CollectionItem { primary, secondary } => {
                rendered.push(command_palette_line(primary, secondary.as_deref()));
            }
            InspectorEntry::Command(command) => {
                rendered.push(command_palette_line(command, None));
            }
            InspectorEntry::Muted(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(SUBTLE),
            ))),
            InspectorEntry::Plain(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(TEXT),
            ))),
            InspectorEntry::Field { key, value } => {
                rendered.push(command_palette_line(key, Some(value)));
            }
            InspectorEntry::Transcript(entry) => {
                rendered.extend(super::transcript::format_transcript_cell(entry));
            }
            InspectorEntry::Empty => rendered.push(Line::raw("")),
        }
    }
    Text::from(rendered)
}

fn command_palette_line(command: &str, summary: Option<&str>) -> Line<'static> {
    let mut spans = vec![
        Span::styled("›", Style::default().fg(USER).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(
            command.to_string(),
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(summary) = summary
        && !summary.trim().is_empty()
    {
        spans.push(Span::styled("  ", Style::default().fg(SUBTLE)));
        spans.push(Span::styled(
            summary.to_string(),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn render_key_value_entry(entry: &InspectorEntry, is_first: bool) -> Vec<Line<'static>> {
    match entry {
        InspectorEntry::Section(title) => {
            let mut lines = Vec::new();
            if !is_first {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(Span::styled(
                title.clone(),
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            )));
            lines
        }
        InspectorEntry::Field { key, value } => vec![Line::from(vec![
            Span::styled(
                format!("{key}:"),
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(value.clone(), value_style(key.trim(), value.trim())),
        ])],
        InspectorEntry::Transcript(entry) => super::transcript::format_transcript_cell(entry),
        InspectorEntry::Command(line) => vec![Line::from(Span::styled(
            line.clone(),
            Style::default().fg(USER).add_modifier(Modifier::BOLD),
        ))],
        InspectorEntry::Muted(line) => vec![Line::from(Span::styled(
            line.clone(),
            Style::default().fg(SUBTLE),
        ))],
        InspectorEntry::Plain(line) => vec![Line::from(Span::styled(
            line.clone(),
            plain_text_style(line),
        ))],
        InspectorEntry::CollectionItem { primary, secondary } => {
            vec![collection_line(
                primary,
                secondary.as_deref(),
                BORDER_ACTIVE,
            )]
        }
        InspectorEntry::Empty => vec![Line::raw("")],
    }
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

pub(super) fn build_thinking_effort_picker_text(
    current: Option<&str>,
    supported: &[String],
    picker: &ThinkingEffortPickerState,
) -> Text<'static> {
    let current = current.unwrap_or("default");
    let mut lines = vec![
        Line::from(vec![
            Span::styled("thinking effort", Style::default().fg(HEADER)),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(current.to_string(), Style::default().fg(USER)),
        ]),
        Line::from(Span::styled(
            "enter apply · ↑↓ move · esc close",
            Style::default().fg(SUBTLE),
        )),
        Line::raw(""),
    ];

    if supported.is_empty() {
        lines.push(Line::from(Span::styled(
            "No configurable thinking effort levels are available for this model.",
            Style::default().fg(SUBTLE),
        )));
        return Text::from(lines);
    }

    for (index, level) in supported.iter().enumerate() {
        let selected = picker.selected == index;
        let active = Some(level.as_str()) == Some(current);
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "›" } else { " " },
                Style::default().fg(if selected { USER } else { SUBTLE }),
            ),
            Span::raw(" "),
            Span::styled(
                if active { "[x]" } else { "[ ]" },
                Style::default().fg(if active { ASSISTANT } else { SUBTLE }),
            ),
            Span::raw(" "),
            Span::styled(
                level.clone(),
                Style::default()
                    .fg(if selected { HEADER } else { TEXT })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Changes apply to the next model request in this TUI session.",
        Style::default().fg(SUBTLE),
    )));
    Text::from(lines)
}

pub(super) fn build_collection_text(title: &str, lines: &[InspectorEntry]) -> Text<'static> {
    let accent = inspector_accent(title);
    let mut rendered = Vec::new();
    for entry in lines {
        match entry {
            InspectorEntry::Section(section) => rendered.push(Line::from(Span::styled(
                section.clone(),
                Style::default().fg(MUTED),
            ))),
            InspectorEntry::Muted(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(SUBTLE),
            ))),
            InspectorEntry::CollectionItem { primary, secondary } => {
                rendered.push(collection_line(primary, secondary.as_deref(), accent));
            }
            InspectorEntry::Transcript(entry) => {
                rendered.extend(render_collection_transcript_entry(entry, accent));
            }
            InspectorEntry::Plain(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(TEXT),
            ))),
            InspectorEntry::Command(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(USER).add_modifier(Modifier::BOLD),
            ))),
            InspectorEntry::Field { key, value } => {
                rendered.push(collection_line(key, Some(value), accent));
            }
            InspectorEntry::Empty => rendered.push(Line::raw("")),
        }
    }
    Text::from(rendered)
}

fn render_collection_transcript_entry(
    entry: &super::super::state::TranscriptEntry,
    accent: Color,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    for (index, line) in super::transcript::format_transcript_cell(entry)
        .into_iter()
        .enumerate()
    {
        if index == 0 {
            let text = super::transcript::line_to_plain_text(&line);
            let body = text
                .strip_prefix("• ")
                .or_else(|| text.strip_prefix("✔ "))
                .or_else(|| text.strip_prefix("✗ "))
                .or_else(|| text.strip_prefix("⚠ "))
                .unwrap_or(text.as_str())
                .to_string();
            rendered.push(Line::from(vec![
                Span::styled(
                    "›",
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    body,
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            let text = super::transcript::line_to_plain_text(&line);
            if let Some(detail) = text.strip_prefix("  └ ") {
                rendered.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(SUBTLE)),
                    Span::styled(detail.to_string(), Style::default().fg(MUTED)),
                ]));
            } else if let Some(detail) = text.strip_prefix("    ") {
                rendered.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(SUBTLE)),
                    Span::styled(detail.to_string(), Style::default().fg(SUBTLE)),
                ]));
            } else {
                rendered.push(line);
            }
        }
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

fn inspector_entry_text(entry: &InspectorEntry) -> String {
    match entry {
        InspectorEntry::Section(line)
        | InspectorEntry::Plain(line)
        | InspectorEntry::Muted(line)
        | InspectorEntry::Command(line) => line.clone(),
        InspectorEntry::Field { key, value } => format!("{key}: {value}"),
        InspectorEntry::Transcript(entry) => entry.serialized(),
        InspectorEntry::CollectionItem { primary, secondary } => secondary
            .as_ref()
            .map(|secondary| format!("{primary}  {secondary}"))
            .unwrap_or_else(|| primary.clone()),
        InspectorEntry::Empty => String::new(),
    }
}
