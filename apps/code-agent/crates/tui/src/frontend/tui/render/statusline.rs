use super::super::state::{ToastTone, TuiState, TurnPhase, preview_text};
use super::theme::palette;
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
        horizontal: 2,
    });
    let status = Paragraph::new(format_footer_context(state))
        .style(Style::default().fg(palette().muted).bg(palette().footer_bg))
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
        horizontal: 2,
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
        push_badge(
            &mut spans,
            vec![
                Span::styled(
                    if state.turn_running { "●" } else { "•" },
                    Style::default()
                        .fg(status_color(state))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    preview_text(status, 32),
                    Style::default()
                        .fg(palette().text)
                        .add_modifier(Modifier::BOLD),
                ),
            ],
        );
    }

    push_labeled_badge(
        &mut spans,
        config.model,
        "model",
        format_model_label(state),
        Style::default().fg(palette().accent),
    );
    push_labeled_badge(
        &mut spans,
        config.cwd,
        "workspace",
        state.session.workspace_name.clone(),
        Style::default().fg(palette().text),
    );

    // Keep the picker's individual toggles intact, but collapse obviously
    // paired fields into one capsule so the footer scans like grouped state
    // instead of a single sentence that the operator must parse every turn.
    if config.repo
        && config.branch
        && state.session.git.available
        && !state.session.git.repo_name.is_empty()
    {
        push_labeled_badge(
            &mut spans,
            true,
            "git",
            format!(
                "{}@{}",
                state.session.git.repo_name, state.session.git.branch
            ),
            Style::default().fg(palette().user),
        );
    } else {
        push_labeled_badge(
            &mut spans,
            config.repo && state.session.git.available && !state.session.git.repo_name.is_empty(),
            "repo",
            state.session.git.repo_name.clone(),
            Style::default().fg(palette().user),
        );
        push_labeled_badge(
            &mut spans,
            config.branch && state.session.git.available,
            "branch",
            state.session.git.branch.clone(),
            Style::default().fg(palette().muted),
        );
    }

    push_labeled_badge(
        &mut spans,
        config.context_window,
        "ctx",
        format_context_window_label(state),
        Style::default().fg(context_window_color(state)),
    );

    if config.input_tokens && config.output_tokens {
        push_labeled_badge(
            &mut spans,
            true,
            "tokens",
            format!(
                "in {} · out {}",
                compact_u64(state.session.token_ledger.cumulative_usage.input_tokens),
                compact_u64(state.session.token_ledger.cumulative_usage.output_tokens)
            ),
            Style::default().fg(palette().muted),
        );
    } else {
        push_labeled_badge(
            &mut spans,
            config.input_tokens,
            "in",
            compact_u64(state.session.token_ledger.cumulative_usage.input_tokens),
            Style::default().fg(palette().muted),
        );
        push_labeled_badge(
            &mut spans,
            config.output_tokens,
            "out",
            compact_u64(state.session.token_ledger.cumulative_usage.output_tokens),
            Style::default().fg(palette().muted),
        );
    }

    push_labeled_badge(
        &mut spans,
        config.queue && state.session.queued_commands > 0,
        "queue",
        state.session.queued_commands.to_string(),
        Style::default().fg(palette().warn),
    );
    push_labeled_badge(
        &mut spans,
        config.session,
        "sid",
        state.session.active_session_ref.clone(),
        Style::default().fg(palette().muted),
    );
    push_labeled_badge(
        &mut spans,
        config.clock,
        "clock",
        Local::now().format("%H:%M").to_string(),
        Style::default().fg(palette().muted),
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

pub(super) fn status_color(state: &TuiState) -> Color {
    match state.turn_phase {
        TurnPhase::Idle => palette().assistant,
        TurnPhase::Working => palette().user,
        TurnPhase::WaitingApproval => palette().warn,
        TurnPhase::Failed => palette().error,
    }
}

fn push_labeled_badge(
    spans: &mut Vec<Span<'static>>,
    enabled: bool,
    label: &str,
    value: String,
    value_style: Style,
) {
    if !enabled || value.trim().is_empty() {
        return;
    }
    push_badge(
        spans,
        vec![
            Span::styled(format!("{label} "), Style::default().fg(palette().subtle)),
            Span::styled(value, value_style),
        ],
    );
}

fn push_badge(spans: &mut Vec<Span<'static>>, badge: Vec<Span<'static>>) {
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled("[", Style::default().fg(palette().subtle)));
    spans.extend(badge);
    spans.push(Span::styled("]", Style::default().fg(palette().subtle)));
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
                "{} / {} tok ({}%)",
                compact_usize(window.used_tokens),
                compact_usize(window.max_tokens),
                usage
            )
        }
        Some(window) => format!("{} / 0 tok (--)", compact_usize(window.used_tokens)),
        None => "--".to_string(),
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
