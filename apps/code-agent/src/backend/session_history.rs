use agent::types::{
    AgentSessionId, Message, MessagePart, MessageRole, SessionEventEnvelope, SessionId,
};
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{SessionSearchResult, SessionStore, SessionSummary, SessionTokenUsageReport};

#[derive(Clone, Debug)]
pub(crate) struct LoadedSession {
    pub(crate) summary: SessionSummary,
    pub(crate) agent_session_ids: Vec<AgentSessionId>,
    pub(crate) transcript: Vec<Message>,
    pub(crate) events: Vec<SessionEventEnvelope>,
    pub(crate) token_usage: SessionTokenUsageReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionExportKind {
    EventsJsonl,
    TranscriptText,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionExportArtifact {
    pub(crate) kind: SessionExportKind,
    pub(crate) session_id: SessionId,
    pub(crate) output_path: PathBuf,
    pub(crate) item_count: usize,
}

pub(crate) async fn list_sessions(store: &Arc<dyn SessionStore>) -> Result<Vec<SessionSummary>> {
    Ok(store.list_sessions().await?)
}

pub(crate) async fn search_sessions(
    store: &Arc<dyn SessionStore>,
    query: &str,
) -> Result<Vec<SessionSearchResult>> {
    Ok(store.search_sessions(query).await?)
}

pub(crate) async fn load_session(
    store: &Arc<dyn SessionStore>,
    session_ref: &str,
) -> Result<LoadedSession> {
    let (session_id, summary) = resolve_session(store, session_ref).await?;
    let (events, agent_session_ids, transcript, token_usage) = tokio::try_join!(
        store.events(&session_id),
        store.agent_session_ids(&session_id),
        store.replay_transcript(&session_id),
        store.token_usage(&session_id),
    )?;
    Ok(LoadedSession {
        summary,
        agent_session_ids,
        transcript,
        events,
        token_usage,
    })
}

pub(crate) async fn export_session_events(
    store: &Arc<dyn SessionStore>,
    workspace_root: &Path,
    session_ref: &str,
    relative_or_absolute: &str,
) -> Result<SessionExportArtifact> {
    let (session_id, _) = resolve_session(store, session_ref).await?;
    let events = store.events(&session_id).await?;
    let output_path = write_output_file(
        workspace_root,
        relative_or_absolute,
        encode_session_events_jsonl(&events)?,
    )
    .await?;
    Ok(SessionExportArtifact {
        kind: SessionExportKind::EventsJsonl,
        session_id,
        output_path,
        item_count: events.len(),
    })
}

pub(crate) async fn export_session_transcript(
    store: &Arc<dyn SessionStore>,
    workspace_root: &Path,
    session_ref: &str,
    relative_or_absolute: &str,
) -> Result<SessionExportArtifact> {
    let (session_id, _) = resolve_session(store, session_ref).await?;
    let transcript = store.replay_transcript(&session_id).await?;
    let output_path = write_output_file(
        workspace_root,
        relative_or_absolute,
        render_transcript_text(&transcript),
    )
    .await?;
    Ok(SessionExportArtifact {
        kind: SessionExportKind::TranscriptText,
        session_id,
        output_path,
        item_count: transcript.len(),
    })
}

async fn resolve_session(
    store: &Arc<dyn SessionStore>,
    session_ref: &str,
) -> Result<(SessionId, SessionSummary)> {
    let sessions = list_sessions(store).await?;
    let session_id = resolve_session_reference(&sessions, session_ref)?;
    let summary = sessions
        .into_iter()
        .find(|summary| summary.session_id == session_id)
        .ok_or_else(|| anyhow!("session missing from store listing: {}", session_id))?;
    Ok((session_id, summary))
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

pub(crate) fn resolve_session_reference(
    sessions: &[SessionSummary],
    session_ref: &str,
) -> Result<SessionId> {
    if let Some(session) = sessions
        .iter()
        .find(|summary| summary.session_id.as_str() == session_ref)
    {
        return Ok(session.session_id.clone());
    }

    let matches = sessions
        .iter()
        .filter(|summary| summary.session_id.as_str().starts_with(session_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow!("unknown session id or prefix: {session_ref}")),
        [session] => Ok(session.session_id.clone()),
        _ => Err(anyhow!(
            "ambiguous session prefix {session_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|session| preview_id(session.session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub(crate) fn encode_session_events_jsonl(events: &[SessionEventEnvelope]) -> Result<String> {
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
    use super::{encode_session_events_jsonl, render_transcript_text, resolve_session_reference};
    use agent::types::{
        AgentSessionId, Message, MessagePart, MessageRole, SessionEventEnvelope, SessionEventKind,
        SessionId,
    };
    use store::SessionSummary;

    #[test]
    fn resolves_unique_session_prefix() {
        let sessions = vec![
            SessionSummary {
                session_id: SessionId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("first".to_string()),
            },
            SessionSummary {
                session_id: SessionId::from("def67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("second".to_string()),
            },
        ];

        assert_eq!(
            resolve_session_reference(&sessions, "abc").unwrap(),
            SessionId::from("abc12345")
        );
    }

    #[test]
    fn rejects_ambiguous_session_prefix() {
        let sessions = vec![
            SessionSummary {
                session_id: SessionId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
            },
            SessionSummary {
                session_id: SessionId::from("abc67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
            },
        ];

        assert!(resolve_session_reference(&sessions, "abc").is_err());
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
        let events = vec![SessionEventEnvelope::new(
            SessionId::from("session-1"),
            AgentSessionId::from("session-1"),
            None,
            None,
            SessionEventKind::SessionStart { reason: None },
        )];

        let encoded = encode_session_events_jsonl(&events).unwrap();
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("\"kind\":\"session_start\""));
    }
}
