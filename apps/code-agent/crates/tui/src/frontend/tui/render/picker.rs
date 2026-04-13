use super::super::state::{
    PendingControlEditorState, PendingControlPickerState, TuiState, preview_text,
};
use super::shared::{
    PendingControlKindSummary, pending_control_focus_label, pending_control_kind_summaries,
    pending_control_reason_label as format_pending_control_reason, pending_controls_have_kind,
};
use super::shell::bottom_band_inner_area;
use super::theme::palette;
use crate::frontend::tui::commands::{
    ComposerCompletionHint, SkillInvocationHint, SkillInvocationSpec, SlashCommandHint,
    SlashInvocationSpec,
};
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub(super) fn render_composer_hint_modal(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    composer_hint: &ComposerCompletionHint,
) {
    let height = build_composer_hint_text(composer_hint)
        .lines
        .len()
        .saturating_add(3)
        .clamp(8, 16) as u16;
    let popup = centered_rect(area, 78, height.min(area.height.saturating_sub(2)).max(8));
    frame.render_widget(Clear, popup);
    let title = match composer_hint {
        ComposerCompletionHint::Slash(_) => " Commands ",
        ComposerCompletionHint::Skill(_) => " Skills ",
    };
    frame.render_widget(
        Block::default()
            .title(title)
            .title_style(
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette().border_active))
            .style(Style::default().bg(palette().footer_bg)),
        popup,
    );
    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_composer_hint_text(composer_hint))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(palette().text).bg(palette().footer_bg)),
        inner,
    );
}

pub(super) fn build_composer_hint_text(composer_hint: &ComposerCompletionHint) -> Text<'static> {
    match composer_hint {
        ComposerCompletionHint::Slash(command_hint) => build_command_hint_text(command_hint),
        ComposerCompletionHint::Skill(skill_hint) => build_skill_hint_text(skill_hint),
    }
}

fn build_command_hint_text(command_hint: &SlashCommandHint) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "Commands",
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!("{} Matches", command_hint.matches.len()),
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
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
        if spec.name() == command_hint.selected.name() {
            lines.push(Line::from(vec![
                Span::styled(
                    "›",
                    Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    spec.usage(),
                    Style::default()
                        .fg(palette().header)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(spec.summary(), Style::default().fg(palette().text)),
            ]));
            let aliases = spec.aliases();
            if !aliases.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("  Aliases ", Style::default().fg(palette().subtle)),
                    Span::styled(
                        aliases
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
                Span::styled(spec.usage(), Style::default().fg(palette().muted)),
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(spec.section(), Style::default().fg(palette().subtle)),
            ]));
        }
    }

    if let Some(arguments) = command_hint.arguments.as_ref() {
        let mut spans = Vec::new();
        if arguments.provided.is_empty() {
            if let Some(next) = arguments.next {
                spans.push(Span::styled(
                    "  Next ",
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
                spans.push(Span::styled("Next ", Style::default().fg(palette().subtle)));
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
            "Keep Typing"
        } else if command_hint.matches.len() > 1 {
            "Tab Next"
        } else if command_hint.selected.executable_input().is_some() {
            "Enter Run"
        } else {
            "Enter Accept"
        }
    } else {
        "Tab Complete"
    };
    let enter_hint = if command_hint.exact {
        if command_hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some_and(|argument| argument.required)
        {
            "Keep Typing"
        } else if command_hint.selected.executable_input().is_some() {
            "Enter Run"
        } else {
            "Enter Accept"
        }
    } else if command_hint.matches.len() == 1 && command_hint.selected.executable_input().is_some()
    {
        "Enter Run"
    } else {
        "Enter Accept"
    };
    lines.push(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(palette().muted)),
        Span::styled(" Move", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(tab_hint, Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Shift+Tab Previous", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(enter_hint, Style::default().fg(palette().muted)),
    ]));

    Text::from(lines)
}

fn build_skill_hint_text(skill_hint: &SkillInvocationHint) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "Skills",
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!("{} Matches", skill_hint.matches.len()),
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ),
    ])];

    let window = visible_skill_match_window(skill_hint, 4);
    if window.start > 0 {
        lines.push(Line::from(Span::styled(
            format!("… {} earlier", window.start),
            Style::default().fg(palette().subtle),
        )));
    }

    for spec in window.items {
        if spec.name == skill_hint.selected.name {
            lines.push(Line::from(vec![
                Span::styled(
                    "›",
                    Style::default()
                        .fg(palette().accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    spec.invocation(),
                    Style::default()
                        .fg(palette().header)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(
                    spec.description.clone(),
                    Style::default().fg(palette().text),
                ),
            ]));
            if !spec.aliases.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("  Aliases ", Style::default().fg(palette().subtle)),
                    Span::styled(
                        spec.aliases
                            .iter()
                            .map(|alias| format!("${alias}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                        Style::default().fg(palette().muted),
                    ),
                ]));
            }
            if !spec.tags.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("  Tags ", Style::default().fg(palette().subtle)),
                    Span::styled(spec.tags.join(" · "), Style::default().fg(palette().muted)),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(spec.invocation(), Style::default().fg(palette().muted)),
                Span::styled("  ", Style::default().fg(palette().subtle)),
                Span::styled(
                    spec.description.clone(),
                    Style::default().fg(palette().subtle),
                ),
            ]));
        }
    }

    if window.end < skill_hint.matches.len() {
        lines.push(Line::from(Span::styled(
            format!("… {} more", skill_hint.matches.len() - window.end),
            Style::default().fg(palette().subtle),
        )));
    }

    lines.push(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(palette().muted)),
        Span::styled(" Move", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Tab Use", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Shift+Tab Previous", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Enter Use", Style::default().fg(palette().muted)),
    ]));

    Text::from(lines)
}

fn centered_rect(area: Rect, width_percent: u16, height: u16) -> Rect {
    let popup_height = height.min(area.height.saturating_sub(2)).max(1);
    let vertical_margin = area.height.saturating_sub(popup_height) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Length(popup_height),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(width_percent)) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100_u16.saturating_sub(width_percent)) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
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
        Span::styled(
            "Queued Follow-ups",
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
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
                format!("Editing {}", pending_kind_label(editing)),
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
                state,
                selected,
                selected_index,
                state.pending_controls.len(),
            ));
        }
    } else if pending_control_kind_summaries(&state.pending_controls).len() > 1 {
        lines.extend(build_pending_control_kind_overview(state));
    } else if let Some(selected) = selected.or_else(|| state.pending_controls.last().cloned()) {
        lines.extend(build_latest_pending_control_block(state, &selected));
    }

    Text::from(lines)
}

fn build_pending_control_row(control: &PendingControlSummary, selected: bool) -> Line<'static> {
    let marker = if selected { "›" } else { "•" };
    let kind_label = pending_control_summary_label(control);
    let accent = match control.kind {
        PendingControlKind::Prompt => palette().user,
        PendingControlKind::Steer => palette().assistant,
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
            kind_label,
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
    if let Some(reason) = control.reason.as_ref() {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            preview_text(
                &format_pending_control_reason(Some(reason)).unwrap_or_default(),
                24,
            ),
            Style::default().fg(palette().muted),
        ));
    }
    Line::from(spans)
}

fn build_pending_control_context_row(control: &PendingControlSummary) -> Line<'static> {
    let kind_label = pending_control_summary_label(control);
    Line::from(vec![
        Span::styled("  • ", Style::default().fg(palette().subtle)),
        Span::styled(kind_label, Style::default().fg(palette().muted)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&control.preview, 56),
            Style::default().fg(palette().text),
        ),
    ])
}

fn build_selected_pending_control_block(
    state: &TuiState,
    control: &PendingControlSummary,
    selected_index: usize,
    total: usize,
) -> Vec<Line<'static>> {
    let kind_label = pending_control_heading_label(control, state.turn_running);
    let accent = match control.kind {
        PendingControlKind::Prompt => palette().user,
        PendingControlKind::Steer => palette().assistant,
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
        build_pending_control_picker_hint_line(control),
    ]
}

fn build_latest_pending_control_block(
    state: &TuiState,
    control: &PendingControlSummary,
) -> Vec<Line<'static>> {
    vec![
        build_pending_control_row(control, true),
        build_pending_control_latest_hint_line(state, control),
    ]
}

fn build_pending_control_kind_overview(state: &TuiState) -> Vec<Line<'static>> {
    let mut lines = pending_control_kind_summaries(&state.pending_controls)
        .into_iter()
        .map(|summary| build_pending_control_kind_summary_line(state, summary))
        .collect::<Vec<_>>();
    lines.push(build_pending_control_kind_overview_hint_line(state));
    lines
}

fn build_pending_control_kind_summary_line(
    state: &TuiState,
    summary: PendingControlKindSummary<'_>,
) -> Line<'static> {
    let accent = match summary.kind {
        PendingControlKind::Prompt => palette().user,
        PendingControlKind::Steer => palette().assistant,
    };
    let mut spans = vec![
        Span::styled(
            "›",
            Style::default()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            pending_control_heading_label(summary.latest, state.turn_running),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&summary.latest.preview, 44),
            Style::default().fg(palette().header),
        ),
    ];
    if summary.count > 1 {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            format!("{} items", summary.count),
            Style::default().fg(palette().muted),
        ));
    }
    if let Some(reason) = format_pending_control_reason(summary.latest.reason.as_ref()) {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            preview_text(&reason, 20),
            Style::default().fg(palette().muted),
        ));
    }
    if summary.latest_index == 0 {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled("next", Style::default().fg(palette().accent)));
    }
    Line::from(spans)
}

fn build_pending_control_kind_overview_hint_line(state: &TuiState) -> Line<'static> {
    let mut spans = vec![Span::styled("  ", Style::default().fg(palette().subtle))];
    if state.turn_running
        && pending_controls_have_kind(&state.pending_controls, PendingControlKind::Steer)
    {
        spans.push(Span::styled("Esc", Style::default().fg(palette().header)));
        spans.push(Span::styled(
            " send now",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    }
    spans.push(Span::styled("Alt+T", Style::default().fg(palette().accent)));
    spans.push(Span::styled(
        " edit latest",
        Style::default().fg(palette().muted),
    ));
    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled("Alt+↑", Style::default().fg(palette().header)));
    spans.push(Span::styled(" queue", Style::default().fg(palette().muted)));
    Line::from(spans)
}

fn build_pending_control_detail_row(
    control: &PendingControlSummary,
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
    if let Some(reason) = format_pending_control_reason(control.reason.as_ref()) {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(reason, Style::default().fg(palette().muted)));
    }
    Line::from(spans)
}

fn build_pending_control_picker_hint_line(control: &PendingControlSummary) -> Line<'static> {
    let mut spans = vec![
        Span::styled("  ", Style::default().fg(palette().subtle)),
        Span::styled("Alt+T", Style::default().fg(palette().accent)),
        Span::styled(" edit", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Del", Style::default().fg(palette().header)),
        Span::styled(" withdraw", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Esc", Style::default().fg(palette().header)),
        Span::styled(" close", Style::default().fg(palette().muted)),
    ];
    if control.kind == PendingControlKind::Steer {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled("Enter", Style::default().fg(palette().accent)));
        spans.push(Span::styled(
            " edit steer",
            Style::default().fg(palette().muted),
        ));
    }
    Line::from(spans)
}

fn build_pending_control_latest_hint_line(
    state: &TuiState,
    control: &PendingControlSummary,
) -> Line<'static> {
    let mut spans = vec![Span::styled("  ", Style::default().fg(palette().subtle))];
    if control.kind == PendingControlKind::Steer && state.turn_running {
        spans.push(Span::styled("Esc", Style::default().fg(palette().header)));
        spans.push(Span::styled(
            " send now",
            Style::default().fg(palette().muted),
        ));
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    }
    spans.push(Span::styled("Alt+T", Style::default().fg(palette().accent)));
    spans.push(Span::styled(" edit", Style::default().fg(palette().muted)));
    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled("Alt+↑", Style::default().fg(palette().header)));
    spans.push(Span::styled(" queue", Style::default().fg(palette().muted)));
    Line::from(spans)
}

fn pending_kind_label(editing: &PendingControlEditorState) -> &'static str {
    match editing.kind {
        PendingControlKind::Prompt => "Queued Prompt",
        PendingControlKind::Steer => "Queued Steer",
    }
}

fn pending_control_heading_label(
    control: &PendingControlSummary,
    turn_running: bool,
) -> &'static str {
    match (control.kind, turn_running) {
        (PendingControlKind::Prompt, _) => "Queued Prompt",
        (PendingControlKind::Steer, true) => "Steer Ready",
        (PendingControlKind::Steer, false) => "Queued Steer",
    }
}

fn pending_control_summary_label(control: &PendingControlSummary) -> &'static str {
    match control.kind {
        PendingControlKind::Prompt => "prompt",
        PendingControlKind::Steer => "steer",
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
    items: &'a [PendingControlSummary],
}

fn visible_pending_control_window<'a>(
    controls: &'a [PendingControlSummary],
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
    items: &'a [SlashInvocationSpec],
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

struct VisibleSkillMatchWindow<'a> {
    start: usize,
    end: usize,
    items: &'a [SkillInvocationSpec],
}

fn visible_skill_match_window(
    skill_hint: &SkillInvocationHint,
    max_items: usize,
) -> VisibleSkillMatchWindow<'_> {
    let total = skill_hint.matches.len();
    let window = total.min(max_items.max(1));
    let mut start = skill_hint
        .selected_match_index
        .saturating_add(1)
        .saturating_sub(window);
    let end = (start + window).min(total);
    if end - start < window {
        start = end.saturating_sub(window);
    }
    VisibleSkillMatchWindow {
        start,
        end,
        items: &skill_hint.matches[start..end],
    }
}

use crate::interaction::{PendingControlKind, PendingControlSummary};
