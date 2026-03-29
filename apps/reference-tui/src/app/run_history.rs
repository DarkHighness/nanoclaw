use super::{RuntimeTui, message_to_text, preview_id};
use std::path::{Path, PathBuf};
use store::RunSummary;
use types::{RunEventEnvelope, RunId};

impl RuntimeTui {
    pub(super) async fn replay_run_lines(&self, run_id: &RunId) -> anyhow::Result<Vec<String>> {
        Ok(self
            .store
            .replay_transcript(run_id)
            .await?
            .iter()
            .map(message_to_text)
            .collect())
    }

    pub(super) async fn write_output_file(
        &self,
        relative_or_absolute: &str,
        content: String,
    ) -> anyhow::Result<PathBuf> {
        let path = resolve_output_path(&self.workspace_root, relative_or_absolute);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(path)
    }
}

pub(super) fn resolve_run_reference(runs: &[RunSummary], run_ref: &str) -> anyhow::Result<RunId> {
    if let Some(run) = runs
        .iter()
        .find(|summary| summary.run_id.as_str() == run_ref)
    {
        return Ok(run.run_id.clone());
    }

    let matches = runs
        .iter()
        .filter(|summary| summary.run_id.as_str().starts_with(run_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!("unknown run id or prefix: {run_ref}")),
        [run] => Ok(run.run_id.clone()),
        _ => Err(anyhow::anyhow!(
            "ambiguous run prefix {run_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|run| preview_id(run.run_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub(super) fn encode_run_events_jsonl(events: &[RunEventEnvelope]) -> anyhow::Result<String> {
    let mut lines = Vec::with_capacity(events.len());
    for event in events {
        lines.push(serde_json::to_string(event)?);
    }
    Ok(if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    })
}

fn resolve_output_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_run_reference;
    use store::RunSummary;
    use types::RunId;

    #[test]
    fn resolves_unique_run_prefix() {
        let runs = vec![
            RunSummary {
                run_id: RunId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("first".to_string()),
            },
            RunSummary {
                run_id: RunId::from("def67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("second".to_string()),
            },
        ];

        assert_eq!(
            resolve_run_reference(&runs, "abc").unwrap(),
            RunId::from("abc12345")
        );
    }

    #[test]
    fn rejects_ambiguous_run_prefix() {
        let runs = vec![
            RunSummary {
                run_id: RunId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
            },
            RunSummary {
                run_id: RunId::from("abc67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
            },
        ];

        assert!(resolve_run_reference(&runs, "abc").is_err());
    }
}
