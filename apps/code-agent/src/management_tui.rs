use agent::mcp::{McpServerConfig, McpTransportConfig};
use agent::types::{ToolOrigin, ToolSource, ToolSpec};
use anyhow::Result;
use code_agent_config::{
    ManagedPluginDetail, ManagedSkillDetail, disabled_tool_names, list_core_mcp_servers,
    list_managed_plugin_details, list_managed_skill_details, set_core_mcp_server_enabled,
    set_managed_plugin_enabled, set_managed_skill_enabled, set_tool_enabled,
};
use code_agent_tui::theme::{ThemePalette, active_palette};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagementSurfaceKind {
    Mcp,
    Tool,
    Skill,
    Plugin,
}

impl ManagementSurfaceKind {
    const CONFIG_ONLY: [Self; 3] = [Self::Mcp, Self::Skill, Self::Plugin];
    const WITH_TOOL: [Self; 4] = [Self::Mcp, Self::Tool, Self::Skill, Self::Plugin];

    fn title(self) -> &'static str {
        match self {
            Self::Mcp => "MCP",
            Self::Tool => "Tools",
            Self::Skill => "Skills",
            Self::Plugin => "Plugins",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToolCatalogSnapshot {
    pub tool_specs: Vec<ToolSpec>,
    pub startup_disabled_tool_names: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusTone {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DetailSection {
    title: String,
    rows: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManagementItem {
    id: String,
    title: String,
    badge: String,
    summary: String,
    enabled: bool,
    sections: Vec<DetailSection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SurfaceState {
    items: Vec<ManagementItem>,
    selected: usize,
}

impl Default for SurfaceState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected: 0,
        }
    }
}

impl SurfaceState {
    fn selected_item(&self) -> Option<&ManagementItem> {
        self.items.get(self.selected)
    }

    fn set_selected_by_id(&mut self, id: Option<&str>) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        if let Some(id) = id
            && let Some(index) = self.items.iter().position(|item| item.id == id)
        {
            self.selected = index;
            return;
        }
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1).min(self.items.len() - 1);
        }
    }

    fn move_home(&mut self) {
        self.selected = 0;
    }

    fn move_end(&mut self) {
        if !self.items.is_empty() {
            self.selected = self.items.len() - 1;
        }
    }
}

#[derive(Clone, Debug)]
struct ManagementTuiState {
    workspace_root: PathBuf,
    workspace_name: String,
    active: ManagementSurfaceKind,
    mcp: SurfaceState,
    tool: SurfaceState,
    skill: SurfaceState,
    plugin: SurfaceState,
    tool_catalog: Option<ToolCatalogSnapshot>,
    status_message: String,
    status_tone: StatusTone,
}

impl ManagementTuiState {
    async fn load(
        workspace_root: PathBuf,
        initial_surface: ManagementSurfaceKind,
        tool_catalog: Option<ToolCatalogSnapshot>,
    ) -> Result<Self> {
        let workspace_name = workspace_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace")
            .to_string();
        let initial_surface =
            if initial_surface == ManagementSurfaceKind::Tool && tool_catalog.is_none() {
                ManagementSurfaceKind::Mcp
            } else {
                initial_surface
            };
        Ok(Self {
            workspace_root: workspace_root.clone(),
            workspace_name,
            active: initial_surface,
            mcp: load_surface_state(&workspace_root, ManagementSurfaceKind::Mcp, None).await?,
            tool: load_surface_state(
                &workspace_root,
                ManagementSurfaceKind::Tool,
                tool_catalog.as_ref(),
            )
            .await?,
            skill: load_surface_state(&workspace_root, ManagementSurfaceKind::Skill, None).await?,
            plugin: load_surface_state(&workspace_root, ManagementSurfaceKind::Plugin, None)
                .await?,
            tool_catalog,
            status_message: "Use Tab to switch surfaces and Space to toggle the selected entry."
                .to_string(),
            status_tone: StatusTone::Info,
        })
    }

    fn available_kinds(&self) -> &'static [ManagementSurfaceKind] {
        if self.tool_catalog.is_some() {
            &ManagementSurfaceKind::WITH_TOOL
        } else {
            &ManagementSurfaceKind::CONFIG_ONLY
        }
    }

    fn next_surface(&self, current: ManagementSurfaceKind) -> ManagementSurfaceKind {
        let kinds = self.available_kinds();
        let index = kinds.iter().position(|kind| *kind == current).unwrap_or(0);
        kinds[(index + 1) % kinds.len()]
    }

    fn previous_surface(&self, current: ManagementSurfaceKind) -> ManagementSurfaceKind {
        let kinds = self.available_kinds();
        let index = kinds.iter().position(|kind| *kind == current).unwrap_or(0);
        kinds[(index + kinds.len() - 1) % kinds.len()]
    }

    fn jump_to_surface_index(&mut self, index: usize) {
        if let Some(kind) = self.available_kinds().get(index).copied() {
            self.active = kind;
        }
    }

    fn surface(&self, kind: ManagementSurfaceKind) -> &SurfaceState {
        match kind {
            ManagementSurfaceKind::Mcp => &self.mcp,
            ManagementSurfaceKind::Tool => &self.tool,
            ManagementSurfaceKind::Skill => &self.skill,
            ManagementSurfaceKind::Plugin => &self.plugin,
        }
    }

    fn surface_mut(&mut self, kind: ManagementSurfaceKind) -> &mut SurfaceState {
        match kind {
            ManagementSurfaceKind::Mcp => &mut self.mcp,
            ManagementSurfaceKind::Tool => &mut self.tool,
            ManagementSurfaceKind::Skill => &mut self.skill,
            ManagementSurfaceKind::Plugin => &mut self.plugin,
        }
    }

    fn active_surface(&self) -> &SurfaceState {
        self.surface(self.active)
    }

    fn active_surface_mut(&mut self) -> &mut SurfaceState {
        self.surface_mut(self.active)
    }

    fn set_status(&mut self, tone: StatusTone, message: impl Into<String>) {
        self.status_tone = tone;
        self.status_message = message.into();
    }

    async fn refresh_surface(&mut self, kind: ManagementSurfaceKind) -> Result<()> {
        let selected_id = self
            .surface(kind)
            .selected_item()
            .map(|item| item.id.as_str().to_string());
        let mut refreshed =
            load_surface_state(&self.workspace_root, kind, self.tool_catalog.as_ref()).await?;
        refreshed.set_selected_by_id(selected_id.as_deref());
        *self.surface_mut(kind) = refreshed;
        Ok(())
    }

    async fn refresh_all(&mut self) -> Result<()> {
        for kind in self.available_kinds() {
            self.refresh_surface(*kind).await?;
        }
        Ok(())
    }

    async fn toggle_selected(&mut self) {
        let kind = self.active;
        let selected = self
            .active_surface()
            .selected_item()
            .map(|item| (item.id.clone(), item.enabled, item.title.clone()));
        let Some((id, enabled, title)) = selected else {
            self.set_status(
                StatusTone::Info,
                "Nothing is available to toggle on this surface.",
            );
            return;
        };
        let target_enabled = !enabled;
        let result = match kind {
            ManagementSurfaceKind::Mcp => {
                set_core_mcp_server_enabled(&self.workspace_root, &id, target_enabled).map(|_| ())
            }
            ManagementSurfaceKind::Tool => {
                set_tool_enabled(&self.workspace_root, &id, target_enabled).map(|_| ())
            }
            ManagementSurfaceKind::Skill => {
                set_managed_skill_enabled(&self.workspace_root, &id, target_enabled)
                    .await
                    .map(|_| ())
            }
            ManagementSurfaceKind::Plugin => {
                set_managed_plugin_enabled(&self.workspace_root, &id, target_enabled).map(|_| ())
            }
        };
        match result {
            Ok(()) => match self.refresh_surface(kind).await {
                Ok(()) => self.set_status(
                    StatusTone::Success,
                    match kind {
                        ManagementSurfaceKind::Tool => format!(
                            "{} {} in workspace config.",
                            if target_enabled {
                                "Enabled"
                            } else {
                                "Disabled"
                            },
                            title
                        ),
                        _ => format!(
                            "{} {}.",
                            if target_enabled {
                                "Enabled"
                            } else {
                                "Disabled"
                            },
                            title
                        ),
                    },
                ),
                Err(error) => self.set_status(
                    StatusTone::Error,
                    format!("State changed, but refresh failed: {error}"),
                ),
            },
            Err(error) => self.set_status(StatusTone::Error, error.to_string()),
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => return Ok(false),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(false);
            }
            KeyCode::Char('q') => return Ok(false),
            KeyCode::Tab | KeyCode::Right => {
                self.active = self.next_surface(self.active);
                self.set_status(
                    StatusTone::Info,
                    format!("Switched to {}.", self.active.title()),
                );
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.active = self.previous_surface(self.active);
                self.set_status(
                    StatusTone::Info,
                    format!("Switched to {}.", self.active.title()),
                );
            }
            KeyCode::Char('1') => self.jump_to_surface_index(0),
            KeyCode::Char('2') => self.jump_to_surface_index(1),
            KeyCode::Char('3') => self.jump_to_surface_index(2),
            KeyCode::Char('4') => self.jump_to_surface_index(3),
            KeyCode::Up | KeyCode::Char('k') => self.active_surface_mut().move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.active_surface_mut().move_down(),
            KeyCode::Home => self.active_surface_mut().move_home(),
            KeyCode::End => self.active_surface_mut().move_end(),
            KeyCode::Char('r') => match self.refresh_all().await {
                Ok(()) => self.set_status(StatusTone::Success, "Refreshed managed surfaces."),
                Err(error) => self.set_status(StatusTone::Error, error.to_string()),
            },
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.toggle_selected().await;
            }
            _ => {}
        }
        Ok(true)
    }
}

struct ManagementTuiScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl ManagementTuiScreen {
    fn enter() -> Result<Self> {
        Ok(Self {
            terminal: enter_terminal()?,
            active: true,
        })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    fn leave(mut self) -> Result<()> {
        self.active = false;
        leave_terminal(&mut self.terminal)
    }
}

impl Drop for ManagementTuiScreen {
    fn drop(&mut self) {
        if self.active {
            best_effort_leave_terminal(&mut self.terminal);
        }
    }
}

pub(crate) async fn run_management_tui(
    workspace_root: PathBuf,
    initial_surface: ManagementSurfaceKind,
    tool_catalog: Option<ToolCatalogSnapshot>,
) -> Result<()> {
    let mut state = ManagementTuiState::load(workspace_root, initial_surface, tool_catalog).await?;
    let mut screen = ManagementTuiScreen::enter()?;
    let result = run_management_loop(screen.terminal_mut(), &mut state).await;
    let cleanup = screen.leave();
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(_cleanup_error)) => Err(error),
    }
}

async fn run_management_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut ManagementTuiState,
) -> Result<()> {
    loop {
        terminal.draw(|frame| render_management_tui(frame, state))?;
        if !event::poll(std::time::Duration::from_millis(250))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if !state.handle_key(key).await? {
                    return Ok(());
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn enter_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
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

fn leave_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let raw_result = disable_raw_mode();
    let screen_result = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor_result = terminal.show_cursor();
    raw_result?;
    screen_result?;
    cursor_result?;
    Ok(())
}

fn best_effort_leave_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
}

fn render_management_tui(frame: &mut ratatui::Frame<'_>, state: &ManagementTuiState) {
    let theme = active_palette();
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.canvas_surface())),
        area,
    );
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(rows[2]);

    frame.render_widget(
        Paragraph::new(build_header_text(state, theme))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.canvas_surface())),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(build_tabs_text(state, theme))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.canvas_surface())),
        rows[1],
    );
    render_list_panel(frame, columns[0], state, theme);
    render_detail_panel(frame, columns[1], state, theme);
    frame.render_widget(
        Paragraph::new(build_footer_text(state, theme))
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(theme.canvas_surface())),
        rows[3],
    );
}

fn build_header_text(state: &ManagementTuiState, theme: ThemePalette) -> Text<'static> {
    let active_surface = state.active_surface();
    Text::from(vec![
        Line::from(vec![
            Span::styled(
                "Managed Surfaces",
                Style::default()
                    .fg(theme.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(theme.subtle)),
            Span::styled(
                format!("workspace {}", state.workspace_name),
                Style::default().fg(theme.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled("focused", Style::default().fg(theme.subtle)),
            Span::raw(" "),
            Span::styled(
                state.active.title().to_ascii_lowercase(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::default().fg(theme.subtle)),
            Span::styled("entries", Style::default().fg(theme.subtle)),
            Span::raw(" "),
            Span::styled(
                active_surface.items.len().to_string(),
                Style::default().fg(theme.user),
            ),
        ]),
    ])
}

fn build_tabs_text(state: &ManagementTuiState, theme: ThemePalette) -> Text<'static> {
    let mut spans = Vec::new();
    for (index, kind) in state.available_kinds().iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("   ", Style::default().fg(theme.subtle)));
        }
        let count = state.surface(*kind).items.len();
        let style = if state.active == *kind {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        spans.push(Span::styled(format!("{} [{}]", kind.title(), count), style));
    }
    Text::from(Line::from(spans))
}

fn render_list_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &ManagementTuiState,
    theme: ThemePalette,
) {
    let surface = state.active_surface();
    let block = Block::default()
        .title(format!(" {} ", state.active.title()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.emphasis_border()))
        .style(Style::default().bg(theme.overlay_surface()));
    frame.render_widget(block.clone(), area);
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let text = build_list_text(surface, theme);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.overlay_surface())),
        inner,
    );
}

fn build_list_text(surface: &SurfaceState, theme: ThemePalette) -> Text<'static> {
    if surface.items.is_empty() {
        return Text::from(vec![
            Line::from(Span::styled(
                "No entries are available on this surface.",
                Style::default().fg(theme.subtle),
            )),
            Line::from(Span::styled(
                "Try refresh if you just changed workspace configuration.",
                Style::default().fg(theme.subtle),
            )),
        ]);
    }
    let capacity = 9usize.max(surface.items.len().min(12));
    let (start, end) = visible_window(surface.selected, surface.items.len(), capacity);
    let mut lines = Vec::new();
    for (index, item) in surface.items[start..end].iter().enumerate() {
        let absolute_index = start + index;
        let selected = absolute_index == surface.selected;
        let marker_style = if selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.subtle)
        };
        let title_style = if selected {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        let toggle_style = if item.enabled {
            Style::default().fg(theme.assistant)
        } else {
            Style::default().fg(theme.subtle)
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { ">" } else { " " }, marker_style),
            Span::raw(" "),
            Span::styled(if item.enabled { "[on] " } else { "[off]" }, toggle_style),
            Span::raw(" "),
            Span::styled(item.title.clone(), title_style),
            Span::styled("  ", Style::default().fg(theme.subtle)),
            Span::styled(
                item.badge.clone(),
                Style::default().fg(if selected { theme.user } else { theme.subtle }),
            ),
        ]));
        if !item.summary.trim().is_empty() {
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default().fg(theme.subtle)),
                Span::styled(
                    item.summary.clone(),
                    Style::default().fg(if selected { theme.text } else { theme.subtle }),
                ),
            ]));
        }
    }
    Text::from(lines)
}

fn render_detail_panel(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &ManagementTuiState,
    theme: ThemePalette,
) {
    let block = Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.chrome_border()))
        .style(Style::default().bg(theme.elevated_surface()));
    frame.render_widget(block.clone(), area);
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });
    let text = build_detail_text(state, theme);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.elevated_surface())),
        inner,
    );
}

fn build_detail_text(state: &ManagementTuiState, theme: ThemePalette) -> Text<'static> {
    let Some(item) = state.active_surface().selected_item() else {
        return Text::from(Line::from(Span::styled(
            "Select an entry to inspect its configuration.",
            Style::default().fg(theme.subtle),
        )));
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                item.title.clone(),
                Style::default()
                    .fg(theme.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(theme.subtle)),
            Span::styled(
                if item.enabled { "enabled" } else { "disabled" },
                Style::default().fg(if item.enabled {
                    theme.assistant
                } else {
                    theme.warn
                }),
            ),
            Span::styled("  ", Style::default().fg(theme.subtle)),
            Span::styled(item.badge.clone(), Style::default().fg(theme.user)),
        ]),
        Line::from(Span::styled(
            item.summary.clone(),
            Style::default().fg(theme.muted),
        )),
    ];
    for section in &item.sections {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![Span::styled(
            section.title.clone(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )]));
        for (key, value) in &section.rows {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().fg(theme.subtle)),
                Span::styled(format!("{key:12}"), Style::default().fg(theme.subtle)),
                Span::styled(value.clone(), Style::default().fg(theme.text)),
            ]));
        }
    }
    Text::from(lines)
}

fn build_footer_text(state: &ManagementTuiState, theme: ThemePalette) -> Text<'static> {
    let tone = match state.status_tone {
        StatusTone::Info => theme.muted,
        StatusTone::Success => theme.assistant,
        StatusTone::Error => theme.error,
    };
    let jump_hint = if state.tool_catalog.is_some() {
        "1-4"
    } else {
        "1-3"
    };
    Text::from(vec![Line::from(vec![
        Span::styled("tab", Style::default().fg(theme.accent)),
        Span::styled(" switch", Style::default().fg(theme.subtle)),
        Span::styled(" · ", Style::default().fg(theme.subtle)),
        Span::styled(jump_hint, Style::default().fg(theme.accent)),
        Span::styled(" jump", Style::default().fg(theme.subtle)),
        Span::styled(" · ", Style::default().fg(theme.subtle)),
        Span::styled("up/down", Style::default().fg(theme.accent)),
        Span::styled(" move", Style::default().fg(theme.subtle)),
        Span::styled(" · ", Style::default().fg(theme.subtle)),
        Span::styled("space", Style::default().fg(theme.accent)),
        Span::styled(" toggle", Style::default().fg(theme.subtle)),
        Span::styled(" · ", Style::default().fg(theme.subtle)),
        Span::styled("r", Style::default().fg(theme.accent)),
        Span::styled(" refresh", Style::default().fg(theme.subtle)),
        Span::styled(" · ", Style::default().fg(theme.subtle)),
        Span::styled("q", Style::default().fg(theme.accent)),
        Span::styled(" quit", Style::default().fg(theme.subtle)),
        Span::styled("  |  ", Style::default().fg(theme.subtle)),
        Span::styled(state.status_message.clone(), Style::default().fg(tone)),
    ])])
}

fn visible_window(selected: usize, total: usize, capacity: usize) -> (usize, usize) {
    if total <= capacity {
        return (0, total);
    }
    let half = capacity / 2;
    let mut start = selected.saturating_sub(half);
    if start + capacity > total {
        start = total.saturating_sub(capacity);
    }
    (start, (start + capacity).min(total))
}

async fn load_surface_state(
    workspace_root: &Path,
    kind: ManagementSurfaceKind,
    tool_catalog: Option<&ToolCatalogSnapshot>,
) -> Result<SurfaceState> {
    let items = match kind {
        ManagementSurfaceKind::Mcp => list_core_mcp_servers(workspace_root)?
            .into_iter()
            .map(|server| build_mcp_item(workspace_root, server))
            .collect(),
        ManagementSurfaceKind::Tool => tool_catalog
            .map(|catalog| build_tool_items(workspace_root, catalog))
            .transpose()?
            .unwrap_or_default(),
        ManagementSurfaceKind::Skill => list_managed_skill_details(workspace_root)
            .await?
            .into_iter()
            .map(|skill| build_skill_item(workspace_root, skill))
            .collect(),
        ManagementSurfaceKind::Plugin => list_managed_plugin_details(workspace_root)?
            .into_iter()
            .map(|plugin| build_plugin_item(workspace_root, plugin))
            .collect(),
    };
    Ok(SurfaceState { items, selected: 0 })
}

fn build_mcp_item(workspace_root: &Path, server: McpServerConfig) -> ManagementItem {
    let mut sections = vec![DetailSection {
        title: "State".to_string(),
        rows: vec![
            (
                "state".to_string(),
                enabled_label(server.enabled).to_string(),
            ),
            (
                "transport".to_string(),
                mcp_transport_label(&server.transport).to_string(),
            ),
        ],
    }];
    let (summary, badge) = match &server.transport {
        McpTransportConfig::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let mut launch_rows = vec![("command".to_string(), command.clone())];
            if !args.is_empty() {
                launch_rows.push(("args".to_string(), args.join(" ")));
            }
            if let Some(cwd) = cwd.as_deref() {
                launch_rows.push(("cwd".to_string(), cwd.to_string()));
            }
            if !env.is_empty() {
                launch_rows.push(("env keys".to_string(), join_keys(env.keys().cloned())));
            }
            sections.push(DetailSection {
                title: "Launch".to_string(),
                rows: launch_rows,
            });
            let summary = if args.is_empty() {
                command.clone()
            } else {
                format!("{command} {}", args.join(" "))
            };
            (summary, "stdio".to_string())
        }
        McpTransportConfig::StreamableHttp { url, headers } => {
            let mut endpoint_rows = vec![("url".to_string(), url.clone())];
            if !headers.is_empty() {
                endpoint_rows.push(("headers".to_string(), join_keys(headers.keys().cloned())));
            }
            sections.push(DetailSection {
                title: "Endpoint".to_string(),
                rows: endpoint_rows,
            });
            (url.clone(), "http".to_string())
        }
    };
    sections.push(DetailSection {
        title: "Files".to_string(),
        rows: vec![(
            "config".to_string(),
            display_workspace_path(
                workspace_root,
                &workspace_root.join(".nanoclaw/config/core.toml"),
            ),
        )],
    });
    ManagementItem {
        id: server.name.to_string(),
        title: server.name.to_string(),
        badge,
        summary,
        enabled: server.enabled,
        sections,
    }
}

fn build_skill_item(workspace_root: &Path, skill: ManagedSkillDetail) -> ManagementItem {
    let summary = if skill.description.trim().is_empty() {
        display_workspace_path(workspace_root, &skill.skill_path)
    } else {
        skill.description.clone()
    };
    ManagementItem {
        id: skill.skill_name.clone(),
        title: skill.skill_name,
        badge: if skill.builtin {
            "built-in".to_string()
        } else {
            "managed".to_string()
        },
        summary,
        enabled: skill.enabled,
        sections: vec![
            DetailSection {
                title: "State".to_string(),
                rows: vec![
                    (
                        "state".to_string(),
                        enabled_label(skill.enabled).to_string(),
                    ),
                    (
                        "source".to_string(),
                        if skill.builtin { "built-in" } else { "managed" }.to_string(),
                    ),
                ],
            },
            DetailSection {
                title: "Files".to_string(),
                rows: vec![(
                    "path".to_string(),
                    display_workspace_path(workspace_root, &skill.skill_path),
                )],
            },
            DetailSection {
                title: "Trigger".to_string(),
                rows: vec![("description".to_string(), skill.description)],
            },
        ],
    }
}

fn build_plugin_item(workspace_root: &Path, plugin: ManagedPluginDetail) -> ManagementItem {
    let title = plugin
        .name
        .clone()
        .unwrap_or_else(|| plugin.plugin_id.to_string());
    let summary = plugin
        .description
        .clone()
        .unwrap_or_else(|| plugin.contribution_summary.clone());
    let mut identity_rows = vec![
        ("id".to_string(), plugin.plugin_id.to_string()),
        ("kind".to_string(), plugin.kind.clone()),
    ];
    if let Some(version) = plugin.version.as_deref() {
        identity_rows.push(("version".to_string(), version.to_string()));
    }
    let mut sections = vec![
        DetailSection {
            title: "Identity".to_string(),
            rows: identity_rows,
        },
        DetailSection {
            title: "State".to_string(),
            rows: vec![
                (
                    "state".to_string(),
                    enabled_label(plugin.enabled).to_string(),
                ),
                ("reason".to_string(), plugin.reason.clone()),
            ],
        },
        DetailSection {
            title: "Files".to_string(),
            rows: vec![(
                "path".to_string(),
                display_workspace_path(workspace_root, &plugin.plugin_path),
            )],
        },
    ];
    if !plugin.contribution_summary.trim().is_empty() {
        sections.push(DetailSection {
            title: "Contribution".to_string(),
            rows: vec![("summary".to_string(), plugin.contribution_summary.clone())],
        });
    }
    if let Some(description) = plugin.description.as_deref() {
        sections.push(DetailSection {
            title: "Description".to_string(),
            rows: vec![("text".to_string(), description.to_string())],
        });
    }
    ManagementItem {
        id: plugin.plugin_id.to_string(),
        title,
        badge: plugin.kind,
        summary,
        enabled: plugin.enabled,
        sections,
    }
}

#[derive(Clone)]
struct ToolCatalogEntry {
    name: String,
    startup_enabled: bool,
    config_enabled: bool,
    spec: Option<ToolSpec>,
}

fn build_tool_items(
    workspace_root: &Path,
    catalog: &ToolCatalogSnapshot,
) -> Result<Vec<ManagementItem>> {
    // Tool management toggles the persisted workspace disabled list, but the
    // live startup state can still diverge when environment variables or
    // startup policy remove a tool from the active registry for this process.
    let configured_disabled = disabled_tool_names(workspace_root)?;
    let startup_disabled = catalog
        .startup_disabled_tool_names
        .iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    let mut entries = std::collections::BTreeMap::new();
    for spec in &catalog.tool_specs {
        let name = spec.name.to_string();
        entries.insert(
            name.clone(),
            ToolCatalogEntry {
                name: name.clone(),
                startup_enabled: !startup_disabled.contains(&name),
                config_enabled: !configured_disabled.contains(&name),
                spec: Some(spec.clone()),
            },
        );
    }
    for name in startup_disabled.union(&configured_disabled) {
        entries.entry(name.clone()).or_insert(ToolCatalogEntry {
            name: name.clone(),
            startup_enabled: !startup_disabled.contains(name),
            config_enabled: !configured_disabled.contains(name),
            spec: None,
        });
    }
    Ok(entries
        .into_values()
        .map(build_tool_item)
        .collect::<Vec<_>>())
}

fn build_tool_item(entry: ToolCatalogEntry) -> ManagementItem {
    let summary = entry
        .spec
        .as_ref()
        .map(|spec| spec.description.clone())
        .unwrap_or_else(|| {
            "Configured in workspace state but not present in the current startup catalog."
                .to_string()
        });
    let mut sections = vec![DetailSection {
        title: "State".to_string(),
        rows: vec![
            (
                "config".to_string(),
                enabled_label(entry.config_enabled).to_string(),
            ),
            (
                "startup".to_string(),
                enabled_label(entry.startup_enabled).to_string(),
            ),
        ],
    }];
    if entry.startup_enabled != entry.config_enabled {
        sections.push(DetailSection {
            title: "Notes".to_string(),
            rows: vec![(
                "startup".to_string(),
                if entry.startup_enabled {
                    "Current startup resolved this tool as enabled even though the workspace config disables it."
                } else {
                    "Current startup resolved this tool as disabled by environment or startup policy."
                }
                .to_string(),
            )],
        });
    }
    match entry.spec {
        Some(spec) => {
            sections.insert(
                0,
                DetailSection {
                    title: "Identity".to_string(),
                    rows: vec![
                        ("kind".to_string(), tool_kind_label(&spec).to_string()),
                        ("source".to_string(), tool_source_label(&spec)),
                        ("origin".to_string(), tool_origin_label(&spec)),
                    ],
                },
            );
            if !spec.aliases.is_empty() {
                sections.push(DetailSection {
                    title: "Aliases".to_string(),
                    rows: vec![(
                        "names".to_string(),
                        spec.aliases
                            .iter()
                            .map(|alias| alias.as_str().to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                    )],
                });
            }
            sections.push(DetailSection {
                title: "Approval".to_string(),
                rows: vec![
                    (
                        "read only".to_string(),
                        yes_no_label(spec.approval.read_only).to_string(),
                    ),
                    (
                        "mutates".to_string(),
                        yes_no_label(spec.approval.mutates_state).to_string(),
                    ),
                    (
                        "network".to_string(),
                        yes_no_label(spec.approval.needs_network).to_string(),
                    ),
                ],
            });
            ManagementItem {
                id: spec.name.to_string(),
                title: spec.name.to_string(),
                badge: tool_source_badge(&spec),
                summary,
                enabled: entry.config_enabled,
                sections,
            }
        }
        None => ManagementItem {
            id: entry.name.clone(),
            title: entry.name,
            badge: "config".to_string(),
            summary,
            enabled: entry.config_enabled,
            sections,
        },
    }
}

fn join_keys(keys: impl IntoIterator<Item = String>) -> String {
    let mut collected = keys.into_iter().collect::<Vec<_>>();
    collected.sort();
    collected.join(", ")
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

fn yes_no_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn mcp_transport_label(transport: &McpTransportConfig) -> &'static str {
    match transport {
        McpTransportConfig::Stdio { .. } => "stdio",
        McpTransportConfig::StreamableHttp { .. } => "http",
    }
}

fn display_workspace_path(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn tool_kind_label(spec: &ToolSpec) -> &'static str {
    match spec.kind {
        agent::types::ToolKind::Function => "function",
        agent::types::ToolKind::Freeform => "freeform",
        agent::types::ToolKind::Native => "native",
    }
}

fn tool_source_badge(spec: &ToolSpec) -> String {
    match &spec.source {
        ToolSource::Builtin => "builtin".to_string(),
        ToolSource::Dynamic => "dynamic".to_string(),
        ToolSource::Plugin { plugin } => format!("plugin:{plugin}"),
        ToolSource::McpTool { server_name } => format!("mcp:{server_name}"),
        ToolSource::McpResource { server_name, .. } => format!("mcp:{server_name}"),
        ToolSource::ProviderBuiltin { provider } => format!("provider:{provider}"),
    }
}

fn tool_source_label(spec: &ToolSpec) -> String {
    match &spec.source {
        ToolSource::Builtin => "builtin".to_string(),
        ToolSource::Dynamic => "dynamic".to_string(),
        ToolSource::Plugin { plugin } => format!("plugin `{plugin}`"),
        ToolSource::McpTool { server_name } => format!("MCP server `{server_name}`"),
        ToolSource::McpResource { server_name } => {
            format!("MCP resource surface from `{server_name}`")
        }
        ToolSource::ProviderBuiltin { provider } => format!("provider builtin `{provider}`"),
    }
}

fn tool_origin_label(spec: &ToolSpec) -> String {
    match &spec.origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("MCP server `{server_name}`"),
        ToolOrigin::Provider { provider } => format!("provider `{provider}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::types::{ToolName, ToolOutputMode};
    use serde_json::json;
    use tempfile::tempdir;

    fn sample_tool_spec(name: &str, description: &str) -> ToolSpec {
        ToolSpec::function(
            ToolName::from(name),
            description.to_string(),
            json!({"type":"object","properties":{}}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
    }

    #[tokio::test]
    async fn tool_surface_reads_config_toggle_state_separately_from_startup_state() {
        let workspace = tempdir().unwrap();
        set_tool_enabled(workspace.path(), "web_search", false).unwrap();
        let state = load_surface_state(
            workspace.path(),
            ManagementSurfaceKind::Tool,
            Some(&ToolCatalogSnapshot {
                tool_specs: vec![sample_tool_spec("web_search", "Search the web")],
                startup_disabled_tool_names: Vec::new(),
            }),
        )
        .await
        .unwrap();

        let item = state.selected_item().unwrap();
        assert_eq!(item.title, "web_search");
        assert!(!item.enabled);
        assert!(item.sections.iter().any(|section| {
            section.title == "State"
                && section
                    .rows
                    .iter()
                    .any(|(key, value)| key == "config" && value == "disabled")
        }));
        assert!(item.sections.iter().any(|section| {
            section.title == "State"
                && section
                    .rows
                    .iter()
                    .any(|(key, value)| key == "startup" && value == "enabled")
        }));
    }

    #[tokio::test]
    async fn tool_surface_keeps_unresolved_disabled_entries_visible() {
        let workspace = tempdir().unwrap();
        set_tool_enabled(workspace.path(), "ghost_tool", false).unwrap();
        let state = load_surface_state(
            workspace.path(),
            ManagementSurfaceKind::Tool,
            Some(&ToolCatalogSnapshot {
                tool_specs: Vec::new(),
                startup_disabled_tool_names: vec!["ghost_tool".to_string()],
            }),
        )
        .await
        .unwrap();

        let item = state.selected_item().unwrap();
        assert_eq!(item.title, "ghost_tool");
        assert_eq!(item.badge, "config");
        assert!(!item.enabled);
    }
}
