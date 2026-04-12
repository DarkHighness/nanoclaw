use super::theme::palette;
use crate::frontend::tui::state::TuiState;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const FULL_LOGO_LINES: [&str; 6] = [
    "▄▄     ▄▄▄    ▄▄       ▄▄     ▄▄▄    ▄▄▄▄     ▄   ▄▄▄▄   ▄▄▄         ▄▄     ▄▄▄",
    "██▄   ██▀   ▄█▀▀█▄     ██▄   ██▀   ▄█▀▀████▄  ▀██████▀  ▀██▀       ▄█▀▀█▄  █▀██  ██  ██▀▀",
    "███▄  ██    ██  ██     ███▄  ██    ██    ██     ██       ██        ██  ██    ██  ██  ██",
    "██ ▀█▄██    ██▀▀██     ██ ▀█▄██    ██    ██     ██       ██        ██▀▀██    ██  ██  ██",
    "██   ▀██  ▄ ██  ██     ██   ▀██    ██    ██     ██       ██      ▄ ██  ██    ██▄ ██▄ ██",
    "▀██▀    ██  ▀██▀  ▀█▄█ ▀██▀    ██     ▀████▀      ▀█████  ████████ ▀██▀  ▀█▄█  ▀████▀███▀",
];

const COMPACT_LOGO_LINES: [&str; 3] = [
    "███  ██ ▄████▄ ███  ██ ▄████▄ ▄█████ ██     ▄████▄ ██     ██",
    "██ ▀▄██ ██▄▄██ ██ ▀▄██ ██  ██ ██     ██     ██▄▄██ ██ ▄█▄ ██",
    "██   ██ ██  ██ ██   ██ ▀████▀ ▀█████ ██████ ██  ██  ▀██▀██▀",
];

pub(super) fn build_welcome_lines(
    state: &TuiState,
    viewport_width: u16,
    viewport_height: u16,
) -> Vec<Line<'static>> {
    let compact = viewport_height < 20 || viewport_width < 96;
    let mut core = build_welcome_title_lines(compact);
    core.push(Line::raw(""));
    core.push(build_meta_summary_line(state));
    core.push(build_runtime_summary_line(state));
    core.push(Line::raw(""));
    core.push(build_prompt_line(compact));
    core.push(Line::raw(""));
    core.push(build_shortcut_line(compact));

    let top_padding = usize::from(viewport_height.saturating_sub(core.len() as u16) / 2);
    let mut lines = vec![Line::raw(""); top_padding];
    lines.extend(core);
    lines
}

fn build_welcome_title_lines(compact: bool) -> Vec<Line<'static>> {
    let subtitle = if compact {
        "Terminal shell for coding work"
    } else {
        "Terminal shell for focused coding work"
    };
    let mut lines = if compact {
        COMPACT_LOGO_LINES
            .into_iter()
            .map(logo_line)
            .collect::<Vec<_>>()
    } else {
        FULL_LOGO_LINES
            .into_iter()
            .map(logo_line)
            .collect::<Vec<_>>()
    };
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        subtitle,
        Style::default().fg(palette().muted),
    )]));
    lines
}

fn build_prompt_line(compact: bool) -> Line<'static> {
    let detail = if compact {
        "Ask for a change or run /help."
    } else {
        "Ask for a change, inspect the workspace, review history, or run /help."
    };
    Line::from(vec![Span::styled(
        detail,
        Style::default().fg(palette().text),
    )])
}

fn build_shortcut_line(compact: bool) -> Line<'static> {
    let suffix = if compact {
        "Enter run · Tab queue · ↑ history · ^T effort"
    } else {
        "Enter run · Tab queue · ↑ history · ^T effort · ^O editor"
    };
    Line::from(vec![Span::styled(
        suffix,
        Style::default().fg(palette().accent),
    )])
}

fn build_meta_summary_line(state: &TuiState) -> Line<'static> {
    Line::from(vec![
        Span::styled("workspace", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            state.session.workspace_name.clone(),
            Style::default().fg(palette().muted),
        ),
        Span::styled("  ·  ", Style::default().fg(palette().subtle)),
        Span::styled("model", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(model_label(state), Style::default().fg(palette().accent)),
    ])
}

fn build_runtime_summary_line(state: &TuiState) -> Line<'static> {
    let diagnostics = &state.session.startup_diagnostics;
    Line::from(vec![
        Span::styled("tools", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            (diagnostics.local_tool_count + diagnostics.mcp_tool_count).to_string(),
            Style::default().fg(palette().muted),
        ),
        Span::styled("  ·  ", Style::default().fg(palette().subtle)),
        Span::styled("mcp", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            diagnostics.mcp_servers.len().to_string(),
            Style::default().fg(palette().assistant),
        ),
        Span::styled("  ·  ", Style::default().fg(palette().subtle)),
        Span::styled("plugins", Style::default().fg(palette().subtle)),
        Span::styled(" ", Style::default().fg(palette().subtle)),
        Span::styled(
            format!(
                "{}/{}",
                diagnostics.enabled_plugin_count, diagnostics.total_plugin_count
            ),
            Style::default().fg(palette().accent),
        ),
    ])
}

fn model_label(state: &TuiState) -> String {
    match state.session.model_reasoning_effort.as_deref() {
        Some(effort) => format!("{} · {}", state.session.model, effort),
        None => state.session.model.clone(),
    }
}

fn logo_line(text: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.to_string(),
        Style::default()
            .fg(palette().header)
            .add_modifier(Modifier::BOLD),
    )])
}
