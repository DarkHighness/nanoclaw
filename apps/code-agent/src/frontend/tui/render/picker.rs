use super::super::state::{
    PendingControlEditorState, PendingControlPickerState, TuiState, preview_text,
};
use super::shared::{
    pending_control_focus_label, pending_control_kind_label,
    pending_control_reason_label as format_pending_control_reason,
};
use super::shell::bottom_band_inner_area;
use super::theme::palette;
use crate::frontend::tui::commands::{SlashCommandHint, SlashCommandSpec};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

pub(super) fn render_command_hint_band(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    command_hint: &SlashCommandHint,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().bottom_pane_bg)),
        area,
    );
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(build_command_hint_text(command_hint))
            .wrap(Wrap { trim: false })
            .style(
                Style::default()
                    .fg(palette().text)
                    .bg(palette().bottom_pane_bg),
            ),
        inner,
    );
}

pub(super) fn build_command_hint_text(command_hint: &SlashCommandHint) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled("commands", Style::default().fg(palette().header)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!("{} matches", command_hint.matches.len()),
            Style::default().fg(palette().accent),
        ),
    ])];

    let window = visible_command_match_window(command_hint, 4);
    if window.start > 0 {
        lines.push(Line::from(Span::styled(
            format!("… {} earlier", window.start),
            Style::default().fg(palette().subtle),
        )));
    }

    for spec in window.items {
        if spec.name == command_hint.selected.name {
            lines.push(Line::from(vec![
                Span::styled(
                    "›",
                    Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("/{}", spec.usage),
                    Style::default()
                        .fg(palette().header)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(spec.summary, Style::default().fg(palette().text)),
            ]));
            if !spec.aliases().is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("  aliases ", Style::default().fg(palette().subtle)),
                    Span::styled(
                        spec.aliases()
                            .iter()
                            .map(|alias| format!("/{alias}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                        Style::default().fg(palette().muted),
                    ),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(
                    format!("/{}", spec.usage),
                    Style::default().fg(palette().muted),
                ),
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(spec.section, Style::default().fg(palette().subtle)),
            ]));
        }
    }

    if let Some(arguments) = command_hint.arguments.as_ref() {
        let mut spans = Vec::new();
        if arguments.provided.is_empty() {
            if let Some(next) = arguments.next {
                spans.push(Span::styled(
                    "  next ",
                    Style::default().fg(palette().subtle),
                ));
                spans.push(Span::styled(
                    next.placeholder,
                    Style::default().fg(palette().muted),
                ));
            }
        } else {
            spans.push(Span::styled("  ", Style::default().fg(palette().subtle)));
            for (index, argument) in arguments.provided.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
                }
                spans.push(Span::styled(
                    argument.placeholder,
                    Style::default().fg(palette().subtle),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    argument.value.clone(),
                    Style::default().fg(palette().text),
                ));
            }
            if let Some(next) = arguments.next {
                spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
                spans.push(Span::styled("next ", Style::default().fg(palette().subtle)));
                spans.push(Span::styled(
                    next.placeholder,
                    Style::default().fg(palette().muted),
                ));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }

    if window.end < command_hint.matches.len() {
        lines.push(Line::from(Span::styled(
            format!("… {} more", command_hint.matches.len() - window.end),
            Style::default().fg(palette().subtle),
        )));
    }

    let tab_hint = if command_hint.exact {
        if command_hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some_and(|argument| argument.required)
        {
            "keep typing"
        } else if command_hint.matches.len() > 1 {
            "tab next"
        } else {
            "enter run"
        }
    } else {
        "tab complete"
    };
    let enter_hint = if command_hint.exact {
        if command_hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some_and(|argument| argument.required)
        {
            "keep typing"
        } else {
            "enter run"
        }
    } else if command_hint.matches.len() == 1 && !command_hint.selected.requires_arguments() {
        "enter run"
    } else {
        "enter accept"
    };
    lines.push(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(palette().muted)),
        Span::styled(" move", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(tab_hint, Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("shift+tab previous", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(enter_hint, Style::default().fg(palette().muted)),
    ]));

    Text::from(lines)
}

pub(super) fn command_hint_height(command_hint: &SlashCommandHint) -> u16 {
    build_command_hint_text(command_hint)
        .lines
        .len()
        .clamp(2, 9) as u16
}

pub(super) fn render_pending_control_band(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().bottom_pane_bg)),
        area,
    );
    let inner = bottom_band_inner_area(area);
    frame.render_widget(
        Paragraph::new(build_pending_control_text(state))
            .wrap(Wrap { trim: false })
            .style(
                Style::default()
                    .fg(palette().text)
                    .bg(palette().bottom_pane_bg),
            ),
        inner,
    );
}

pub(super) fn pending_control_height(state: &TuiState) -> Option<u16> {
    if state.pending_controls.is_empty() {
        return None;
    }
    Some(build_pending_control_text(state).lines.len().clamp(2, 8) as u16)
}

pub(super) fn build_pending_control_text(state: &TuiState) -> Text<'static> {
    let editing = state.editing_pending_control.as_ref();
    let selected = state.selected_pending_control();
    let pending_count = state.pending_controls.len();
    let mut lines = vec![Line::from(vec![
        Span::styled("pending", Style::default().fg(palette().header)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!(
                "{pending_count} item{}",
                if pending_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(palette().warn),
        ),
        if editing.is_some() {
            Span::styled(" · ", Style::default().fg(palette().subtle))
        } else {
            Span::raw("")
        },
        if let Some(editing) = editing {
            Span::styled(
                format!("editing {}", pending_kind_label(editing)),
                Style::default().fg(palette().accent),
            )
        } else {
            Span::raw("")
        },
    ])];

    let picker = state.pending_control_picker.as_ref();
    if let Some(picker) = picker {
        let window = visible_pending_control_window(&state.pending_controls, picker, 3);
        if window.start > 0 {
            lines.push(Line::from(Span::styled(
                format!("… {} older", window.start),
                Style::default().fg(palette().subtle),
            )));
        }
        let selected_index = picker.selected;
        for (index, control) in window.items.iter().enumerate() {
            let actual_index = window.start + index;
            if actual_index == selected_index {
                continue;
            }
            lines.push(build_pending_control_context_row(control));
        }
        if window.end < state.pending_controls.len() {
            lines.push(Line::from(Span::styled(
                format!("… {} newer", state.pending_controls.len() - window.end),
                Style::default().fg(palette().subtle),
            )));
        }
        if let Some(selected) = state.pending_controls.get(selected_index) {
            lines.extend(build_selected_pending_control_block(
                selected,
                selected_index,
                state.pending_controls.len(),
            ));
        }
    } else if let Some(selected) = selected.or_else(|| state.pending_controls.last().cloned()) {
        lines.push(build_pending_control_row(&selected, true));
        lines.push(Line::from(Span::styled(
            "alt+up open queue",
            Style::default().fg(palette().subtle),
        )));
    }

    Text::from(lines)
}

fn build_pending_control_row(
    control: &crate::backend::PendingControlSummary,
    selected: bool,
) -> Line<'static> {
    let marker = if selected { "›" } else { " " };
    let kind_label = pending_control_kind_label(control.kind);
    let accent = match control.kind {
        crate::backend::PendingControlKind::Prompt => palette().user,
        crate::backend::PendingControlKind::Steer => palette().assistant,
    };
    let mut spans = vec![
        Span::styled(
            marker,
            Style::default()
                .fg(if selected {
                    palette().accent
                } else {
                    palette().subtle
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{kind_label:<6}"),
            Style::default()
                .fg(if selected { accent } else { palette().muted })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&control.preview, 72),
            Style::default().fg(if selected {
                palette().header
            } else {
                palette().text
            }),
        ),
    ];
    if let Some(reason) = control.reason.as_deref() {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            preview_text(
                &format_pending_control_reason(Some(reason)).unwrap_or_else(|| reason.to_string()),
                24,
            ),
            Style::default().fg(palette().muted),
        ));
    }
    Line::from(spans)
}

fn build_pending_control_context_row(
    control: &crate::backend::PendingControlSummary,
) -> Line<'static> {
    let kind_label = pending_control_kind_label(control.kind);
    Line::from(vec![
        Span::styled("  ", Style::default().fg(palette().subtle)),
        Span::styled(kind_label, Style::default().fg(palette().muted)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&control.preview, 56),
            Style::default().fg(palette().text),
        ),
    ])
}

fn build_selected_pending_control_block(
    control: &crate::backend::PendingControlSummary,
    selected_index: usize,
    total: usize,
) -> Vec<Line<'static>> {
    let kind_label = pending_control_kind_label(control.kind);
    let accent = match control.kind {
        crate::backend::PendingControlKind::Prompt => palette().user,
        crate::backend::PendingControlKind::Steer => palette().assistant,
    };
    vec![
        Line::from(vec![
            Span::styled(
                "›",
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                kind_label,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(palette().subtle)),
            Span::styled(
                pending_control_focus_label(selected_index, total),
                Style::default().fg(palette().accent),
            ),
        ]),
        Line::from(vec![
            Span::styled("  ", Style::default().fg(palette().subtle)),
            Span::styled(
                preview_text(&control.preview, 84),
                Style::default().fg(palette().header),
            ),
        ]),
        build_pending_control_detail_row(control, selected_index, total),
    ]
}

fn build_pending_control_detail_row(
    control: &crate::backend::PendingControlSummary,
    selected_index: usize,
    total: usize,
) -> Line<'static> {
    let mut spans = vec![
        Span::styled("  ", Style::default().fg(palette().subtle)),
        Span::styled(
            pending_control_queue_position_label(selected_index, total),
            Style::default().fg(palette().subtle),
        ),
    ];
    if let Some(reason) = format_pending_control_reason(control.reason.as_deref()) {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(reason, Style::default().fg(palette().muted)));
    }
    Line::from(spans)
}

fn pending_kind_label(editing: &PendingControlEditorState) -> &'static str {
    match editing.kind {
        crate::backend::PendingControlKind::Prompt => "queued prompt",
        crate::backend::PendingControlKind::Steer => "queued steer",
    }
}

fn pending_control_queue_position_label(selected_index: usize, total: usize) -> String {
    match (selected_index, total) {
        (_, 0) => "no queued work".to_string(),
        (0, _) => "runs next".to_string(),
        (index, count) if index + 1 == count => format!("after {} older item(s)", index),
        (index, _) => format!("after {} older item(s)", index),
    }
}

struct VisiblePendingControlWindow<'a> {
    start: usize,
    end: usize,
    items: &'a [crate::backend::PendingControlSummary],
}

fn visible_pending_control_window<'a>(
    controls: &'a [crate::backend::PendingControlSummary],
    picker: &PendingControlPickerState,
    max_items: usize,
) -> VisiblePendingControlWindow<'a> {
    let total = controls.len();
    let window = total.min(max_items.max(1));
    let mut start = picker.selected.saturating_add(1).saturating_sub(window);
    let end = (start + window).min(total);
    if end - start < window {
        start = end.saturating_sub(window);
    }
    VisiblePendingControlWindow {
        start,
        end,
        items: &controls[start..end],
    }
}

struct VisibleCommandMatchWindow<'a> {
    start: usize,
    end: usize,
    items: &'a [SlashCommandSpec],
}

fn visible_command_match_window(
    command_hint: &SlashCommandHint,
    max_items: usize,
) -> VisibleCommandMatchWindow<'_> {
    let total = command_hint.matches.len();
    let window = total.min(max_items.max(1));
    let mut start = command_hint
        .selected_match_index
        .saturating_add(1)
        .saturating_sub(window);
    let end = (start + window).min(total);
    if end - start < window {
        start = end.saturating_sub(window);
    }
    VisibleCommandMatchWindow {
        start,
        end,
        items: &command_hint.matches[start..end],
    }
}
