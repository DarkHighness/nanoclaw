use super::super::state::{MainPaneMode, ToastTone, TuiState, TurnPhase, preview_text};
use super::theme::palette;
use crate::statusline::StatusLineContextWindowStyle;
use chrono::Local;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin};
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
    if should_render_input_footer(state) {
        let right = format_input_footer_context(state);
        let sections = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(footer_line_width(&right).saturating_add(1)),
            ])
            .split(inner);
        frame.render_widget(
            Paragraph::new(format_input_footer_hint(state))
                .style(Style::default().fg(palette().muted).bg(palette().footer_bg)),
            sections[0],
        );
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(palette().muted).bg(palette().footer_bg)),
            sections[1],
        );
        return;
    }
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
    let status = compact_status_label(state);

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
                    status,
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
        "Model",
        format_model_label(state),
        Style::default().fg(palette().accent),
    );
    push_labeled_badge(
        &mut spans,
        config.cwd,
        "Workspace",
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
            "Git",
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
            "Repo",
            state.session.git.repo_name.clone(),
            Style::default().fg(palette().user),
        );
        push_labeled_badge(
            &mut spans,
            config.branch && state.session.git.available,
            "Branch",
            state.session.git.branch.clone(),
            Style::default().fg(palette().muted),
        );
    }

    if config.context_window {
        match config.context_window_style {
            StatusLineContextWindowStyle::Summary => push_labeled_badge(
                &mut spans,
                true,
                "Context",
                format_context_window_label(state),
                Style::default().fg(context_window_color(state)),
            ),
            StatusLineContextWindowStyle::Meter => push_context_meter_segment(&mut spans, state),
        }
    }

    if config.input_tokens && config.output_tokens {
        push_labeled_badge(
            &mut spans,
            true,
            "Tokens",
            format!(
                "In {} · Out {}",
                compact_input_usage(state.session.token_ledger.cumulative_usage),
                compact_output_usage(state.session.token_ledger.cumulative_usage)
            ),
            Style::default().fg(palette().muted),
        );
    } else {
        push_labeled_badge(
            &mut spans,
            config.input_tokens,
            "Input",
            compact_input_usage(state.session.token_ledger.cumulative_usage),
            Style::default().fg(palette().muted),
        );
        push_labeled_badge(
            &mut spans,
            config.output_tokens,
            "Output",
            compact_output_usage(state.session.token_ledger.cumulative_usage),
            Style::default().fg(palette().muted),
        );
    }

    push_labeled_badge(
        &mut spans,
        config.queue && state.session.queued_commands > 0,
        "Queue",
        state.session.queued_commands.to_string(),
        Style::default().fg(palette().warn),
    );
    push_labeled_badge(
        &mut spans,
        config.session,
        "Session",
        state.session.active_session_ref.clone(),
        Style::default().fg(palette().muted),
    );
    push_labeled_badge(
        &mut spans,
        config.clock,
        "Clock",
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
    let message_color = match toast.tone {
        ToastTone::Error => tone_color,
        _ => palette().text,
    };
    Line::from(vec![
        Span::styled(
            "●",
            Style::default().fg(tone_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("Notice", Style::default().fg(tone_color)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(
            preview_text(&toast.message, 120),
            Style::default().fg(message_color),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusBadge {
    Ready,
    Working,
    Approval,
    Failed,
    Command,
    Rollback,
    Queue,
    Editing,
    Status,
    Thinking,
    Theme,
    Manage,
    Review,
    View,
}

impl StatusBadge {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Working => "Working",
            Self::Approval => "Approval",
            Self::Failed => "Failed",
            Self::Command => "Command",
            Self::Rollback => "Rollback",
            Self::Queue => "Queue",
            Self::Editing => "Editing",
            Self::Status => "Status",
            Self::Thinking => "Thinking",
            Self::Theme => "Theme",
            Self::Manage => "Manage",
            Self::Review => "Review",
            Self::View => "View",
        }
    }
}

fn compact_status_label(state: &TuiState) -> String {
    status_badge(state).label().to_string()
}

fn status_badge(state: &TuiState) -> StatusBadge {
    if state.history_rollback_overlay().is_some() || state.history_rollback_is_primed() {
        return StatusBadge::Rollback;
    }
    if state.pending_control_picker.is_some() {
        return StatusBadge::Queue;
    }
    if state.editing_pending_control.is_some() {
        return StatusBadge::Editing;
    }
    if state.statusline_picker.is_some() {
        return StatusBadge::Status;
    }
    if state.thinking_effort_picker.is_some() {
        return StatusBadge::Thinking;
    }
    if state.theme_picker.is_some() {
        return StatusBadge::Theme;
    }
    if state.managed_toggle_picker.is_some() {
        return StatusBadge::Manage;
    }
    if state.tool_review_overlay().is_some() {
        return StatusBadge::Review;
    }
    if state.main_pane == MainPaneMode::View {
        return StatusBadge::View;
    }
    if !state.input.trim().is_empty() && state.input.starts_with('/') {
        return StatusBadge::Command;
    }
    match state.turn_phase {
        TurnPhase::Working => StatusBadge::Working,
        TurnPhase::WaitingApproval => StatusBadge::Approval,
        TurnPhase::Failed => StatusBadge::Failed,
        TurnPhase::Idle => StatusBadge::Ready,
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

pub(super) fn should_render_input_footer(state: &TuiState) -> bool {
    !state.input.trim().is_empty()
        && !state.input.starts_with('/')
        && state.editing_pending_control.is_none()
        && state.pending_control_picker.is_none()
}

pub(super) fn format_input_footer_hint(state: &TuiState) -> Line<'static> {
    let action = if state.turn_running {
        "Enter to send steer"
    } else {
        "Enter to send"
    };
    Line::from(vec![
        Span::styled("Tab", Style::default().fg(palette().header)),
        Span::styled(" to queue message", Style::default().fg(palette().muted)),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled("Enter", Style::default().fg(palette().accent)),
        Span::styled(format!(" {action}"), Style::default().fg(palette().muted)),
    ])
}

pub(super) fn format_input_footer_context(state: &TuiState) -> Line<'static> {
    let tone = context_window_color(state);
    Line::from(vec![Span::styled(
        format_context_left_label(state),
        Style::default().fg(tone).add_modifier(Modifier::BOLD),
    )])
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

fn format_context_left_label(state: &TuiState) -> String {
    match state.session.token_ledger.context_window {
        Some(window) if window.max_tokens > 0 => {
            let used = window.used_tokens.min(window.max_tokens);
            let left = 100_u64
                .saturating_sub((used as u64).saturating_mul(100) / window.max_tokens as u64);
            format!("{left}% Context left")
        }
        Some(_) | None => "Context left --".to_string(),
    }
}

fn push_context_meter_segment(spans: &mut Vec<Span<'static>>, state: &TuiState) {
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    let tone = context_window_color(state);
    spans.push(Span::styled(
        "Context ",
        Style::default().fg(palette().subtle),
    ));
    spans.push(Span::styled("[", Style::default().fg(palette().subtle)));
    spans.push(Span::styled(
        context_window_meter(state),
        Style::default().fg(tone),
    ));
    spans.push(Span::styled("]", Style::default().fg(palette().subtle)));
}

fn context_window_meter(state: &TuiState) -> String {
    const WIDTH: usize = 5;
    const PARTIAL_BLOCKS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];

    let Some(window) = state.session.token_ledger.context_window else {
        return " ".repeat(WIDTH);
    };
    if window.max_tokens == 0 {
        return " ".repeat(WIDTH);
    }

    let used = window.used_tokens.min(window.max_tokens);
    let units = used.saturating_mul(WIDTH).saturating_mul(8) / window.max_tokens;
    let full = (units / 8).min(WIDTH);
    let partial = units % 8;

    let mut meter = String::new();
    meter.push_str(&"█".repeat(full));
    if partial > 0 && full < WIDTH {
        meter.push(PARTIAL_BLOCKS[partial - 1]);
    }
    let visible = full + usize::from(partial > 0 && full < WIDTH);
    meter.push_str(&" ".repeat(WIDTH.saturating_sub(visible)));
    meter
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

fn footer_line_width(line: &Line<'_>) -> u16 {
    line.spans
        .iter()
        .map(|span| super::shared::composer_cursor_width(span.content.as_ref()))
        .sum()
}

fn compact_usize(value: usize) -> String {
    compact_u64(value as u64)
}

fn compact_input_usage(usage: agent::types::TokenUsage) -> String {
    let mut rendered = compact_u64(usage.uncached_input_tokens());
    if usage.cache_read_tokens > 0 {
        rendered.push('+');
        rendered.push_str(&compact_u64(usage.cache_read_tokens));
        rendered.push('c');
    }
    rendered
}

fn compact_output_usage(usage: agent::types::TokenUsage) -> String {
    let mut rendered = compact_u64(usage.output_tokens);
    if usage.reasoning_tokens > 0 {
        rendered.push('+');
        rendered.push_str(&compact_u64(usage.reasoning_tokens));
        rendered.push('r');
    }
    rendered
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
