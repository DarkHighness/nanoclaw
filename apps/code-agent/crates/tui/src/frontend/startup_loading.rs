use crate::backend::{
    BootProgressItem, BootProgressItemKind, BootProgressStage, BootProgressStatus,
    BootProgressUpdate,
};
use anyhow::Result;
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

const BG: Color = Color::Rgb(11, 14, 18);
const FOOTER_BG: Color = Color::Rgb(19, 25, 32);
const BOTTOM_PANE_BG: Color = Color::Rgb(24, 32, 41);
const TEXT: Color = Color::Rgb(237, 244, 247);
const MUTED: Color = Color::Rgb(177, 188, 196);
const SUBTLE: Color = Color::Rgb(108, 120, 130);
const ACCENT: Color = Color::Rgb(110, 223, 211);
const USER: Color = Color::Rgb(228, 190, 115);
const ASSISTANT: Color = Color::Rgb(143, 221, 178);
const WARN: Color = Color::Rgb(240, 197, 99);
const HEADER: Color = Color::Rgb(255, 255, 255);

const ITEM_PREVIEW_LIMIT: usize = 6;

pub struct StartupLoadingScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: StartupLoadingState,
}

impl StartupLoadingScreen {
    pub fn enter(
        workspace_name: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<String>,
    ) -> Result<Self> {
        let mut terminal = enter_loading_terminal()?;
        let state = StartupLoadingState::new(workspace_name, model, reasoning_effort);
        terminal.draw(|frame| render_loading(frame, &state))?;
        Ok(Self { terminal, state })
    }

    pub fn apply(&mut self, update: BootProgressUpdate) -> Result<()> {
        self.state.apply(update);
        self.terminal
            .draw(|frame| render_loading(frame, &self.state))?;
        Ok(())
    }

    pub fn leave(mut self) -> Result<()> {
        leave_loading_terminal(&mut self.terminal)
    }
}

#[derive(Clone, Debug)]
struct StartupLoadingState {
    workspace_name: String,
    model_label: String,
    current_stage: BootProgressStage,
    completed: [bool; BootProgressStage::ALL.len()],
    stage_items: [Vec<BootProgressItem>; BootProgressStage::ALL.len()],
    stage_notes: [Option<String>; BootProgressStage::ALL.len()],
    recent_notes: Vec<String>,
}

impl StartupLoadingState {
    fn new(
        workspace_name: impl Into<String>,
        model: impl Into<String>,
        reasoning_effort: Option<String>,
    ) -> Self {
        let model = model.into();
        let model_label = reasoning_effort
            .filter(|effort| !effort.is_empty())
            .map(|effort| format!("{model} · {effort}"))
            .unwrap_or(model);
        Self {
            workspace_name: workspace_name.into(),
            model_label,
            current_stage: BootProgressStage::Store,
            completed: [false; BootProgressStage::ALL.len()],
            stage_items: std::array::from_fn(|_| Vec::new()),
            stage_notes: std::array::from_fn(|_| None),
            recent_notes: vec!["Preparing session boot".to_string()],
        }
    }

    fn apply(&mut self, update: BootProgressUpdate) {
        let index = update.stage.position();
        self.current_stage = update.stage;
        self.stage_items[index] = update.items;
        if let Some(note) = update.note {
            self.stage_notes[index] = Some(note.clone());
            self.push_recent_note(format!("{} · {note}", update.stage.label()));
        }
        self.completed[index] = matches!(update.status, BootProgressStatus::Completed);
    }

    fn completed_count(&self) -> usize {
        self.completed.iter().filter(|status| **status).count()
    }

    fn visible_stage_items(&self) -> &[BootProgressItem] {
        let current = &self.stage_items[self.current_stage.position()];
        if !current.is_empty() {
            return current;
        }
        BootProgressStage::ALL
            .iter()
            .rev()
            .find_map(|stage| {
                let items = &self.stage_items[stage.position()];
                (!items.is_empty()).then_some(items.as_slice())
            })
            .unwrap_or(&[])
    }

    fn current_note(&self) -> Option<&str> {
        self.stage_notes[self.current_stage.position()].as_deref()
    }

    fn push_recent_note(&mut self, note: String) {
        if self.recent_notes.last() == Some(&note) {
            return;
        }
        self.recent_notes.push(note);
        if self.recent_notes.len() > 5 {
            let overflow = self.recent_notes.len() - 5;
            self.recent_notes.drain(0..overflow);
        }
    }
}

fn enter_loading_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
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

fn leave_loading_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let raw_result = disable_raw_mode();
    let screen_result = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor_result = terminal.show_cursor();
    raw_result?;
    screen_result?;
    cursor_result?;
    Ok(())
}

fn render_loading(frame: &mut ratatui::Frame<'_>, state: &StartupLoadingState) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let popup = centered_rect(area, 82, 78);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(FOOTER_BG)),
        popup,
    );
    let inner = popup.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(5),
            Constraint::Min(3),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(build_summary_text(state))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(FOOTER_BG)),
        rows[0],
    );
    render_card(frame, rows[1], build_stage_text(state));
    render_card(frame, rows[2], build_item_text(state));
    render_card(frame, rows[3], build_note_text(state));
    frame.render_widget(
        Paragraph::new(build_footer_text())
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(MUTED).bg(FOOTER_BG)),
        rows[4],
    );
}

fn render_card(frame: &mut ratatui::Frame<'_>, area: Rect, text: Text<'static>) {
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(SUBTLE))
            .style(Style::default().bg(BOTTOM_PANE_BG)),
        area,
    );
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(TEXT).bg(BOTTOM_PANE_BG)),
        area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );
}

fn build_summary_text(state: &StartupLoadingState) -> Text<'static> {
    let completed = state.completed_count();
    let total = BootProgressStage::ALL.len();
    Text::from(vec![
        Line::from(vec![
            Span::styled(
                "Loading Session",
                Style::default().fg(HEADER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(SUBTLE)),
            Span::styled(
                format!("{}/{} stages", completed, total),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("workspace", Style::default().fg(SUBTLE)),
            Span::raw(" "),
            Span::styled(state.workspace_name.clone(), Style::default().fg(MUTED)),
            Span::styled("  ·  ", Style::default().fg(SUBTLE)),
            Span::styled("model", Style::default().fg(SUBTLE)),
            Span::raw(" "),
            Span::styled(state.model_label.clone(), Style::default().fg(USER)),
        ]),
        Line::from(vec![Span::styled(
            "Skills, MCP, tooling, and persisted session surfaces are being prepared before the welcome screen.",
            Style::default().fg(MUTED),
        )]),
    ])
}

fn build_stage_text(state: &StartupLoadingState) -> Text<'static> {
    let mut lines = vec![section_line("Progress", ACCENT), Line::raw("")];
    for stage in BootProgressStage::ALL {
        let index = stage.position();
        let (marker, marker_style, label_style) = if state.completed[index] {
            (
                "●",
                Style::default().fg(ASSISTANT).add_modifier(Modifier::BOLD),
                Style::default().fg(MUTED),
            )
        } else if stage == state.current_stage {
            (
                "◉",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                "·",
                Style::default().fg(SUBTLE),
                Style::default().fg(SUBTLE),
            )
        };
        lines.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled(stage.label().to_string(), label_style),
            Span::styled("  ", Style::default().fg(SUBTLE)),
            Span::styled(
                stage_status_label(stage, state),
                Style::default().fg(stage_status_color(stage, state)),
            ),
        ]));
    }
    Text::from(lines)
}

fn build_item_text(state: &StartupLoadingState) -> Text<'static> {
    let items = state.visible_stage_items();
    let mut lines = vec![section_line(
        format!("{} Items", state.current_stage.label()),
        USER,
    )];
    if let Some(note) = state.current_note() {
        lines.push(Line::from(vec![Span::styled(
            note.to_string(),
            Style::default().fg(MUTED),
        )]));
    }
    lines.push(Line::raw(""));
    if items.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "Waiting for concrete items from the current boot stage.",
            Style::default().fg(SUBTLE),
        )]));
        return Text::from(lines);
    }
    for item in items.iter().take(ITEM_PREVIEW_LIMIT) {
        lines.push(Line::from(vec![
            Span::styled(
                "• ",
                Style::default()
                    .fg(item_kind_color(item.kind))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                item_kind_label(item.kind),
                Style::default().fg(SUBTLE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(SUBTLE)),
            Span::styled(item.label.clone(), Style::default().fg(TEXT)),
        ]));
    }
    if items.len() > ITEM_PREVIEW_LIMIT {
        lines.push(Line::from(vec![Span::styled(
            format!("+ {} more item(s)", items.len() - ITEM_PREVIEW_LIMIT),
            Style::default().fg(SUBTLE),
        )]));
    }
    Text::from(lines)
}

fn build_note_text(state: &StartupLoadingState) -> Text<'static> {
    let mut lines = vec![section_line("Recent Notes", WARN), Line::raw("")];
    for note in &state.recent_notes {
        lines.push(Line::from(vec![
            Span::styled("• ", Style::default().fg(WARN)),
            Span::styled(note.clone(), Style::default().fg(MUTED)),
        ]));
    }
    Text::from(lines)
}

fn build_footer_text() -> Text<'static> {
    Text::from(vec![Line::from(vec![
        Span::styled("startup", Style::default().fg(SUBTLE)),
        Span::styled(" · ", Style::default().fg(SUBTLE)),
        Span::styled(
            "The session will enter the normal welcome page as soon as finalization completes.",
            Style::default().fg(MUTED),
        ),
    ])])
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

fn section_line(label: impl Into<String>, color: Color) -> Line<'static> {
    Line::from(vec![Span::styled(
        label.into(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )])
}

fn stage_status_label(stage: BootProgressStage, state: &StartupLoadingState) -> &'static str {
    let index = stage.position();
    if state.completed[index] {
        "ready"
    } else if stage == state.current_stage {
        "loading"
    } else {
        "pending"
    }
}

fn stage_status_color(stage: BootProgressStage, state: &StartupLoadingState) -> Color {
    let index = stage.position();
    if state.completed[index] {
        ASSISTANT
    } else if stage == state.current_stage {
        ACCENT
    } else {
        SUBTLE
    }
}

fn item_kind_label(kind: BootProgressItemKind) -> &'static str {
    match kind {
        BootProgressItemKind::Store => "Store",
        BootProgressItemKind::Plugin => "Plugin",
        BootProgressItemKind::SkillRoot => "Root",
        BootProgressItemKind::Skill => "Skill",
        BootProgressItemKind::McpServer => "MCP",
        BootProgressItemKind::ToolSurface => "Surface",
    }
}

fn item_kind_color(kind: BootProgressItemKind) -> Color {
    match kind {
        BootProgressItemKind::Store => USER,
        BootProgressItemKind::Plugin => WARN,
        BootProgressItemKind::SkillRoot | BootProgressItemKind::Skill => ACCENT,
        BootProgressItemKind::McpServer => ASSISTANT,
        BootProgressItemKind::ToolSurface => HEADER,
    }
}
