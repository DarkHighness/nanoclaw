use crate::TuiState;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(chunks[0]);

    let transcript = Paragraph::new(state.transcript_text())
        .block(Block::default().borders(Borders::ALL).title("Conversation"))
        .wrap(Wrap { trim: false });
    frame.render_widget(transcript, top[0]);

    let sidebar =
        Paragraph::new(state.sidebar_text())
            .block(Block::default().borders(Borders::ALL).title(
                if state.sidebar_title.is_empty() {
                    "Sidebar"
                } else {
                    state.sidebar_title.as_str()
                },
            ))
            .wrap(Wrap { trim: false });
    frame.render_widget(sidebar, top[1]);

    let status = Paragraph::new(state.status.clone())
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: false });
    frame.render_widget(status, chunks[1]);

    let input = Paragraph::new(state.input.clone())
        .block(Block::default().borders(Borders::ALL).title("Input"))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, chunks[2]);
}
