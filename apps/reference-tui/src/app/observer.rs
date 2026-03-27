use super::{TuiState, preview_text};
use crate::render;
use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use runtime::{Result as RuntimeResult, RuntimeError, RuntimeObserver, RuntimeProgressEvent};
use std::io::Stdout;
use types::ToolLifecycleEventKind;

// Streaming progress keeps a small amount of transient UI state that should stay
// separate from the keyboard loop. That way `RuntimeTui` remains the command and
// outcome facade while the observer owns incremental render bookkeeping.
pub(super) struct LiveRenderObserver<'a> {
    terminal: &'a mut Terminal<CrosstermBackend<Stdout>>,
    state: &'a mut TuiState,
    active_assistant_line: Option<usize>,
}

impl<'a> LiveRenderObserver<'a> {
    pub(super) fn new(
        terminal: &'a mut Terminal<CrosstermBackend<Stdout>>,
        state: &'a mut TuiState,
    ) -> Self {
        Self {
            terminal,
            state,
            active_assistant_line: None,
        }
    }

    fn redraw(&mut self) -> Result<()> {
        self.terminal.draw(|frame| render(frame, self.state))?;
        Ok(())
    }
}

impl RuntimeObserver for LiveRenderObserver<'_> {
    fn on_event(&mut self, event: RuntimeProgressEvent) -> RuntimeResult<()> {
        match event {
            RuntimeProgressEvent::SteerApplied { message, reason } => {
                self.active_assistant_line = None;
                self.state.transcript.push(format!("system> {message}"));
                self.state.status = match reason {
                    Some(reason) => format!("Steer applied ({reason})"),
                    None => "Steer applied".to_string(),
                };
            }
            RuntimeProgressEvent::UserPromptAdded { prompt } => {
                self.active_assistant_line = None;
                self.state.transcript.push(format!("user> {prompt}"));
                self.state.status = "Thinking...".to_string();
            }
            RuntimeProgressEvent::CompactionCompleted {
                reason,
                source_message_count,
                retained_message_count,
                summary,
            } => {
                self.state.status = format!(
                    "Compacted {} messages, kept {} recent ({reason})",
                    source_message_count, retained_message_count
                );
                self.state.sidebar_title = "Compaction".to_string();
                self.state.sidebar = vec![
                    format!("reason: {reason}"),
                    format!("source messages: {source_message_count}"),
                    format!("retained messages: {retained_message_count}"),
                    format!("summary: {}", preview_text(&summary, 120)),
                ];
            }
            RuntimeProgressEvent::ModelRequestStarted { iteration, .. } => {
                self.state.status = if iteration == 1 {
                    "Waiting for model response...".to_string()
                } else {
                    format!("Continuing tool loop (iteration {iteration})...")
                };
            }
            RuntimeProgressEvent::AssistantTextDelta { delta } => {
                if let Some(index) = self.active_assistant_line {
                    self.state.transcript[index].push_str(&delta);
                } else {
                    self.state.transcript.push(format!("assistant> {delta}"));
                    self.active_assistant_line = Some(self.state.transcript.len() - 1);
                }
                self.state.status = "Streaming response...".to_string();
            }
            RuntimeProgressEvent::ToolCallRequested { call } => {
                self.state.status = format!("Model requested tool `{}`", call.tool_name);
            }
            RuntimeProgressEvent::ModelResponseCompleted { tool_calls, .. } => {
                self.active_assistant_line = None;
                self.state.status = if tool_calls.is_empty() {
                    "Model response complete".to_string()
                } else {
                    format!("Model response requested {} tool(s)", tool_calls.len())
                };
            }
            RuntimeProgressEvent::ToolApprovalRequested { call, .. } => {
                self.state.status = format!("Approval required for `{}`", call.tool_name);
            }
            RuntimeProgressEvent::ToolApprovalResolved {
                call,
                approved,
                reason,
            } => {
                self.state.status = if approved {
                    format!("Approved `{}`", call.tool_name)
                } else {
                    format!(
                        "Denied `{}`: {}",
                        call.tool_name,
                        reason.unwrap_or_else(|| "permission denied".to_string())
                    )
                };
            }
            RuntimeProgressEvent::ToolLifecycle { event } => match event.event {
                ToolLifecycleEventKind::Started { call } => {
                    self.state.status = format!("Running tool `{}`...", call.tool_name);
                }
                ToolLifecycleEventKind::Completed { call, .. } => {
                    self.state.status = format!("Tool `{}` completed", call.tool_name);
                }
                ToolLifecycleEventKind::Failed { call, error } => {
                    self.state.status = format!(
                        "Tool `{}` failed: {}",
                        call.tool_name,
                        preview_text(&error, 64)
                    );
                }
                ToolLifecycleEventKind::Cancelled { call, reason } => {
                    self.state.status = format!(
                        "Tool `{}` cancelled{}",
                        call.tool_name,
                        reason
                            .as_deref()
                            .map(|value| format!(": {}", preview_text(value, 64)))
                            .unwrap_or_default()
                    );
                }
            },
            RuntimeProgressEvent::TurnCompleted { .. } => {
                self.active_assistant_line = None;
                self.state.status = "Turn complete".to_string();
            }
        }
        self.redraw()
            .map_err(|error| RuntimeError::invalid_state(error.to_string()))
    }
}
