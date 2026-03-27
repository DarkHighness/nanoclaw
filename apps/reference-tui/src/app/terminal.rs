use super::{RuntimeTui, TuiState, observer::LiveRenderObserver};
use crate::{parse_command, render};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};

impl RuntimeTui {
    pub async fn run(mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        let mut state = TuiState {
            sidebar: self.startup_summary.sidebar.clone(),
            sidebar_title: self.startup_summary.sidebar_title.clone(),
            status: self.startup_summary.status.clone(),
            ..TuiState::default()
        };

        let result = self.event_loop(&mut terminal, &mut state).await;

        disable_raw_mode()?;
        crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        result
    }

    // Raw terminal lifecycle and key handling stay together so alternate-screen
    // teardown, command dispatch, and streaming redraws do not drift apart.
    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        state: &mut TuiState,
    ) -> anyhow::Result<()> {
        loop {
            terminal.draw(|frame| render(frame, state))?;
            if !event::poll(std::time::Duration::from_millis(100))? {
                continue;
            }
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Enter => {
                        let input = std::mem::take(&mut state.input);
                        if let Some(command) = parse_command(&input, &self.command_prefix) {
                            match self.apply_command(command, state).await {
                                Ok(true) => return Ok(()),
                                Ok(false) => {}
                                Err(error) => state.status = format!("Command error: {error}"),
                            }
                            continue;
                        }
                        if input.trim().is_empty() {
                            continue;
                        }
                        state.status = "Running...".to_string();
                        let mut observer = LiveRenderObserver::new(terminal, state);
                        match self
                            .runtime
                            .run_user_prompt_with_observer(input.clone(), &mut observer)
                            .await
                        {
                            Ok(outcome) => self.apply_outcome(state, outcome).await?,
                            Err(error) => {
                                if let Ok(lines) =
                                    self.replay_run_lines(&self.runtime.run_id()).await
                                {
                                    if !lines.is_empty() {
                                        state.transcript = lines;
                                    }
                                }
                                if state.transcript.is_empty() {
                                    state.transcript.push(format!("user> {input}"));
                                }
                                state.transcript.push(format!("error> {error}"));
                                state.status = format!("Error: {error}");
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        state.input.pop();
                    }
                    KeyCode::Char(ch) => {
                        state.input.push(ch);
                    }
                    _ => {}
                }
            }
        }
    }
}
