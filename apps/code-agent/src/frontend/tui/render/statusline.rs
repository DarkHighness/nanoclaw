use super::super::state::{TuiState, preview_text};
use super::theme::{ACCENT, ASSISTANT, ERROR, FOOTER_BG, MUTED, SUBTLE, TEXT, USER, WARN};
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
    frame.render_widget(Block::default().style(Style::default().bg(FOOTER_BG)), area);
    let inner = area.inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let status = Paragraph::new(format_footer_context(state))
        .style(Style::default().fg(TEXT).bg(FOOTER_BG))
        .wrap(Wrap { trim: true });
    frame.render_widget(status, inner);
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
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
    }

    push_status_item(
        &mut spans,
        config
            .model
            .then(|| (format_model_label(state), Style::default().fg(ACCENT))),
    );
    push_status_item(
        &mut spans,
        config.cwd.then(|| {
            (
                state.session.workspace_name.clone(),
                Style::default().fg(TEXT),
            )
        }),
    );
    push_status_item(
        &mut spans,
        (config.repo && state.session.git.available && !state.session.git.repo_name.is_empty())
            .then(|| {
                (
                    state.session.git.repo_name.clone(),
                    Style::default().fg(USER),
                )
            }),
    );
    push_status_item(
        &mut spans,
        (config.branch && state.session.git.available)
            .then(|| (state.session.git.branch.clone(), Style::default().fg(MUTED))),
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
                Style::default().fg(MUTED),
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
                Style::default().fg(MUTED),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.queue.then(|| {
            (
                format!("queue {}", state.session.queued_commands),
                if state.session.queued_commands == 0 {
                    Style::default().fg(MUTED)
                } else {
                    Style::default().fg(WARN)
                },
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.session.then(|| {
            (
                preview_id(&state.session.active_session_ref),
                Style::default().fg(MUTED),
            )
        }),
    );
    push_status_item(
        &mut spans,
        config.clock.then(|| {
            (
                Local::now().format("%H:%M").to_string(),
                Style::default().fg(MUTED),
            )
        }),
    );

    Line::from(spans)
}

pub(super) fn status_color(status: &str) -> Color {
    let lower = status.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("failed") || lower.contains("denied") {
        ERROR
    } else if lower.contains("approval") || lower.contains("running") || lower.contains("waiting") {
        WARN
    } else if lower.contains("ready") || lower.contains("complete") || lower.contains("approved") {
        ASSISTANT
    } else {
        USER
    }
}

fn push_status_item(spans: &mut Vec<Span<'static>>, item: Option<(String, Style)>) {
    let Some((label, style)) = item else {
        return;
    };
    if !spans.is_empty() {
        spans.push(Span::styled(" · ", Style::default().fg(SUBTLE)));
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
        Some(window) => format!(
            "ctx {}/{}",
            compact_usize(window.used_tokens),
            compact_usize(window.max_tokens)
        ),
        None => "ctx --".to_string(),
    }
}

fn context_window_color(state: &TuiState) -> Color {
    let Some(window) = state.session.token_ledger.context_window else {
        return SUBTLE;
    };
    if window.max_tokens == 0 {
        return SUBTLE;
    }
    let usage = window.used_tokens.saturating_mul(100) / window.max_tokens;
    if usage >= 85 { WARN } else { ASSISTANT }
}

fn compact_usize(value: usize) -> String {
    compact_u64(value as u64)
}

fn compact_u64(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => format!("{:.1}k", value as f64 / 1_000.0)
            .trim_end_matches(".0k")
            .to_string()
            .replace(".k", "k"),
        1_000_000..=999_999_999 => format!("{:.1}m", value as f64 / 1_000_000.0)
            .trim_end_matches(".0m")
            .to_string()
            .replace(".m", "m"),
        _ => format!("{:.1}b", value as f64 / 1_000_000_000.0)
            .trim_end_matches(".0b")
            .to_string()
            .replace(".b", "b"),
    }
}
