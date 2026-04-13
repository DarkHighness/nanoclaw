use crate::backend::{
    BootProgressItem, BootProgressItemKind, BootProgressStage, BootProgressStatus,
    BootProgressUpdate,
};
use crate::theme::{ThemePalette, active_palette};
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
    let theme = active_palette();
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.canvas_surface())),
        area,
    );
    let popup = centered_rect(area, 82, 78);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.overlay_surface())),
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
        Paragraph::new(build_summary_text(state, theme))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.overlay_surface())),
        rows[0],
    );
    render_card(frame, rows[1], build_stage_text(state, theme), theme);
    render_card(frame, rows[2], build_item_text(state, theme), theme);
    render_card(frame, rows[3], build_note_text(state, theme), theme);
    frame.render_widget(
        Paragraph::new(build_footer_text(theme))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.muted).bg(theme.overlay_surface())),
        rows[4],
    );
}

fn render_card(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    text: Text<'static>,
    theme: ThemePalette,
) {
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.chrome_border()))
            .style(Style::default().bg(theme.elevated_surface())),
        area,
    );
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.elevated_surface())),
        area.inner(Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );
}

fn build_summary_text(state: &StartupLoadingState, theme: ThemePalette) -> Text<'static> {
    let completed = state.completed_count();
    let total = BootProgressStage::ALL.len();
    Text::from(vec![
        Line::from(vec![
            Span::styled(
                "Loading Session",
                Style::default()
                    .fg(theme.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(theme.subtle)),
            Span::styled(
                format!("{}/{} stages", completed, total),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("workspace", Style::default().fg(theme.subtle)),
            Span::raw(" "),
            Span::styled(
                state.workspace_name.clone(),
                Style::default().fg(theme.muted),
            ),
            Span::styled("  ·  ", Style::default().fg(theme.subtle)),
            Span::styled("model", Style::default().fg(theme.subtle)),
            Span::raw(" "),
            Span::styled(state.model_label.clone(), Style::default().fg(theme.user)),
        ]),
        Line::from(vec![Span::styled(
            "Skills, MCP, tooling, and persisted session surfaces are being prepared before the welcome screen.",
            Style::default().fg(theme.muted),
        )]),
    ])
}

fn build_stage_text(state: &StartupLoadingState, theme: ThemePalette) -> Text<'static> {
    let mut lines = vec![section_line("Progress", theme.accent), Line::raw("")];
    for stage in BootProgressStage::ALL {
        let index = stage.position();
        let (marker, marker_style, label_style) = if state.completed[index] {
            (
                "●",
                Style::default()
                    .fg(theme.assistant)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.muted),
            )
        } else if stage == state.current_stage {
            (
                "◉",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                "·",
                Style::default().fg(theme.subtle),
                Style::default().fg(theme.subtle),
            )
        };
        lines.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled(stage.label().to_string(), label_style),
            Span::styled("  ", Style::default().fg(theme.subtle)),
            Span::styled(
                stage_status_label(stage, state),
                Style::default().fg(stage_status_color(stage, state, theme)),
            ),
        ]));
    }
    Text::from(lines)
}

fn build_item_text(state: &StartupLoadingState, theme: ThemePalette) -> Text<'static> {
    let items = state.visible_stage_items();
    let mut lines = vec![section_line(
        format!("{} Items", state.current_stage.label()),
        theme.user,
    )];
    if let Some(note) = state.current_note() {
        lines.push(Line::from(vec![Span::styled(
            note.to_string(),
            Style::default().fg(theme.muted),
        )]));
    }
    lines.push(Line::raw(""));
    if items.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "Waiting for concrete items from the current boot stage.",
            Style::default().fg(theme.subtle),
        )]));
        return Text::from(lines);
    }
    for item in items.iter().take(ITEM_PREVIEW_LIMIT) {
        lines.push(Line::from(vec![
            Span::styled(
                "• ",
                Style::default()
                    .fg(item_kind_color(item.kind, theme))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                item_kind_label(item.kind),
                Style::default()
                    .fg(theme.subtle)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().fg(theme.subtle)),
            Span::styled(item.label.clone(), Style::default().fg(theme.text)),
        ]));
    }
    if items.len() > ITEM_PREVIEW_LIMIT {
        lines.push(Line::from(vec![Span::styled(
            format!("+ {} more item(s)", items.len() - ITEM_PREVIEW_LIMIT),
            Style::default().fg(theme.subtle),
        )]));
    }
    Text::from(lines)
}

fn build_note_text(state: &StartupLoadingState, theme: ThemePalette) -> Text<'static> {
    let mut lines = vec![section_line("Recent Notes", theme.warn), Line::raw("")];
    for note in &state.recent_notes {
        lines.push(Line::from(vec![
            Span::styled("• ", Style::default().fg(theme.warn)),
            Span::styled(note.clone(), Style::default().fg(theme.muted)),
        ]));
    }
    Text::from(lines)
}

fn build_footer_text(theme: ThemePalette) -> Text<'static> {
    Text::from(vec![Line::from(vec![
        Span::styled("startup", Style::default().fg(theme.subtle)),
        Span::styled(" · ", Style::default().fg(theme.subtle)),
        Span::styled(
            "The session will enter the normal welcome page as soon as finalization completes.",
            Style::default().fg(theme.muted),
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

fn stage_status_color(
    stage: BootProgressStage,
    state: &StartupLoadingState,
    theme: ThemePalette,
) -> Color {
    let index = stage.position();
    if state.completed[index] {
        theme.assistant
    } else if stage == state.current_stage {
        theme.accent
    } else {
        theme.subtle
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

fn item_kind_color(kind: BootProgressItemKind, theme: ThemePalette) -> Color {
    match kind {
        BootProgressItemKind::Store => theme.user,
        BootProgressItemKind::Plugin => theme.warn,
        BootProgressItemKind::SkillRoot | BootProgressItemKind::Skill => theme.accent,
        BootProgressItemKind::McpServer => theme.assistant,
        BootProgressItemKind::ToolSurface => theme.header,
    }
}
