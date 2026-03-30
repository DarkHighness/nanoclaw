use super::theme::{ACCENT, BOTTOM_PANE_BG, HEADER, MUTED, SUBTLE, TEXT};
use crate::frontend::tui::commands::{SlashCommandHint, SlashCommandSpec};
use ratatui::layout::Margin;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

pub(super) fn render_command_hint_band(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    command_hint: &SlashCommandHint,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(build_command_hint_text(command_hint))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        inner,
    );
}

pub(super) fn build_command_hint_text(command_hint: &SlashCommandHint) -> Text<'static> {
    let mut lines = vec![Line::from(vec![
        Span::styled("commands", Style::default().fg(HEADER)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            format!("{} matches", command_hint.matches.len()),
            Style::default().fg(ACCENT),
        ),
    ])];

    let window = visible_command_match_window(command_hint, 4);
    if window.start > 0 {
        lines.push(Line::from(Span::styled(
            format!("… {} earlier", window.start),
            Style::default().fg(SUBTLE),
        )));
    }

    for spec in window.items {
        if spec.name == command_hint.selected.name {
            lines.push(Line::from(vec![
                Span::styled(
                    "›",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("/{}", spec.usage),
                    Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(spec.summary, Style::default().fg(TEXT)),
            ]));
            if !spec.aliases().is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("  aliases ", Style::default().fg(SUBTLE)),
                    Span::styled(
                        spec.aliases()
                            .iter()
                            .map(|alias| format!("/{alias}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                        Style::default().fg(MUTED),
                    ),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(format!("/{}", spec.usage), Style::default().fg(MUTED)),
                Span::styled("  ", Style::default().fg(SUBTLE)),
                Span::styled(spec.section, Style::default().fg(SUBTLE)),
            ]));
        }
    }

    if let Some(arguments) = command_hint.arguments.as_ref() {
        let mut spans = Vec::new();
        if arguments.provided.is_empty() {
            if let Some(next) = arguments.next {
                spans.push(Span::styled("  next ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled(next.placeholder, Style::default().fg(MUTED)));
            }
        } else {
            spans.push(Span::styled("  ", Style::default().fg(SUBTLE)));
            for (index, argument) in arguments.provided.iter().enumerate() {
                if index > 0 {
                    spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                }
                spans.push(Span::styled(
                    argument.placeholder,
                    Style::default().fg(SUBTLE),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    argument.value.clone(),
                    Style::default().fg(TEXT),
                ));
            }
            if let Some(next) = arguments.next {
                spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled("next ", Style::default().fg(SUBTLE)));
                spans.push(Span::styled(next.placeholder, Style::default().fg(MUTED)));
            }
        }
        if !spans.is_empty() {
            lines.push(Line::from(spans));
        }
    }

    if window.end < command_hint.matches.len() {
        lines.push(Line::from(Span::styled(
            format!("… {} more", command_hint.matches.len() - window.end),
            Style::default().fg(SUBTLE),
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
        Span::styled("↑↓", Style::default().fg(MUTED)),
        Span::styled(" move", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(tab_hint, Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("shift+tab previous", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(enter_hint, Style::default().fg(MUTED)),
    ]));

    Text::from(lines)
}

pub(super) fn command_hint_height(command_hint: &SlashCommandHint) -> u16 {
    build_command_hint_text(command_hint)
        .lines
        .len()
        .clamp(2, 9) as u16
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
