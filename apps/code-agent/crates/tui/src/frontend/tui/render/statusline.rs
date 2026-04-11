use super::super::state::{ToastTone, TuiState, preview_text};
use super::theme::palette;
use crate::backend::preview_id;
use chrono::Local;
use ratatui::layout::Margin;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

pub(super) fn render_status_line(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().footer_bg)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let status = Paragraph::new(format_footer_context(state))
        .style(Style::default().fg(palette().text).bg(palette().footer_bg))
        .wrap(Wrap { trim: true });
    frame.render_widget(status, inner);
}

pub(super) fn toast_height(state: &TuiState) -> Option<u16> {
    state.toast.as_ref().map(|_| 1)
}

pub(super) fn render_toast_band(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(palette().footer_bg)),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let toast = Paragraph::new(format_toast_line(state))
        .style(Style::default().fg(palette().text).bg(palette().footer_bg))
        .wrap(Wrap { trim: true });
    frame.render_widget(toast, inner);
}

pub(super) fn format_footer_context(state: &TuiState) -> Line<'static> {
    let config = &state.session.statusline;
    let status = if state.status.is_empty() {
        "Ready"
    } else {
        state.status.as_str()
    };

    let mut spans = Vec::new();
    if config.status {
        spans.push(Span::styled(
            if state.turn_running { "●" } else { "•" },
            Style::default()
                .fg(status_color(status))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            preview_text(status, 32),
            Style::default()
                .fg(palette().text)
                .add_modifier(Modifier::BOLD),
        ));
    }

    push_status_item(
        &mut spans,
        config.model.then(|| {
            (
                format_model_label(state),
                Style::default().fg(palette().accent),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.cwd.then(|| {
            (
                state.session.workspace_name.clone(),
                Style::default().fg(palette().text),
            )
        }),
    );
    push_status_item(
        &mut spans,
        (config.repo && state.session.git.available && !state.session.git.repo_name.is_empty())
            .then(|| {
                (
                    state.session.git.repo_name.clone(),
                    Style::default().fg(palette().user),
                )
            }),
    );
    push_status_item(
        &mut spans,
        (config.branch && state.session.git.available).then(|| {
            (
                state.session.git.branch.clone(),
                Style::default().fg(palette().muted),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.context_window.then(|| {
            (
                format_context_window_label(state),
                Style::default().fg(context_window_color(state)),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.input_tokens.then(|| {
            (
                format!(
                    "in {}",
                    compact_u64(state.session.token_ledger.cumulative_usage.input_tokens)
                ),
                Style::default().fg(palette().muted),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.output_tokens.then(|| {
            (
                format!(
                    "out {}",
                    compact_u64(state.session.token_ledger.cumulative_usage.output_tokens)
                ),
                Style::default().fg(palette().muted),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.queue.then(|| {
            (
                format!("queue {}", state.session.queued_commands),
                if state.session.queued_commands == 0 {
                    Style::default().fg(palette().muted)
                } else {
                    Style::default().fg(palette().warn)
                },
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.session.then(|| {
            (
                preview_id(&state.session.active_session_ref),
                Style::default().fg(palette().muted),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.clock.then(|| {
            (
                Local::now().format("%H:%M").to_string(),
                Style::default().fg(palette().muted),
            )
        }),
    );

    Line::from(spans)
}

pub(super) fn format_toast_line(state: &TuiState) -> Line<'static> {
    let Some(toast) = state.toast.as_ref() else {
        return Line::raw("");
    };
    let tone_color = match toast.tone {
        ToastTone::Info => palette().accent,
        ToastTone::Success => palette().assistant,
        ToastTone::Warning => palette().warn,
        ToastTone::Error => palette().error,
    };
    Line::from(vec![
        Span::styled(
            "●",
            Style::default().fg(tone_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("notice", Style::default().fg(tone_color)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&toast.message, 120),
            Style::default().fg(palette().text),
        ),
    ])
}

pub(super) fn status_color(status: &str) -> Color {
    let lower = status.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("failed") || lower.contains("denied") {
        palette().error
    } else if lower.contains("approval") || lower.contains("running") || lower.contains("waiting") {
        palette().warn
    } else if lower.contains("ready") || lower.contains("complete") || lower.contains("approved") {
        palette().assistant
    } else {
        palette().user
    }
}

fn push_status_item(spans: &mut Vec<Span<'static>>, item: Option<(String, Style)>) {
    let Some((label, style)) = item else {
        return;
    };
    if !spans.is_empty() {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
    }
    spans.push(Span::styled(label, style));
}

fn format_model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) if !effort.is_empty() => format!("{} ({effort})", state.session.model),
        _ => state.session.model.clone(),
    }
}

fn format_context_window_label(state: &TuiState) -> String {
    match state.session.token_ledger.context_window {
        Some(window) if window.max_tokens > 0 => {
            let usage = window.used_tokens.saturating_mul(100) / window.max_tokens;
            format!(
                "ctx {} / {} tok ({}%)",
                compact_usize(window.used_tokens),
                compact_usize(window.max_tokens),
                usage
            )
        }
        Some(window) => format!("ctx {} / 0 tok (--)", compact_usize(window.used_tokens)),
        None => "ctx --".to_string(),
    }
}

fn context_window_color(state: &TuiState) -> Color {
    let Some(window) = state.session.token_ledger.context_window else {
        return palette().subtle;
    };
    if window.max_tokens == 0 {
        return palette().subtle;
    }
    let usage = window.used_tokens.saturating_mul(100) / window.max_tokens;
    if usage >= 85 {
        palette().warn
    } else {
        palette().assistant
    }
}

fn compact_usize(value: usize) -> String {
    compact_u64(value as u64)
}

fn compact_u64(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => compact_with_suffix(value, 1_000, "k"),
        1_000_000..=999_999_999 => compact_with_suffix(value, 1_000_000, "m"),
        _ => compact_with_suffix(value, 1_000_000_000, "b"),
    }
}

fn compact_with_suffix(value: u64, divisor: u64, suffix: &str) -> String {
    if value % divisor == 0 {
        format!("{}{}", value / divisor, suffix)
    } else {
        format!("{:.1}{suffix}", value as f64 / divisor as f64).replace(".0", "")
    }
}
