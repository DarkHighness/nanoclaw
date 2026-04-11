use crate::backend::SandboxFallbackNotice;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
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
}

struct StartupPromptLayout {
    popup: Rect,
    summary: Rect,
    risk_card: Rect,
    policy_card: Rect,
    reason_card: Rect,
    fix_card: Rect,
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
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn enter_prompt_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Preserve native terminal selection here as well. The startup prompt has
    // complete keyboard coverage, so click-only affordances are not worth
    // globally hijacking the terminal mouse protocol before the session starts.
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error.into());
    }
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let mut cleanup_stdout = io::stdout();
            let _ = execute!(cleanup_stdout, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            Err(error.into())
        }
    }
}

fn leave_prompt_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let raw_result = disable_raw_mode();
    let screen_result = execute!(terminal.backend_mut(), LeaveAlternateScreen);
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
        Block::default().style(Style::default().bg(FOOTER_BG)),
        layout.popup,
    );

    frame.render_widget(
        Paragraph::new(build_summary_text())
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        layout.summary,
    );
    render_section_card(
        frame,
        layout.risk_card,
        build_risk_card_text(),
        Style::default().bg(BOTTOM_PANE_BG),
    );
    render_section_card(
        frame,
        layout.policy_card,
        build_policy_card_text(notice),
        Style::default().bg(BOTTOM_PANE_BG),
    );
    render_section_card(
        frame,
        layout.reason_card,
        build_reason_card_text(notice),
        Style::default().bg(BOTTOM_PANE_BG),
    );
    render_section_card(
        frame,
        layout.fix_card,
        build_fix_card_text(notice),
        Style::default().bg(BOTTOM_PANE_BG),
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
        "Esc",
        "Abort Startup",
        state.selection == StartupPromptSelection::Abort,
        ERROR,
    );
    render_action_button(
        frame,
        layout.continue_button,
        "Y",
        "Continue Without Sandbox",
        state.selection == StartupPromptSelection::Continue,
        ACCENT,
    );
    frame.render_widget(
        Paragraph::new(build_footer_text())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED).bg(FOOTER_BG)),
        layout.footer,
    );
}

fn render_section_card(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    text: Text<'static>,
    style: Style,
) {
    frame.render_widget(Block::default().style(style), area);
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(style.fg(TEXT)),
        inner,
    );
}

fn render_action_button(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    shortcut: &'static str,
    label: &'static str,
    selected: bool,
    accent: Color,
) {
    frame.render_widget(
        Paragraph::new(build_action_button_line(shortcut, label, selected, accent))
            .alignment(Alignment::Center)
            .style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
}

fn build_action_button_line(
    shortcut: &'static str,
    label: &'static str,
    selected: bool,
    accent: Color,
) -> Line<'static> {
    let marker = if selected { "› " } else { "  " };
    let shortcut_style = if selected {
        Style::default()
            .fg(BG)
            .bg(accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    };
    let label_style = if selected {
        Style::default().fg(HEADER).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    Line::from(vec![
        Span::styled(
            marker,
            Style::default().fg(if selected { accent } else { SUBTLE }),
        ),
        Span::styled(format!(" {shortcut} "), shortcut_style),
        Span::styled(" ", Style::default().fg(SUBTLE)),
        Span::styled(label, label_style),
    ])
}

fn build_summary_text() -> Text<'static> {
    Text::from(vec![
        Line::from(vec![
            Span::styled(
                "startup safety check",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                "sandbox enforcement unavailable",
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "HIGH RISK",
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                "continuing will disable sandbox enforcement for this run.",
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::styled(
            "Review the degraded surfaces and host-side fixes below before you continue.",
            Style::default().fg(MUTED),
        )]),
    ])
}

fn build_risk_card_text() -> Text<'static> {
    Text::from(vec![
        build_section_label("What Changes Now", ERROR),
        Line::raw(""),
        Line::from(vec![Span::styled(
            "Sandbox enforcement is disabled for this run.",
            Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
        )]),
        Line::raw(""),
        bullet_line("shell access stays degraded"),
        bullet_line("command hooks start degraded until host-process surfaces return"),
        bullet_line("stdio MCP starts degraded until host-process surfaces return"),
        bullet_line("managed code-intel helpers start degraded until host-process surfaces return"),
    ])
}

fn build_policy_card_text(notice: &SandboxFallbackNotice) -> Text<'static> {
    Text::from(vec![
        build_section_label("Current Policy", USER),
        Line::raw(""),
        detail_line(&notice.policy_summary),
    ])
}

fn build_reason_card_text(notice: &SandboxFallbackNotice) -> Text<'static> {
    Text::from(vec![
        build_section_label("Why This Failed", ACCENT),
        Line::raw(""),
        detail_line(&notice.reason),
    ])
}

fn build_fix_card_text(notice: &SandboxFallbackNotice) -> Text<'static> {
    let mut lines = vec![build_section_label("Fix On Host", WARN), Line::raw("")];
    lines.extend(notice.setup_steps.iter().enumerate().map(|(index, step)| {
        Line::from(vec![
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
            "Choose one option to continue startup.",
            Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
        ),
    ])])
}

fn build_footer_text() -> Text<'static> {
    Text::from(vec![Line::from(vec![
        Span::styled("safe default", Style::default().fg(MUTED)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("abort startup", Style::default().fg(ERROR)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled("Tab", Style::default().fg(ACCENT)),
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
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )])
}

fn bullet_line(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("• ", Style::default().fg(SUBTLE)),
        Span::styled(text.to_string(), Style::default().fg(TEXT)),
    ])
}

fn detail_line(text: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.to_string(),
        Style::default().fg(TEXT),
    )])
}

fn startup_prompt_layout(area: Rect) -> StartupPromptLayout {
    let popup = centered_rect(area, 84, 78);
    let stack_buttons = popup.width < 92;
    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(14),
            Constraint::Length(if stack_buttons { 5 } else { 2 }),
            Constraint::Length(1),
        ])
        .split(inner);
    let body_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(44),
            Constraint::Length(1),
            Constraint::Percentage(56),
        ])
        .split(sections[1]);
    let top_cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(49),
            Constraint::Percentage(2),
            Constraint::Percentage(49),
        ])
        .split(body_rows[0]);
    let bottom_cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(49),
            Constraint::Percentage(2),
            Constraint::Percentage(49),
        ])
        .split(body_rows[2]);
    let action_inner = sections[2].inner(Margin {
        vertical: 0,
        horizontal: 0,
    });
    let action_sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if stack_buttons {
            vec![
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
        } else {
            vec![Constraint::Length(1), Constraint::Length(1)]
        })
        .split(action_inner);
    let (abort_button, continue_button) = if stack_buttons {
        (action_sections[2], action_sections[4])
    } else {
        let buttons = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(38),
                Constraint::Percentage(4),
                Constraint::Percentage(58),
            ])
            .split(action_sections[1]);
        (buttons[0], buttons[2])
    };
    StartupPromptLayout {
        popup,
        summary: sections[0],
        risk_card: top_cards[0],
        policy_card: top_cards[2],
        reason_card: bottom_cards[0],
        fix_card: bottom_cards[2],
        action_area: sections[2],
        action_prompt: action_sections[0],
        abort_button,
        continue_button,
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

#[cfg(test)]
mod tests {
    use super::{
        ACCENT, BG, HEADER, StartupPromptSelection, StartupPromptState, TEXT,
        build_action_button_line, build_fix_card_text, build_footer_text, build_policy_card_text,
        build_reason_card_text, build_risk_card_text, build_summary_text, startup_prompt_layout,
    };
    use crate::backend::SandboxFallbackNotice;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;
    use ratatui::text::Text;

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
    fn startup_prompt_stacks_buttons_on_narrow_viewports() {
        let layout = startup_prompt_layout(Rect::new(0, 0, 80, 40));
        assert!(layout.continue_button.y > layout.abort_button.y);
        assert_eq!(layout.continue_button.x, layout.abort_button.x);
        assert_eq!(layout.abort_button.height, 1);
        assert_eq!(layout.continue_button.height, 1);
    }

    #[test]
    fn startup_prompt_keeps_wide_buttons_single_row() {
        let layout = startup_prompt_layout(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.abort_button.height, 1);
        assert_eq!(layout.continue_button.height, 1);
    }

    #[test]
    fn startup_prompt_selected_button_uses_high_contrast_label() {
        let line = build_action_button_line("Y", "Continue Without Sandbox", true, ACCENT);
        assert_eq!(line.spans[1].style.bg, Some(ACCENT));
        assert_eq!(line.spans[1].style.fg, Some(BG));
        assert_eq!(line.spans[3].style.fg, Some(HEADER));
    }

    #[test]
    fn startup_prompt_unselected_button_keeps_badge_background_clear() {
        let line = build_action_button_line("Esc", "Abort Startup", false, ACCENT);
        assert_eq!(line.spans[1].style.bg, None);
        assert_eq!(line.spans[1].style.fg, Some(ACCENT));
        assert_eq!(line.spans[3].style.fg, Some(TEXT));
    }

    #[test]
    fn startup_prompt_cards_surface_policy_reason_and_host_fixes() {
        let notice = SandboxFallbackNotice {
            policy_summary: "workspace-write, network off".to_string(),
            reason: "uid map denied".to_string(),
            risk_summary: "sandbox disabled".to_string(),
            setup_steps: vec![
                "Install bubblewrap".to_string(),
                "Enable user namespaces".to_string(),
            ],
        };

        let policy = flatten_text(build_policy_card_text(&notice));
        let reason = flatten_text(build_reason_card_text(&notice));
        let fix = flatten_text(build_fix_card_text(&notice));
        let risk = flatten_text(build_risk_card_text());

        assert!(policy.iter().any(|line| line == "Current Policy"));
        assert!(policy.iter().any(|line| line.contains("workspace-write")));
        assert!(reason.iter().any(|line| line == "Why This Failed"));
        assert!(reason.iter().any(|line| line.contains("uid map denied")));
        assert!(fix.iter().any(|line| line == "Fix On Host"));
        assert!(
            fix.iter()
                .any(|line| line.contains("1. Install bubblewrap"))
        );
        assert!(
            fix.iter()
                .any(|line| line.contains("2. Enable user namespaces"))
        );
        assert!(risk.iter().any(|line| line == "What Changes Now"));
        assert!(
            risk.iter()
                .any(|line| line.contains("Sandbox enforcement is disabled"))
        );
    }

    #[test]
    fn startup_prompt_summary_marks_the_risk_as_severe() {
        let lines = flatten_text(build_summary_text());
        assert!(lines.iter().any(|line| line.contains("HIGH RISK")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("disable sandbox enforcement"))
        );
    }

    #[test]
    fn startup_prompt_footer_stays_short_and_operator_facing() {
        let line = flatten_text(build_footer_text()).join(" ");
        assert!(line.contains("safe default"));
        assert!(line.contains("abort startup"));
        assert!(line.contains("Tab switch"));
        assert!(line.contains("Enter confirm"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn flatten_text(text: Text<'static>) -> Vec<String> {
        text.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    }
}
