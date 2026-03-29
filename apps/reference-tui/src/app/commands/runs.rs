use super::super::run_history::{encode_run_events_jsonl, resolve_run_reference};
use super::super::{
    RuntimeTui, TuiState, format_run_search_line, format_run_sidebar, format_run_summary_line,
    preview_id,
};
use crate::TuiCommand;

impl RuntimeTui {
    pub(in crate::app) async fn apply_runs_command(
        &mut self,
        command: TuiCommand,
        state: &mut TuiState,
    ) -> anyhow::Result<bool> {
        match command {
            TuiCommand::Runs { query } => {
                if let Some(query) = query {
                    let sessions = self.store.search_sessions(&query).await?;
                    state.sidebar = if sessions.is_empty() {
                        vec![format!("no sessions matched `{query}`")]
                    } else {
                        sessions
                            .iter()
                            .take(12)
                            .map(format_run_search_line)
                            .collect()
                    };
                    state.sidebar_title = "Run Search".to_string();
                    state.status = if sessions.is_empty() {
                        format!("No sessions matched `{query}`")
                    } else {
                        format!(
                            "Found {} matching sessions. Use {}session <id-prefix> to replay one.",
                            sessions.len(),
                            self.command_prefix
                        )
                    };
                } else {
                    let sessions = self.store.list_sessions().await?;
                    state.sidebar = if sessions.is_empty() {
                        vec!["no sessions recorded yet".to_string()]
                    } else {
                        sessions
                            .iter()
                            .take(12)
                            .map(format_run_summary_line)
                            .collect()
                    };
                    state.sidebar_title = "Runs".to_string();
                    state.status = if sessions.is_empty() {
                        "No sessions available yet".to_string()
                    } else {
                        format!(
                            "Listed {} sessions. Use {}session <id-prefix> to replay one.",
                            sessions.len(),
                            self.command_prefix
                        )
                    };
                }
                Ok(false)
            }
            TuiCommand::Run { run_ref } => {
                let sessions = self.store.list_sessions().await?;
                let session_id = resolve_run_reference(&sessions, &run_ref)?;
                let summary = sessions
                    .iter()
                    .find(|summary| summary.session_id == session_id)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!("session missing from store listing: {}", session_id)
                    })?;
                let events = self.store.events(&session_id).await?;
                let agent_session_ids = self.store.agent_session_ids(&session_id).await?;
                let token_usage = self.store.token_usage(&session_id).await?;
                state.transcript = self.replay_run_lines(&session_id).await?;
                state.sidebar =
                    format_run_sidebar(&summary, &agent_session_ids, &events, &token_usage);
                state.sidebar_title = "Run".to_string();
                state.status = format!(
                    "Loaded session {} with {} transcript messages",
                    preview_id(session_id.as_str()),
                    summary.transcript_message_count
                );
                Ok(false)
            }
            TuiCommand::ExportRun { run_ref, path } => {
                let sessions = self.store.list_sessions().await?;
                let session_id = resolve_run_reference(&sessions, &run_ref)?;
                let events = self.store.events(&session_id).await?;
                let output_path = self
                    .write_output_file(&path, encode_run_events_jsonl(&events)?)
                    .await?;
                state.sidebar = vec![
                    format!("exported session: {}", session_id),
                    format!("path: {}", output_path.display()),
                    format!("events: {}", events.len()),
                ];
                state.sidebar_title = "Export".to_string();
                state.status = format!(
                    "Exported session {} to {}",
                    preview_id(session_id.as_str()),
                    output_path.display()
                );
                Ok(false)
            }
            TuiCommand::ExportTranscript { run_ref, path } => {
                let sessions = self.store.list_sessions().await?;
                let session_id = resolve_run_reference(&sessions, &run_ref)?;
                let transcript = self.replay_run_lines(&session_id).await?;
                let content = if transcript.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", transcript.join("\n\n"))
                };
                let output_path = self.write_output_file(&path, content).await?;
                state.sidebar = vec![
                    format!("exported transcript: {}", session_id),
                    format!("path: {}", output_path.display()),
                    format!("lines: {}", transcript.len()),
                ];
                state.sidebar_title = "Export".to_string();
                state.status = format!(
                    "Exported transcript {} to {}",
                    preview_id(session_id.as_str()),
                    output_path.display()
                );
                Ok(false)
            }
            _ => unreachable!("sessions handler received non-session command"),
        }
    }
}
