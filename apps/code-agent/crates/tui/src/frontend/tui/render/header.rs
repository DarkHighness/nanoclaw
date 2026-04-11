use super::super::state::{TuiState, preview_text};
use super::theme::palette;
use crate::backend::preview_id;
use ratatui::layout::Margin;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};

const HEADER_HEIGHT: u16 = 3;

pub(super) fn header_height() -> u16 {
    HEADER_HEIGHT
}

pub(super) fn render_header(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().bottom_pane_bg)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let header = Paragraph::new(build_header_text(state))
        .style(
            Style::default()
                .fg(palette().text)
                .bg(palette().bottom_pane_bg),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(header, inner);
}

fn build_header_text(state: &TuiState) -> Text<'static> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "Code Agent",
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&state.session.workspace_name, 28),
            Style::default().fg(palette().text),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_id(&state.session.active_session_ref),
            Style::default().fg(palette().muted),
        ),
    ]));

    let mut meta = vec![Span::styled(
        format_model_label(state),
        Style::default().fg(palette().accent),
    )];
    push_meta_span(
        &mut meta,
        format!("perm {}", state.session.permission_mode.as_str()),
        palette().muted,
    );
    if state.session.git.available && !state.session.git.branch.is_empty() {
        push_meta_span(
            &mut meta,
            format!("git {}", state.session.git.branch),
            palette().user,
        );
    }
    push_meta_span(
        &mut meta,
        format!("queue {}", state.session.queued_commands),
        if state.session.queued_commands == 0 {
            palette().muted
        } else {
            palette().warn
        },
    );
    lines.push(Line::from(meta));

    lines.push(Line::from(vec![
        Span::styled(
            preview_text(&state.status, 48),
            Style::default().fg(palette().text),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Enter", Style::default().fg(palette().accent)),
        Span::styled(" run", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Tab", Style::default().fg(palette().accent)),
        Span::styled(" queue", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("/help", Style::default().fg(palette().accent)),
        Span::styled(" commands", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("^T", Style::default().fg(palette().accent)),
        Span::styled(" effort", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("^O", Style::default().fg(palette().accent)),
        Span::styled(" editor", Style::default().fg(palette().muted)),
    ]));

    Text::from(lines)
}

fn format_model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) if !effort.is_empty() => {
            format!(
                "{} / {} ({effort})",
                state.session.provider_label, state.session.model
            )
        }
        _ => format!("{} / {}", state.session.provider_label, state.session.model),
    }
}

fn push_meta_span(spans: &mut Vec<Span<'static>>, text: String, color: ratatui::style::Color) {
    spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    spans.push(Span::styled(text, Style::default().fg(color)));
}
