use crate::backend::SandboxFallbackNotice;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::io::{self, Stdout};

const BG: Color = Color::Rgb(15, 17, 20);
const FOOTER_BG: Color = Color::Rgb(20, 22, 26);
const BOTTOM_PANE_BG: Color = Color::Rgb(24, 27, 31);
const TEXT: Color = Color::Rgb(235, 236, 232);
const MUTED: Color = Color::Rgb(154, 158, 151);
const SUBTLE: Color = Color::Rgb(98, 103, 108);
const ACCENT: Color = Color::Rgb(108, 189, 182);
const USER: Color = Color::Rgb(221, 188, 128);
const HEADER: Color = Color::Rgb(244, 244, 239);
const WARN: Color = Color::Rgb(223, 179, 88);
const ERROR: Color = Color::Rgb(227, 125, 118);
const BORDER_ACTIVE: Color = Color::Rgb(165, 168, 160);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupPromptSelection {
    Abort,
    Continue,
}

impl StartupPromptSelection {
    fn toggle(self) -> Self {
        match self {
            Self::Abort => Self::Continue,
            Self::Continue => Self::Abort,
        }
    }

    fn decision(self) -> bool {
        matches!(self, Self::Continue)
    }
}

#[derive(Clone, Debug)]
struct StartupPromptState {
    selection: StartupPromptSelection,
}

impl Default for StartupPromptState {
    fn default() -> Self {
        Self {
            // Preserve the previous `[y/N]` behavior by defaulting to abort
            // until the operator makes an explicit choice.
            selection: StartupPromptSelection::Abort,
        }
    }
}

impl StartupPromptState {
    fn handle_key(&mut self, key: KeyEvent) -> Option<bool> {
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.selection = self.selection.toggle();
                None
            }
            KeyCode::Enter => Some(self.selection.decision()),
            KeyCode::Esc => Some(false),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(false),
            KeyCode::Char(ch) => match ch.to_ascii_lowercase() {
                'y' => Some(true),
                'n' | 'q' => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    fn handle_mouse(&self, mouse: MouseEvent, area: Rect) -> Option<bool> {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }
        let layout = startup_prompt_layout(area);
        if rect_contains(layout.abort_button, mouse.column, mouse.row) {
            return Some(false);
        }
        if rect_contains(layout.continue_button, mouse.column, mouse.row) {
            return Some(true);
        }
        None
    }
}

struct StartupPromptLayout {
    popup: Rect,
    summary: Rect,
    body: Rect,
    action_area: Rect,
    action_prompt: Rect,
    abort_button: Rect,
    continue_button: Rect,
    footer: Rect,
}

// This prompt runs before the main session exists because the operator's
// choice changes whether session construction is allowed to fail open.
pub(crate) fn confirm_unsandboxed_startup_screen(notice: &SandboxFallbackNotice) -> Result<bool> {
    let mut terminal = enter_prompt_terminal()?;
    let result = run_prompt_loop(&mut terminal, notice);
    let cleanup = leave_prompt_terminal(&mut terminal);
    match (result, cleanup) {
        (Ok(answer), Ok(())) => Ok(answer),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(_cleanup_error)) => Err(error),
    }
}

fn run_prompt_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    notice: &SandboxFallbackNotice,
) -> Result<bool> {
    let mut state = StartupPromptState::default();
    loop {
        terminal.draw(|frame| render_prompt(frame, notice, &state))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(decision) = state.handle_key(key) {
                    return Ok(decision);
                }
            }
            Event::Mouse(mouse) => {
                let size = terminal.size()?;
                let area = Rect::new(0, 0, size.width, size.height);
                if let Some(decision) = state.handle_mouse(mouse, area) {
                    return Ok(decision);
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn enter_prompt_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        return Err(error.into());
    }
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let mut cleanup_stdout = io::stdout();
            let _ = execute!(cleanup_stdout, LeaveAlternateScreen, DisableMouseCapture);
            let _ = disable_raw_mode();
            Err(error.into())
        }
    }
}

fn leave_prompt_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let raw_result = disable_raw_mode();
    let screen_result = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let cursor_result = terminal.show_cursor();
    raw_result?;
    screen_result?;
    cursor_result?;
    Ok(())
}

fn render_prompt(
    frame: &mut ratatui::Frame<'_>,
    notice: &SandboxFallbackNotice,
    state: &StartupPromptState,
) {
    let area = frame.area();
    let layout = startup_prompt_layout(area);
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    frame.render_widget(Clear, layout.popup);
    frame.render_widget(
        Block::default()
            .title(" Startup Safety Check ")
            .title_style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_ACTIVE))
            .style(Style::default().bg(FOOTER_BG)),
        layout.popup,
    );

    frame.render_widget(
        Paragraph::new(build_summary_text())
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        layout.summary,
    );
    frame.render_widget(
        Paragraph::new(build_notice_text(notice))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        layout.body,
    );
    frame.render_widget(
        Block::default().style(Style::default().bg(BOTTOM_PANE_BG)),
        layout.action_area,
    );
    frame.render_widget(
        Paragraph::new(build_prompt_text())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        layout.action_prompt,
    );
    render_action_button(
        frame,
        layout.abort_button,
        "Abort Startup",
        state.selection == StartupPromptSelection::Abort,
        ERROR,
    );
    render_action_button(
        frame,
        layout.continue_button,
        "Continue Without Sandbox",
        state.selection == StartupPromptSelection::Continue,
        ACCENT,
    );
    frame.render_widget(
        Paragraph::new(build_footer_text(state))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED).bg(FOOTER_BG)),
        layout.footer,
    );
}

fn render_action_button(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    label: &'static str,
    selected: bool,
    accent: Color,
) {
    let border_style = if selected {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(SUBTLE)
    };
    let text_style = if selected {
        Style::default().fg(HEADER).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    frame.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .style(Style::default().bg(BOTTOM_PANE_BG)),
            )
            .style(text_style.bg(BOTTOM_PANE_BG)),
        area,
    );
}

fn build_summary_text() -> Text<'static> {
    Text::from(vec![
        Line::from(vec![
            Span::styled(
                "warning",
                Style::default().fg(WARN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                "sandbox backend unavailable for the configured runtime policy",
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::styled(
            "The current host can start the session, but it cannot enforce the requested sandbox backend.",
            Style::default().fg(MUTED),
        )]),
    ])
}

fn build_notice_text(notice: &SandboxFallbackNotice) -> Text<'static> {
    let mut lines = Vec::new();
    lines.extend(build_detail_section("policy", &notice.policy_summary, USER));
    lines.push(Line::raw(""));
    lines.extend(build_detail_section("reason", &notice.reason, ACCENT));
    lines.push(Line::raw(""));
    lines.extend(build_detail_section("risk", &notice.risk_summary, WARN));
    lines.push(Line::raw(""));
    lines.push(build_section_label("setup", ACCENT));
    lines.extend(notice.setup_steps.iter().enumerate().map(|(index, step)| {
        Line::from(vec![
            Span::styled("  └ ", Style::default().fg(SUBTLE)),
            Span::styled(
                format!("{}. ", index + 1),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(step.clone(), Style::default().fg(TEXT)),
        ])
    }));
    Text::from(lines)
}

fn build_prompt_text() -> Text<'static> {
    Text::from(vec![Line::from(vec![
        Span::styled(
            "startup",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            "Continue without sandbox enforcement for this run?",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
    ])])
}

fn build_footer_text(state: &StartupPromptState) -> Text<'static> {
    let (selection, color) = match state.selection {
        StartupPromptSelection::Abort => ("abort startup", ERROR),
        StartupPromptSelection::Continue => ("continue without sandbox", ACCENT),
    };
    Text::from(vec![Line::from(vec![
        Span::styled("selected", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            selection,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("←/→ or Tab", Style::default().fg(ACCENT)),
        Span::styled(" switch", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("Enter", Style::default().fg(HEADER)),
        Span::styled(" confirm", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("y", Style::default().fg(ACCENT)),
        Span::styled(" continue", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("esc", Style::default().fg(ERROR)),
        Span::styled(" abort", Style::default().fg(MUTED)),
    ])])
}

fn build_section_label(label: &'static str, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("• {label}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )])
}

fn build_detail_section(label: &'static str, value: &str, color: Color) -> Vec<Line<'static>> {
    vec![
        build_section_label(label, color),
        Line::from(vec![
            Span::styled("  └ ", Style::default().fg(SUBTLE)),
            Span::styled(value.to_string(), Style::default().fg(TEXT)),
        ]),
    ]
}

fn startup_prompt_layout(area: Rect) -> StartupPromptLayout {
    let popup = centered_rect(area, 80, 74);
    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(inner);
    let action_inner = sections[2].inner(Margin {
        vertical: 0,
        horizontal: 1,
    });
    let action_sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(3)])
        .split(action_inner);
    let buttons = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(48),
            Constraint::Percentage(4),
            Constraint::Percentage(48),
        ])
        .split(action_sections[1]);
    StartupPromptLayout {
        popup,
        summary: sections[0],
        body: sections[1],
        action_area: sections[2],
        action_prompt: action_sections[0],
        abort_button: buttons[0],
        continue_button: buttons[2],
        footer: sections[3],
    }
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(height_percent)) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100_u16.saturating_sub(height_percent)) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(width_percent)) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100_u16.saturating_sub(width_percent)) / 2),
        ])
        .split(vertical[1])[1]
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
mod tests {
    use super::{
        StartupPromptSelection, StartupPromptState, build_footer_text, build_notice_text,
        startup_prompt_layout,
    };
    use crate::backend::SandboxFallbackNotice;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;

    #[test]
    fn startup_prompt_defaults_to_abort() {
        let state = StartupPromptState::default();
        assert_eq!(state.selection, StartupPromptSelection::Abort);
    }

    #[test]
    fn startup_prompt_toggles_selection_before_confirming() {
        let mut state = StartupPromptState::default();

        assert_eq!(state.handle_key(key(KeyCode::Right)), None);
        assert_eq!(state.selection, StartupPromptSelection::Continue);
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Some(true));

        let mut state = StartupPromptState::default();
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Some(false));
    }

    #[test]
    fn startup_prompt_shortcuts_match_previous_yes_no_flow() {
        let mut state = StartupPromptState::default();

        assert_eq!(state.handle_key(key(KeyCode::Char('y'))), Some(true));
        assert_eq!(state.handle_key(key(KeyCode::Char('n'))), Some(false));
        assert_eq!(state.handle_key(key(KeyCode::Esc)), Some(false));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(false)
        );
    }

    #[test]
    fn startup_prompt_mouse_clicks_activate_buttons() {
        let state = StartupPromptState::default();
        let layout = startup_prompt_layout(Rect::new(0, 0, 120, 40));

        assert_eq!(
            state.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: layout.abort_button.x + 1,
                    row: layout.abort_button.y + 1,
                    modifiers: KeyModifiers::NONE,
                },
                Rect::new(0, 0, 120, 40),
            ),
            Some(false)
        );
        assert_eq!(
            state.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: layout.continue_button.x + 1,
                    row: layout.continue_button.y + 1,
                    modifiers: KeyModifiers::NONE,
                },
                Rect::new(0, 0, 120, 40),
            ),
            Some(true)
        );
    }

    #[test]
    fn startup_prompt_notice_text_lists_policy_reason_risk_and_setup() {
        let notice = SandboxFallbackNotice {
            policy_summary: "workspace-write, network off".to_string(),
            reason: "uid map denied".to_string(),
            risk_summary: "sandbox disabled".to_string(),
            setup_steps: vec![
                "Install bubblewrap".to_string(),
                "Enable user namespaces".to_string(),
            ],
        };

        let text = build_notice_text(&notice);
        let lines = text
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(lines.iter().any(|line| line == "• policy"));
        assert!(lines.iter().any(|line| line.contains("└ workspace-write")));
        assert!(lines.iter().any(|line| line == "• reason"));
        assert!(lines.iter().any(|line| line.contains("uid map denied")));
        assert!(lines.iter().any(|line| line == "• risk"));
        assert!(lines.iter().any(|line| line.contains("sandbox disabled")));
        assert!(lines.iter().any(|line| line == "• setup"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("└ 1. Install bubblewrap"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("└ 2. Enable user namespaces"))
        );
    }

    #[test]
    fn startup_prompt_footer_tracks_the_selected_action() {
        let mut state = StartupPromptState::default();
        let default_line = footer_line(&state);
        assert!(default_line.contains("abort startup"));

        state.selection = StartupPromptSelection::Continue;
        let continue_line = footer_line(&state);
        assert!(continue_line.contains("continue without sandbox"));
        assert!(continue_line.contains("Enter confirm"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn footer_line(state: &StartupPromptState) -> String {
        build_footer_text(state)
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>()
    }
}
