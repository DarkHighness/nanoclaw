use crate::app_config::{CliOverrides, SchedClawConfig};
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::runtime::{RuntimeSession, load_runtime_session};
use agent::{
    AgentSessionId, FileSessionStore, Message, SessionId, SessionStore, message_operator_text,
};
use store::{SessionSearchResult, SessionSummary, SessionTokenUsageReport, visible_transcript};

#[derive(Clone)]
pub struct SessionHistory {
    store: Arc<dyn SessionStore>,
}

#[derive(Clone, Debug)]
pub struct LoadedSessionDetail {
    pub summary: SessionSummary,
    pub agent_session_ids: Vec<AgentSessionId>,
    pub transcript: Vec<Message>,
    pub token_usage: SessionTokenUsageReport,
}

#[derive(Clone, Debug)]
pub struct SessionExportArtifact {
    pub kind: SessionExportKind,
    pub session_id: SessionId,
    pub output_path: PathBuf,
    pub item_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionExportKind {
    EventsJsonl,
    TranscriptText,
}

impl SessionHistory {
    pub async fn open(workspace_root: &Path, overrides: &CliOverrides) -> Result<Self> {
        let config = SchedClawConfig::load_from_dir(workspace_root, overrides)?;
        let store_dir = config.core.resolved_store_dir(workspace_root);
        let store = FileSessionStore::open(&store_dir)
            .await
            .with_context(|| format!("failed to open session store at {}", store_dir.display()))?;
        Ok(Self {
            store: Arc::new(store),
        })
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        Ok(self.store.list_sessions().await?)
    }

    pub async fn search_sessions(&self, query: &str) -> Result<Vec<SessionSearchResult>> {
        Ok(self.store.search_sessions(query).await?)
    }

    pub async fn load_session(&self, session_ref: &str) -> Result<LoadedSessionDetail> {
        let (session_id, summary) = self.resolve_session(session_ref).await?;
        let (events, agent_session_ids, token_usage) = tokio::try_join!(
            self.store.events(&session_id),
            self.store.agent_session_ids(&session_id),
            self.store.token_usage(&session_id),
        )?;
        let transcript = visible_transcript(&events);
        Ok(LoadedSessionDetail {
            summary: SessionSummary {
                transcript_message_count: transcript.len(),
                ..summary
            },
            agent_session_ids,
            transcript,
            token_usage,
        })
    }

    pub async fn load_resumable_session(
        &self,
        session_ref: &str,
    ) -> Result<(SessionSummary, RuntimeSession)> {
        let detail = self.load_session(session_ref).await?;
        let agent_session_id = detail.agent_session_ids.last().cloned().ok_or_else(|| {
            anyhow!(
                "session {} has no persisted agent sessions",
                detail.summary.session_id
            )
        })?;
        let mut runtime = load_runtime_session(
            self.store.as_ref(),
            &detail.summary.session_id,
            &agent_session_id,
        )
        .await?;
        // Resume must fork a fresh live agent-session identity so new turns do
        // not overwrite the archived attachment that seeded the runtime window.
        runtime.agent_session_id = AgentSessionId::new();
        Ok((detail.summary, runtime))
    }

    pub async fn export_transcript(
        &self,
        workspace_root: &Path,
        session_ref: &str,
        output_path: &str,
    ) -> Result<SessionExportArtifact> {
        let detail = self.load_session(session_ref).await?;
        let path = write_output_file(
            workspace_root,
            output_path,
            render_transcript_text(&detail.transcript),
        )
        .await?;
        Ok(SessionExportArtifact {
            kind: SessionExportKind::TranscriptText,
            session_id: detail.summary.session_id,
            output_path: path,
            item_count: detail.transcript.len(),
        })
    }

    pub async fn export_events(
        &self,
        workspace_root: &Path,
        session_ref: &str,
        output_path: &str,
    ) -> Result<SessionExportArtifact> {
        let (session_id, _) = self.resolve_session(session_ref).await?;
        let events = self.store.events(&session_id).await?;
        let path =
            write_output_file(workspace_root, output_path, encode_events_jsonl(&events)?).await?;
        Ok(SessionExportArtifact {
            kind: SessionExportKind::EventsJsonl,
            session_id,
            output_path: path,
            item_count: events.len(),
        })
    }

    async fn resolve_session(&self, session_ref: &str) -> Result<(SessionId, SessionSummary)> {
        let summaries = self.list_sessions().await?;
        let session_id = resolve_session_reference(&summaries, session_ref)?;
        let summary = summaries
            .into_iter()
            .find(|summary| summary.session_id == session_id)
            .with_context(|| format!("missing session summary for {}", session_id))?;
        Ok((session_id, summary))
    }
}

pub fn resolve_session_reference(
    summaries: &[SessionSummary],
    session_ref: &str,
) -> Result<SessionId> {
    let normalized = session_ref.trim();
    if normalized.is_empty() {
        anyhow::bail!("session reference cannot be empty");
    }
    if normalized == "last" {
        return summaries
            .first()
            .map(|summary| summary.session_id.clone())
            .ok_or_else(|| anyhow!("no persisted sessions found"));
    }
    if let Some(summary) = summaries
        .iter()
        .find(|summary| summary.session_id.as_str() == normalized)
    {
        return Ok(summary.session_id.clone());
    }
    let matches = summaries
        .iter()
        .filter(|summary| summary.session_id.as_str().starts_with(normalized))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow!("unknown session id or prefix: {normalized}")),
        [summary] => Ok(summary.session_id.clone()),
        _ => Err(anyhow!(
            "ambiguous session prefix {normalized}: {}",
            matches
                .iter()
                .take(6)
                .map(|summary| preview_id(summary.session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn render_transcript_text(transcript: &[Message]) -> String {
    let lines = transcript.iter().map(message_to_text).collect::<Vec<_>>();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n\n"))
    }
}

pub fn message_to_text(message: &Message) -> String {
    let role = match message.role {
        agent::MessageRole::System => "system",
        agent::MessageRole::User => "user",
        agent::MessageRole::Assistant => "assistant",
        agent::MessageRole::Tool => "tool",
    };
    format!("{role}> {}", message_operator_text(message))
}

pub fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
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

fn encode_events_jsonl(events: &[agent::SessionEventEnvelope]) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::{message_to_text, preview_id, render_transcript_text, resolve_session_reference};
    use agent::{
        AgentSessionId, InMemorySessionStore, Message, SessionEventEnvelope, SessionEventKind,
        SessionId,
    };
    use std::sync::Arc;
    use store::{EventSink, SessionSummary};

    fn summary(id: &str) -> SessionSummary {
        SessionSummary {
            session_id: SessionId::from(id),
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            event_count: 3,
            agent_session_count: 1,
            transcript_message_count: 2,
            last_user_prompt: Some("hello".to_string()),
            token_usage: None,
        }
    }

    #[test]
    fn resolves_last_session() {
        let summaries = vec![summary("session_abc123"), summary("session_def456")];
        assert_eq!(
            resolve_session_reference(&summaries, "last")
                .unwrap()
                .as_str(),
            "session_abc123"
        );
    }

    #[test]
    fn rejects_ambiguous_prefix() {
        let summaries = vec![summary("session_abc123"), summary("session_abc456")];
        assert!(resolve_session_reference(&summaries, "session_abc").is_err());
    }

    #[test]
    fn renders_transcript_with_roles() {
        let transcript = vec![Message::user("inspect"), Message::assistant("done")];
        let rendered = render_transcript_text(&transcript);
        assert!(rendered.contains("user> inspect"));
        assert!(rendered.contains("assistant> done"));
        assert_eq!(message_to_text(&Message::assistant("ok")), "assistant> ok");
        assert_eq!(preview_id("session_abcdef"), "session_");
    }

    #[tokio::test]
    async fn resumable_session_forks_a_fresh_agent_session_id() {
        let session_id = SessionId::new();
        let persisted_agent_session_id = AgentSessionId::new();
        let store = InMemorySessionStore::new();
        store
            .append(SessionEventEnvelope::new(
                session_id.clone(),
                persisted_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("resume me"),
                },
            ))
            .await
            .unwrap();

        let history = super::SessionHistory {
            store: Arc::new(store),
        };
        let (_, runtime_session) = history
            .load_resumable_session(session_id.as_str())
            .await
            .unwrap();

        assert_eq!(runtime_session.session_id, session_id);
        assert_ne!(runtime_session.agent_session_id, persisted_agent_session_id);
    }
}
