use super::super::{RuntimeTui, TuiState, build_turn_sidebar, preview_text};
use crate::TuiCommand;

impl RuntimeTui {
    pub(in crate::app) async fn apply_session_command(
        &mut self,
        command: TuiCommand,
        state: &mut TuiState,
    ) -> anyhow::Result<bool> {
        match command {
            TuiCommand::Status => {
                self.restore_startup_summary(state);
                state.status = self.startup_summary.status.clone();
                Ok(false)
            }
            TuiCommand::Clear => {
                state.transcript.clear();
                self.restore_startup_summary(state);
                state.status = "Cleared transcript".to_string();
                Ok(false)
            }
            TuiCommand::Compact { instructions } => {
                if self.runtime.compact_now(instructions.clone()).await? {
                    state.transcript = self.replay_run_lines(&self.runtime.session_id()).await?;
                    let events = self.store.events(&self.runtime.session_id()).await?;
                    state.sidebar = build_turn_sidebar(&events);
                    state.sidebar_title = "Turn".to_string();
                    state.status = if let Some(instructions) = instructions {
                        format!(
                            "Compacted visible history with notes: {}",
                            preview_text(&instructions, 48)
                        )
                    } else {
                        "Compacted visible history".to_string()
                    };
                } else {
                    state.status = "Compaction skipped".to_string();
                }
                Ok(false)
            }
            _ => unreachable!("session handler received non-session command"),
        }
    }
}
