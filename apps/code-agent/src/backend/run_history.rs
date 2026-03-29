use agent::types::{AgentSessionId, Message, MessagePart, MessageRole, RunEventEnvelope, RunId};
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{RunSearchResult, RunStore, RunSummary, RunTokenUsageReport};

#[derive(Clone, Debug)]
pub(crate) struct LoadedRun {
    pub(crate) summary: RunSummary,
    pub(crate) agent_session_ids: Vec<AgentSessionId>,
    pub(crate) transcript: Vec<Message>,
    pub(crate) events: Vec<RunEventEnvelope>,
    pub(crate) token_usage: RunTokenUsageReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunExportKind {
    EventsJsonl,
    TranscriptText,
}

#[derive(Clone, Debug)]
pub(crate) struct RunExportArtifact {
    pub(crate) kind: RunExportKind,
    pub(crate) run_id: RunId,
    pub(crate) output_path: PathBuf,
    pub(crate) item_count: usize,
}

pub(crate) async fn list_runs(store: &Arc<dyn RunStore>) -> Result<Vec<RunSummary>> {
    Ok(store.list_runs().await?)
}

pub(crate) async fn search_runs(
    store: &Arc<dyn RunStore>,
    query: &str,
) -> Result<Vec<RunSearchResult>> {
    Ok(store.search_runs(query).await?)
}

pub(crate) async fn load_run(store: &Arc<dyn RunStore>, run_ref: &str) -> Result<LoadedRun> {
    let (run_id, summary) = resolve_run(store, run_ref).await?;
    let (events, agent_session_ids, transcript, token_usage) = tokio::try_join!(
        store.events(&run_id),
        store.agent_session_ids(&run_id),
        store.replay_transcript(&run_id),
        store.token_usage(&run_id),
    )?;
    Ok(LoadedRun {
        summary,
        agent_session_ids,
        transcript,
        events,
        token_usage,
    })
}

pub(crate) async fn export_run_events(
    store: &Arc<dyn RunStore>,
    workspace_root: &Path,
    run_ref: &str,
    relative_or_absolute: &str,
) -> Result<RunExportArtifact> {
    let (run_id, _) = resolve_run(store, run_ref).await?;
    let events = store.events(&run_id).await?;
    let output_path = write_output_file(
        workspace_root,
        relative_or_absolute,
        encode_run_events_jsonl(&events)?,
    )
    .await?;
    Ok(RunExportArtifact {
        kind: RunExportKind::EventsJsonl,
        run_id,
        output_path,
        item_count: events.len(),
    })
}

pub(crate) async fn export_run_transcript(
    store: &Arc<dyn RunStore>,
    workspace_root: &Path,
    run_ref: &str,
    relative_or_absolute: &str,
) -> Result<RunExportArtifact> {
    let (run_id, _) = resolve_run(store, run_ref).await?;
    let transcript = store.replay_transcript(&run_id).await?;
    let output_path = write_output_file(
        workspace_root,
        relative_or_absolute,
        render_transcript_text(&transcript),
    )
    .await?;
    Ok(RunExportArtifact {
        kind: RunExportKind::TranscriptText,
        run_id,
        output_path,
        item_count: transcript.len(),
    })
}

async fn resolve_run(store: &Arc<dyn RunStore>, run_ref: &str) -> Result<(RunId, RunSummary)> {
    let runs = list_runs(store).await?;
    let run_id = resolve_run_reference(&runs, run_ref)?;
    let summary = runs
        .into_iter()
        .find(|summary| summary.run_id == run_id)
        .ok_or_else(|| anyhow!("run missing from store listing: {}", run_id))?;
    Ok((run_id, summary))
}

fn write_output_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

async fn write_output_file(
    workspace_root: &Path,
    relative_or_absolute: &str,
    content: String,
) -> Result<PathBuf> {
    let path = write_output_path(workspace_root, relative_or_absolute);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, content).await?;
    Ok(path)
}

pub(crate) fn resolve_run_reference(runs: &[RunSummary], run_ref: &str) -> Result<RunId> {
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
        [] => Err(anyhow!("unknown run id or prefix: {run_ref}")),
        [run] => Ok(run.run_id.clone()),
        _ => Err(anyhow!(
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

pub(crate) fn encode_run_events_jsonl(events: &[RunEventEnvelope]) -> Result<String> {
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

pub(crate) fn render_transcript_text(transcript: &[Message]) -> String {
    let lines = transcript.iter().map(message_to_text).collect::<Vec<_>>();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n\n"))
    }
}

pub(crate) fn message_to_text(message: &Message) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!(
        "{role}> {}",
        message
            .parts
            .iter()
            .map(message_part_to_text)
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn message_part_to_text(part: &MessagePart) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Image { mime_type, .. } => format!("[image:{mime_type}]"),
        MessagePart::File {
            file_name,
            mime_type,
            uri,
            ..
        } => format!(
            "[file:{}{}{}]",
            file_name.clone().unwrap_or_else(|| "unnamed".to_string()),
            mime_type
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
            uri.as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
        ),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            if text.is_empty() {
                "[reasoning]".to_string()
            } else {
                format!("[reasoning] {text}")
            }
        }
        MessagePart::ToolCall { call } => format!("[tool_call:{}]", call.tool_name),
        MessagePart::ToolResult { result } => {
            format!(
                "[tool_result:{}] {}",
                result.tool_name,
                result.text_content()
            )
        }
        MessagePart::Resource {
            uri,
            mime_type,
            text,
            ..
        } => format!(
            "[resource:{}{}{}]",
            uri,
            mime_type
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
            text.as_deref()
                .map(|value: &str| format!(" {}", value.replace('\n', " ")))
                .unwrap_or_default(),
        ),
        MessagePart::Json { value } => format!("[json] {value}"),
        MessagePart::ProviderExtension { provider, kind, .. } => {
            format!("[provider_extension:{provider}:{kind}]")
        }
    }
}

pub(crate) fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::{encode_run_events_jsonl, render_transcript_text, resolve_run_reference};
    use agent::types::{
        AgentSessionId, Message, MessagePart, MessageRole, RunEventEnvelope, RunEventKind, RunId,
    };
    use store::RunSummary;

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

    #[test]
    fn transcript_export_joins_messages_with_blank_lines() {
        let transcript = vec![
            Message::new(
                MessageRole::User,
                vec![MessagePart::Text {
                    text: "first".to_string(),
                }],
            ),
            Message::new(
                MessageRole::Assistant,
                vec![MessagePart::Text {
                    text: "second".to_string(),
                }],
            ),
        ];

        assert_eq!(
            render_transcript_text(&transcript),
            "user> first\n\nassistant> second\n"
        );
    }

    #[test]
    fn event_export_writes_jsonl_lines() {
        let events = vec![RunEventEnvelope::new(
            RunId::from("run-1"),
            AgentSessionId::from("session-1"),
            None,
            None,
            RunEventKind::SessionStart { reason: None },
        )];

        let encoded = encode_run_events_jsonl(&events).unwrap();
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("\"kind\":\"session_start\""));
    }
}
