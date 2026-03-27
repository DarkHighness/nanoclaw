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
                    let runs = self.store.search_runs(&query).await?;
                    state.sidebar = if runs.is_empty() {
                        vec![format!("no runs matched `{query}`")]
                    } else {
                        runs.iter().take(12).map(format_run_search_line).collect()
                    };
                    state.sidebar_title = "Run Search".to_string();
                    state.status = if runs.is_empty() {
                        format!("No runs matched `{query}`")
                    } else {
                        format!(
                            "Found {} matching runs. Use {}run <id-prefix> to replay one.",
                            runs.len(),
                            self.command_prefix
                        )
                    };
                } else {
                    let runs = self.store.list_runs().await?;
                    state.sidebar = if runs.is_empty() {
                        vec!["no runs recorded yet".to_string()]
                    } else {
                        runs.iter().take(12).map(format_run_summary_line).collect()
                    };
                    state.sidebar_title = "Runs".to_string();
                    state.status = if runs.is_empty() {
                        "No runs available yet".to_string()
                    } else {
                        format!(
                            "Listed {} runs. Use {}run <id-prefix> to replay one.",
                            runs.len(),
                            self.command_prefix
                        )
                    };
                }
                Ok(false)
            }
            TuiCommand::Run { run_ref } => {
                let runs = self.store.list_runs().await?;
                let run_id = resolve_run_reference(&runs, &run_ref)?;
                let summary = runs
                    .iter()
                    .find(|summary| summary.run_id == run_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("run missing from store listing: {}", run_id))?;
                let events = self.store.events(&run_id).await?;
                let session_ids = self.store.session_ids(&run_id).await?;
                state.transcript = self.replay_run_lines(&run_id).await?;
                state.sidebar = format_run_sidebar(&summary, &session_ids, &events);
                state.sidebar_title = "Run".to_string();
                state.status = format!(
                    "Loaded run {} with {} transcript messages",
                    preview_id(run_id.as_str()),
                    summary.transcript_message_count
                );
                Ok(false)
            }
            TuiCommand::ExportRun { run_ref, path } => {
                let runs = self.store.list_runs().await?;
                let run_id = resolve_run_reference(&runs, &run_ref)?;
                let events = self.store.events(&run_id).await?;
                let output_path = self
                    .write_output_file(&path, encode_run_events_jsonl(&events)?)
                    .await?;
                state.sidebar = vec![
                    format!("exported run: {}", run_id),
                    format!("path: {}", output_path.display()),
                    format!("events: {}", events.len()),
                ];
                state.sidebar_title = "Export".to_string();
                state.status = format!(
                    "Exported run {} to {}",
                    preview_id(run_id.as_str()),
                    output_path.display()
                );
                Ok(false)
            }
            TuiCommand::ExportTranscript { run_ref, path } => {
                let runs = self.store.list_runs().await?;
                let run_id = resolve_run_reference(&runs, &run_ref)?;
                let transcript = self.replay_run_lines(&run_id).await?;
                let content = if transcript.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", transcript.join("\n\n"))
                };
                let output_path = self.write_output_file(&path, content).await?;
                state.sidebar = vec![
                    format!("exported transcript: {}", run_id),
                    format!("path: {}", output_path.display()),
                    format!("lines: {}", transcript.len()),
                ];
                state.sidebar_title = "Export".to_string();
                state.status = format!(
                    "Exported transcript {} to {}",
                    preview_id(run_id.as_str()),
                    output_path.display()
                );
                Ok(false)
            }
            _ => unreachable!("runs handler received non-run command"),
        }
    }
}
