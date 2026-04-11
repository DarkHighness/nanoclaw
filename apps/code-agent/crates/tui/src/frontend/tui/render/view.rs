use super::super::state::{
    InspectorAction, InspectorEntry, StatusLinePickerState, ThemePickerState,
    ThinkingEffortPickerState,
};
use super::theme::palette;
use crate::statusline::{StatusLineConfig, status_line_fields};
use crate::theme::ThemeSummary;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

pub(super) fn build_inspector_text(
    title: &str,
    lines: &[InspectorEntry],
    selected_collection: Option<usize>,
) -> Text<'static> {
    if is_command_palette_title(title) {
        build_command_palette_text(lines, selected_collection)
    } else if is_collection_inspector(title) {
        build_collection_text(title, lines, selected_collection)
    } else {
        build_key_value_text(lines)
    }
}

#[cfg(test)]
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

pub(super) fn build_command_palette_text(
    lines: &[InspectorEntry],
    selected_collection: Option<usize>,
) -> Text<'static> {
    let mut rendered = Vec::new();
    let mut actionable_index = 0;
    for entry in lines {
        match entry {
            InspectorEntry::Section(section) => {
                if !rendered.is_empty() {
                    rendered.push(Line::raw(""));
                }
                rendered.push(Line::from(vec![
                    Span::styled("section", Style::default().fg(palette().subtle)),
                    Span::styled(" · ", Style::default().fg(palette().subtle)),
                    Span::styled(
                        section.clone(),
                        Style::default()
                            .fg(palette().header)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
            InspectorEntry::CollectionItem {
                primary,
                secondary,
                action,
                alternate_action,
            } => {
                let selected = (action.is_some() || alternate_action.is_some())
                    && selected_collection == Some(actionable_index);
                if action.is_some() || alternate_action.is_some() {
                    actionable_index += 1;
                }
                rendered.extend(command_palette_item_lines(
                    primary,
                    secondary.as_deref(),
                    selected,
                ));
            }
            InspectorEntry::Command(command) => {
                rendered.extend(command_palette_item_lines(command, None, false));
            }
            InspectorEntry::Muted(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(palette().subtle),
            ))),
            InspectorEntry::Plain(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(palette().text),
            ))),
            InspectorEntry::Field { key, value } => {
                rendered.extend(command_palette_item_lines(key, Some(value), false));
            }
            InspectorEntry::Transcript(entry) => {
                rendered.extend(super::transcript::format_transcript_cell(entry));
            }
            InspectorEntry::Empty => rendered.push(Line::raw("")),
        }
    }
    Text::from(rendered)
}

fn command_palette_item_lines(
    command: &str,
    summary: Option<&str>,
    selected: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            if selected { "›" } else { "·" },
            Style::default().fg(if selected {
                palette().accent
            } else {
                palette().subtle
            }),
        ),
        Span::raw(" "),
        Span::styled(
            command.to_string(),
            Style::default()
                .fg(if selected {
                    palette().header
                } else {
                    palette().text
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ])];
    if let Some(summary) = summary
        && !summary.trim().is_empty()
    {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled(
                summary.trim().to_string(),
                Style::default().fg(if selected {
                    palette().muted
                } else {
                    palette().subtle
                }),
            ),
        ]));
    }
    lines
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
                Style::default()
                    .fg(palette().header)
                    .add_modifier(Modifier::BOLD),
            )));
            lines
        }
        InspectorEntry::Field { key, value } => vec![Line::from(vec![
            Span::styled(
                format!("{key}:"),
                Style::default()
                    .fg(palette().muted)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(value.clone(), value_style(key.trim(), value.trim())),
        ])],
        InspectorEntry::Transcript(entry) => super::transcript::format_transcript_cell(entry),
        InspectorEntry::Command(line) => vec![Line::from(Span::styled(
            line.clone(),
            Style::default()
                .fg(palette().user)
                .add_modifier(Modifier::BOLD),
        ))],
        InspectorEntry::Muted(line) => vec![Line::from(Span::styled(
            line.clone(),
            Style::default().fg(palette().subtle),
        ))],
        InspectorEntry::Plain(line) => vec![Line::from(Span::styled(
            line.clone(),
            plain_text_style(line),
        ))],
        InspectorEntry::CollectionItem {
            primary, secondary, ..
        } => collection_item_lines(
            primary,
            secondary.as_deref(),
            palette().border_active,
            false,
        ),
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
            Span::styled("status line", Style::default().fg(palette().header)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(
                format!("{enabled_count}/{} visible", status_line_fields().len()),
                Style::default().fg(palette().user),
            ),
        ]),
        Line::from(Span::styled(
            "space toggle · enter close · esc close",
            Style::default().fg(palette().subtle),
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
                Style::default().fg(if selected {
                    palette().user
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                checkbox,
                Style::default().fg(if enabled {
                    palette().assistant
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:<8}", spec.label),
                Style::default()
                    .fg(if selected {
                        palette().header
                    } else {
                        palette().text
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(spec.summary, Style::default().fg(palette().muted)),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Changes apply immediately for this TUI session.",
        Style::default().fg(palette().subtle),
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
            Span::styled("thinking effort", Style::default().fg(palette().header)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(current.to_string(), Style::default().fg(palette().user)),
        ]),
        Line::from(Span::styled(
            "enter save · ↑↓ preview · home/end jump · esc restore",
            Style::default().fg(palette().subtle),
        )),
        Line::raw(""),
    ];

    if supported.is_empty() {
        lines.push(Line::from(Span::styled(
            "No configurable thinking effort levels are available for this model.",
            Style::default().fg(palette().subtle),
        )));
        return Text::from(lines);
    }

    for (index, level) in supported.iter().enumerate() {
        let selected = picker.selected == index;
        let active = Some(level.as_str()) == Some(current);
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "›" } else { " " },
                Style::default().fg(if selected {
                    palette().user
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                if active { "[x]" } else { "[ ]" },
                Style::default().fg(if active {
                    palette().assistant
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                level.clone(),
                Style::default()
                    .fg(if selected {
                        palette().header
                    } else {
                        palette().text
                    })
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
        Style::default().fg(palette().subtle),
    )));
    Text::from(lines)
}

pub(super) fn build_theme_picker_text(
    current: &str,
    themes: &[ThemeSummary],
    picker: &ThemePickerState,
) -> Text<'static> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("theme", Style::default().fg(palette().header)),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(current.to_string(), Style::default().fg(palette().user)),
        ]),
        Line::from(Span::styled(
            "enter save · ↑↓ preview · home/end jump · esc restore",
            Style::default().fg(palette().subtle),
        )),
        Line::raw(""),
    ];

    if themes.is_empty() {
        lines.push(Line::from(Span::styled(
            "No TUI themes are available in the loaded theme catalog.",
            Style::default().fg(palette().subtle),
        )));
        return Text::from(lines);
    }

    for (index, theme) in themes.iter().enumerate() {
        let selected = picker.selected == index;
        let active = theme.id == current;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "›" } else { " " },
                Style::default().fg(if selected {
                    palette().user
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                if active { "[x]" } else { "[ ]" },
                Style::default().fg(if active {
                    palette().assistant
                } else {
                    palette().subtle
                }),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:<12}", theme.id),
                Style::default()
                    .fg(if selected {
                        palette().header
                    } else {
                        palette().text
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(theme.summary.clone(), Style::default().fg(palette().muted)),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Themes come from builtin assets plus any user-supplied theme catalog file.",
        Style::default().fg(palette().subtle),
    )));
    lines.push(Line::from(Span::styled(
        "Moving the picker previews immediately; Enter saves and Esc restores.",
        Style::default().fg(palette().subtle),
    )));
    Text::from(lines)
}

pub(super) fn build_collection_text(
    title: &str,
    lines: &[InspectorEntry],
    selected_collection: Option<usize>,
) -> Text<'static> {
    let accent = inspector_accent(title);
    let mut rendered = Vec::new();
    let mut actionable_index = 0;
    for entry in lines {
        match entry {
            InspectorEntry::Section(section) => rendered.push(Line::from(Span::styled(
                section.clone(),
                Style::default().fg(palette().muted),
            ))),
            InspectorEntry::Muted(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(palette().subtle),
            ))),
            InspectorEntry::CollectionItem {
                primary,
                secondary,
                action,
                alternate_action,
            } => {
                let selected = (action.is_some() || alternate_action.is_some())
                    && selected_collection == Some(actionable_index);
                if action.is_some() || alternate_action.is_some() {
                    actionable_index += 1;
                }
                rendered.extend(collection_item_lines(
                    primary,
                    secondary.as_deref(),
                    accent,
                    selected,
                ));
            }
            InspectorEntry::Transcript(entry) => {
                rendered.extend(render_collection_transcript_entry(entry, accent));
            }
            InspectorEntry::Plain(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(palette().text),
            ))),
            InspectorEntry::Command(line) => rendered.push(Line::from(Span::styled(
                line.clone(),
                Style::default()
                    .fg(palette().user)
                    .add_modifier(Modifier::BOLD),
            ))),
            InspectorEntry::Field { key, value } => {
                rendered.extend(collection_item_lines(key, Some(value), accent, false));
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
                    Span::styled("  ", Style::default().fg(palette().subtle)),
                    Span::styled(detail.to_string(), Style::default().fg(palette().muted)),
                ]));
            } else if let Some(detail) = text.strip_prefix("    ") {
                rendered.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(palette().subtle)),
                    Span::styled(detail.to_string(), Style::default().fg(palette().subtle)),
                ]));
            } else {
                rendered.push(line);
            }
        }
    }
    rendered
}

fn collection_item_lines(
    primary: &str,
    secondary: Option<&str>,
    accent: Color,
    selected: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            if selected { "›" } else { "·" },
            Style::default().fg(if selected { accent } else { palette().subtle }),
        ),
        Span::raw(" "),
        Span::styled(
            primary.to_string(),
            Style::default()
                .fg(if selected { accent } else { palette().text })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ])];
    if let Some(secondary) = secondary
        && !secondary.trim().is_empty()
    {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled(
                secondary.trim().to_string(),
                Style::default().fg(if selected {
                    palette().muted
                } else {
                    palette().subtle
                }),
            ),
        ]));
    }
    lines
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
        "Live Tasks" => palette().user,
        "Sessions" | "Session Search" | "Agent Sessions" | "Tasks" => palette().assistant,
        "Command Palette" => palette().header,
        _ => palette().border_active,
    }
}

fn value_style(key: &str, value: &str) -> Style {
    if key.contains("warning") {
        Style::default().fg(palette().warn)
    } else if key.contains("status") {
        if value.contains("completed") || value.contains("ready") {
            Style::default().fg(palette().assistant)
        } else if value.contains("cancel") || value.contains("failed") {
            Style::default().fg(palette().error)
        } else {
            Style::default().fg(palette().warn)
        }
    } else if key.contains("action") {
        if value.contains("sent")
            || value.contains("cancelled")
            || value.contains("reattached")
            || value.contains("started")
        {
            Style::default().fg(palette().assistant)
        } else {
            Style::default().fg(palette().warn)
        }
    } else if key.contains("sandbox") {
        Style::default().fg(palette().user)
    } else if key.contains("dirty") {
        if value.contains("modified 0")
            && value.contains("untracked 0")
            && value.contains("staged 0")
        {
            Style::default().fg(palette().assistant)
        } else {
            Style::default().fg(palette().warn)
        }
    } else if key.contains("queue") {
        if value.starts_with('0') {
            Style::default().fg(palette().assistant)
        } else {
            Style::default().fg(palette().warn)
        }
    } else if key.contains("active ref")
        || key.contains("runtime id")
        || key.contains("session ref")
        || key.contains("agent id")
        || key.contains("task id")
    {
        Style::default().fg(palette().user)
    } else if key.contains("summary") {
        Style::default().fg(palette().header)
    } else {
        Style::default().fg(palette().text)
    }
}

fn plain_text_style(line: &str) -> Style {
    if line.starts_with("Use /") {
        Style::default().fg(palette().muted)
    } else if line.starts_with("warning:") {
        Style::default().fg(palette().warn)
    } else if line.starts_with("diagnostic:") {
        Style::default().fg(palette().user)
    } else if line.starts_with("No ") || line.starts_with("no ") {
        Style::default().fg(palette().subtle)
    } else {
        Style::default().fg(palette().text)
    }
}

#[cfg(test)]
fn inspector_entry_text(entry: &InspectorEntry) -> String {
    match entry {
        InspectorEntry::Section(line)
        | InspectorEntry::Plain(line)
        | InspectorEntry::Muted(line)
        | InspectorEntry::Command(line) => line.clone(),
        InspectorEntry::Field { key, value } => format!("{key}: {value}"),
        InspectorEntry::Transcript(entry) => entry.serialized(),
        InspectorEntry::CollectionItem {
            primary, secondary, ..
        } => secondary
            .as_ref()
            .map(|secondary| format!("{primary}  {secondary}"))
            .unwrap_or_else(|| primary.clone()),
        InspectorEntry::Empty => String::new(),
    }
}

pub(super) fn collection_picker_footer(
    title: &str,
    selected: Option<&InspectorEntry>,
) -> Option<Line<'static>> {
    let InspectorEntry::CollectionItem {
        action,
        alternate_action,
        ..
    } = selected?
    else {
        return None;
    };
    let action_label = action
        .as_ref()
        .map(|action| collection_action_label(title, action))
        .unwrap_or("select");
    let mut spans = vec![
        Span::styled("enter", Style::default().fg(palette().accent)),
        Span::styled(
            format!(" {action_label}"),
            Style::default().fg(palette().muted),
        ),
    ];
    if let Some(alternate_action) = alternate_action {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            alternate_action.key_hint.clone(),
            Style::default().fg(palette().assistant),
        ));
        spans.push(Span::styled(
            format!(" {}", alternate_action.label),
            Style::default().fg(palette().muted),
        ));
    }
    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled("↑↓", Style::default().fg(palette().accent)));
    spans.push(Span::styled(" move", Style::default().fg(palette().muted)));
    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled("esc", Style::default().fg(palette().header)));
    spans.push(Span::styled(" close", Style::default().fg(palette().muted)));
    Some(Line::from(spans))
}

fn collection_action_label(title: &str, action: &InspectorAction) -> &'static str {
    if is_command_palette_title(title) {
        match action {
            InspectorAction::RunCommand(_) => "run",
            InspectorAction::FillInput(_) => "insert",
        }
    } else {
        match action {
            InspectorAction::RunCommand(_) => "open",
            InspectorAction::FillInput(_) => "insert",
        }
    }
}
