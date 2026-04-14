use crate::backend::session_catalog;
use crate::backend::session_history::{self, preview_id};
use crate::backend::session_memory_compaction::session_memory_note_absolute_path;
use crate::backend::session_memory_note::session_memory_note_title;
use crate::backend::task_history;
use crate::ui::{
    LoadedAgentSession, LoadedSession, LoadedTask, PersistedAgentSessionSummary,
    PersistedSessionSearchMatch, PersistedSessionSummary, PersistedTaskSummary,
    SessionExportArtifact,
};
use agent::types::{SessionEventEnvelope, SessionEventKind, SessionId};
use anyhow::{Context, Result, anyhow};
use futures::{StreamExt, stream};
use nanoclaw_config::CoreConfig;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{SessionStore, SessionSummary};
use tokio::fs;
use tracing::warn;

// Session-note titles are host-owned files, so read them alongside the store
// catalog with bounded fan-out instead of widening the store schema for a
// frontend-friendly cue.
const SESSION_NOTE_TITLE_LOAD_CONCURRENCY_LIMIT: usize = 8;
const SESSION_ARCHIVE_FORMAT: &str = "nanoclaw.session-archive";
const SESSION_ARCHIVE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionArchiveArtifact {
    pub root_session_id: SessionId,
    pub output_path: PathBuf,
    pub session_count: usize,
    pub event_count: usize,
    pub session_note_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionImportArtifact {
    pub root_session_id: SessionId,
    pub input_path: PathBuf,
    pub session_count: usize,
    pub event_count: usize,
    pub session_note_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionArchiveDocument {
    format: String,
    version: u32,
    root_session_id: SessionId,
    sessions: Vec<SessionArchiveSession>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionArchiveSession {
    session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_note: Option<String>,
    #[serde(default)]
    events: Vec<SessionEventEnvelope>,
}

pub struct SessionHistoryClient {
    store: Arc<dyn SessionStore>,
    workspace_root: PathBuf,
    store_label: String,
    store_warning: Option<String>,
}

impl SessionHistoryClient {
    pub async fn open(core: &CoreConfig, workspace_root: &Path) -> Result<Self> {
        let handle = super::store::build_store(core, workspace_root).await?;
        Ok(Self {
            store: handle.store,
            workspace_root: workspace_root.to_path_buf(),
            store_label: handle.label,
            store_warning: handle.warning,
        })
    }

    pub fn store_label(&self) -> &str {
        &self.store_label
    }

    pub fn store_warning(&self) -> Option<&str> {
        self.store_warning.as_deref()
    }

    pub async fn list_sessions(&self) -> Result<Vec<PersistedSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        Ok(sessions
            .iter()
            .map(|summary| {
                session_catalog::persisted_session_summary(
                    summary,
                    "",
                    session_titles.get(&summary.session_id).cloned(),
                )
            })
            .collect())
    }

    pub async fn search_sessions(&self, query: &str) -> Result<Vec<PersistedSessionSearchMatch>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let matches = session_history::search_sessions(&self.store, query).await?;
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let mut seen_session_refs = BTreeSet::new();
        let mut title_matches = Vec::new();
        let mut other_matches = Vec::new();

        for result in matches {
            let session_title = session_titles.get(&result.summary.session_id).cloned();
            let mut persisted =
                session_catalog::persisted_session_search_match(&result, "", session_title.clone());
            let matched_title =
                prepend_session_title_preview(&mut persisted, session_title.as_deref(), query);
            seen_session_refs.insert(persisted.summary.session_ref.clone());
            if matched_title {
                title_matches.push(persisted);
            } else {
                other_matches.push(persisted);
            }
        }

        let title_only_matches = sessions
            .iter()
            .filter_map(|summary| {
                let session_title = session_titles.get(&summary.session_id)?.clone();
                if !session_title_matches_query(Some(&session_title), query)
                    || seen_session_refs.contains(summary.session_id.as_str())
                {
                    return None;
                }
                Some(PersistedSessionSearchMatch {
                    summary: session_catalog::persisted_session_summary(
                        summary,
                        "",
                        Some(session_title.clone()),
                    ),
                    matched_event_count: 0,
                    preview_matches: vec![session_title_preview(&session_title)],
                })
            })
            .collect::<Vec<_>>();

        title_matches.extend(title_only_matches);
        title_matches.extend(other_matches);
        Ok(title_matches)
    }

    pub async fn resolve_session_ref(&self, session_ref: &str) -> Result<String> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let session_id =
            resolve_session_reference_from_catalog(&sessions, &session_titles, session_ref)?;
        Ok(session_id.to_string())
    }

    pub async fn resolve_last_session_ref(&self) -> Result<String> {
        session_history::list_sessions(&self.store)
            .await?
            .into_iter()
            .next()
            .map(|summary| summary.session_id.to_string())
            .context("no persisted sessions available")
    }

    pub async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        let resolved_ref = self.resolve_session_ref(session_ref).await?;
        session_history::load_session(&self.store, &resolved_ref).await
    }

    pub async fn list_agent_sessions(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<PersistedAgentSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let filtered_session_id = session_ref
            .map(|session_ref| {
                resolve_session_reference_from_catalog(&sessions, &session_titles, session_ref)
            })
            .transpose()?;
        let mut agent_sessions = Vec::new();
        for summary in sessions.into_iter().filter(|summary| {
            filtered_session_id
                .as_ref()
                .is_none_or(|session_id| summary.session_id == *session_id)
        }) {
            let events = self.store.events(&summary.session_id).await?;
            agent_sessions.extend(session_catalog::persisted_agent_session_summaries(
                summary.session_id.as_str(),
                session_titles.get(&summary.session_id).map(String::as_str),
                &events,
                "",
            ));
        }
        agent_sessions.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.agent_session_ref.cmp(&right.agent_session_ref))
        });
        Ok(agent_sessions)
    }

    pub async fn load_agent_session(&self, agent_session_ref: &str) -> Result<LoadedAgentSession> {
        let summary = self.resolve_agent_session_ref(agent_session_ref).await?;
        session_history::load_agent_session(&self.store, summary).await
    }

    pub async fn list_tasks(&self, session_ref: Option<&str>) -> Result<Vec<PersistedTaskSummary>> {
        let resolved_session_ref = if let Some(session_ref) = session_ref {
            Some(self.resolve_session_ref(session_ref).await?)
        } else {
            None
        };
        task_history::list_tasks(&self.store, resolved_session_ref.as_deref()).await
    }

    pub async fn load_task(&self, task_ref: &str) -> Result<LoadedTask> {
        let tasks = self.list_tasks(None).await?;
        let summary = task_history::resolve_task_reference(&tasks, task_ref)?.clone();
        task_history::load_task(&self.store, summary).await
    }

    pub async fn export_session(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        let resolved_ref = self.resolve_session_ref(session_ref).await?;
        session_history::export_session_events(
            &self.store,
            &self.workspace_root,
            &resolved_ref,
            relative_or_absolute,
        )
        .await
    }

    pub async fn export_session_transcript(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        let resolved_ref = self.resolve_session_ref(session_ref).await?;
        session_history::export_session_transcript(
            &self.store,
            &self.workspace_root,
            &resolved_ref,
            relative_or_absolute,
        )
        .await
    }

    pub async fn export_session_archive(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionArchiveArtifact> {
        let resolved_ref = self.resolve_session_ref(session_ref).await?;
        let archive = self
            .build_session_archive(&SessionId::from(resolved_ref.clone()))
            .await?;
        let output_path = resolve_history_path(&self.workspace_root, relative_or_absolute);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&output_path, serde_json::to_vec_pretty(&archive)?).await?;
        let (session_count, event_count, session_note_count) = archive_stats(&archive);
        Ok(SessionArchiveArtifact {
            root_session_id: archive.root_session_id,
            output_path,
            session_count,
            event_count,
            session_note_count,
        })
    }

    pub async fn import_session_archive(
        &self,
        relative_or_absolute: &str,
    ) -> Result<SessionImportArtifact> {
        let input_path = resolve_history_path(&self.workspace_root, relative_or_absolute);
        let archive: SessionArchiveDocument = serde_json::from_slice(
            &fs::read(&input_path)
                .await
                .with_context(|| format!("failed to read archive {}", input_path.display()))?,
        )
        .with_context(|| format!("failed to parse archive {}", input_path.display()))?;
        validate_archive(&archive)?;
        self.ensure_archive_sessions_absent(&archive).await?;

        let mut all_events = Vec::new();
        for session in &archive.sessions {
            all_events.extend(session.events.iter().cloned());
        }
        self.store.append_batch(all_events).await?;

        for session in &archive.sessions {
            if let Some(note) = session.session_note.as_deref() {
                self.write_session_note_text(&session.session_id, note)
                    .await?;
            }
        }

        let (session_count, event_count, session_note_count) = archive_stats(&archive);
        Ok(SessionImportArtifact {
            root_session_id: archive.root_session_id,
            input_path,
            session_count,
            event_count,
            session_note_count,
        })
    }

    async fn build_session_archive(
        &self,
        root_session_id: &SessionId,
    ) -> Result<SessionArchiveDocument> {
        let mut pending = VecDeque::from([root_session_id.clone()]);
        let mut visited = BTreeSet::new();
        let mut sessions = Vec::new();

        while let Some(session_id) = pending.pop_front() {
            if !visited.insert(session_id.clone()) {
                continue;
            }

            let events = self
                .store
                .events(&session_id)
                .await
                .with_context(|| format!("failed to load session events for {session_id}"))?;
            for child_session_id in referenced_child_session_ids(&events, &session_id) {
                if !visited.contains(&child_session_id) {
                    pending.push_back(child_session_id);
                }
            }
            sessions.push(SessionArchiveSession {
                session_note: self.load_session_note_text(&session_id).await?,
                session_id,
                events,
            });
        }

        Ok(SessionArchiveDocument {
            format: SESSION_ARCHIVE_FORMAT.to_string(),
            version: SESSION_ARCHIVE_VERSION,
            root_session_id: root_session_id.clone(),
            sessions,
        })
    }

    async fn load_session_note_titles<I>(&self, session_ids: I) -> BTreeMap<SessionId, String>
    where
        I: IntoIterator<Item = SessionId>,
    {
        let workspace_root = self.workspace_root.clone();
        stream::iter(session_ids.into_iter().map(|session_id| {
            let workspace_root = workspace_root.clone();
            async move {
                let path = session_memory_note_absolute_path(&workspace_root, &session_id);
                let text = match fs::read_to_string(path).await {
                    Ok(text) => text,
                    Err(error) if error.kind() == ErrorKind::NotFound => return None,
                    Err(error) => {
                        warn!(
                            session_id = %session_id,
                            error = %error,
                            "failed to load session note title"
                        );
                        return None;
                    }
                };
                session_memory_note_title(&text).map(|title| (session_id, title))
            }
        }))
        .buffer_unordered(SESSION_NOTE_TITLE_LOAD_CONCURRENCY_LIMIT)
        .filter_map(async move |entry| entry)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
    }

    async fn load_session_note_text(&self, session_id: &SessionId) -> Result<Option<String>> {
        let path = session_memory_note_absolute_path(&self.workspace_root, session_id);
        match fs::read_to_string(&path).await {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error)
                .with_context(|| format!("failed to read session note {}", path.display())),
        }
    }

    async fn write_session_note_text(&self, session_id: &SessionId, text: &str) -> Result<()> {
        let path = session_memory_note_absolute_path(&self.workspace_root, session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, text)
            .await
            .with_context(|| format!("failed to write session note {}", path.display()))
    }

    async fn ensure_archive_sessions_absent(&self, archive: &SessionArchiveDocument) -> Result<()> {
        let existing = session_history::list_sessions(&self.store).await?;
        let duplicates = archive
            .sessions
            .iter()
            .filter(|session| {
                existing
                    .iter()
                    .any(|summary| summary.session_id == session.session_id)
            })
            .map(|session| preview_id(session.session_id.as_str()))
            .collect::<Vec<_>>();
        if duplicates.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(
                "archive import would overwrite existing sessions: {}",
                duplicates.join(", ")
            ))
        }
    }

    async fn resolve_agent_session_ref(
        &self,
        agent_session_ref: &str,
    ) -> Result<PersistedAgentSessionSummary> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        resolve_agent_session_reference_from_catalog(&agent_sessions, agent_session_ref)
    }
}

fn resolve_history_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn archive_stats(archive: &SessionArchiveDocument) -> (usize, usize, usize) {
    let session_count = archive.sessions.len();
    let event_count = archive
        .sessions
        .iter()
        .map(|session| session.events.len())
        .sum();
    let session_note_count = archive
        .sessions
        .iter()
        .filter(|session| session.session_note.is_some())
        .count();
    (session_count, event_count, session_note_count)
}

fn validate_archive(archive: &SessionArchiveDocument) -> Result<()> {
    if archive.format != SESSION_ARCHIVE_FORMAT {
        return Err(anyhow!(
            "unsupported archive format {}; expected {SESSION_ARCHIVE_FORMAT}",
            archive.format
        ));
    }
    if archive.version != SESSION_ARCHIVE_VERSION {
        return Err(anyhow!(
            "unsupported archive version {}; expected {SESSION_ARCHIVE_VERSION}",
            archive.version
        ));
    }
    if archive.sessions.is_empty() {
        return Err(anyhow!("archive does not contain any sessions"));
    }

    let mut seen = BTreeSet::new();
    let mut found_root = false;
    for session in &archive.sessions {
        if !seen.insert(session.session_id.clone()) {
            return Err(anyhow!(
                "archive contains duplicate session {}",
                session.session_id
            ));
        }
        if session.session_id == archive.root_session_id {
            found_root = true;
        }
        if session.events.is_empty() {
            return Err(anyhow!(
                "archive session {} does not contain any events",
                session.session_id
            ));
        }
        if let Some(mismatched) = session
            .events
            .iter()
            .find(|event| event.session_id != session.session_id)
        {
            return Err(anyhow!(
                "archive session {} contains mismatched event for {}",
                session.session_id,
                mismatched.session_id
            ));
        }
    }
    if !found_root {
        return Err(anyhow!(
            "archive root session {} is missing from the archive payload",
            archive.root_session_id
        ));
    }
    Ok(())
}

fn referenced_child_session_ids(
    events: &[SessionEventEnvelope],
    current_session_id: &SessionId,
) -> BTreeSet<SessionId> {
    let mut child_session_ids = BTreeSet::new();
    for event in events {
        match &event.event {
            SessionEventKind::SubagentStart { handle, .. }
            | SessionEventKind::SubagentStop { handle, .. } => {
                if handle.session_id != *current_session_id {
                    child_session_ids.insert(handle.session_id.clone());
                }
            }
            SessionEventKind::AgentEnvelope { envelope } => {
                if envelope.session_id != *current_session_id {
                    child_session_ids.insert(envelope.session_id.clone());
                }
            }
            _ => {}
        }
    }
    child_session_ids
}

fn resolve_session_reference_from_catalog(
    sessions: &[SessionSummary],
    session_titles: &BTreeMap<SessionId, String>,
    session_ref: &str,
) -> Result<SessionId> {
    if let Some(session) = sessions
        .iter()
        .find(|summary| summary.session_id.as_str() == session_ref)
    {
        return Ok(session.session_id.clone());
    }

    let prefix_matches = sessions
        .iter()
        .filter(|summary| summary.session_id.as_str().starts_with(session_ref))
        .collect::<Vec<_>>();
    match prefix_matches.as_slice() {
        [session] => return Ok(session.session_id.clone()),
        [] => {}
        _ => {
            return Err(anyhow!(
                "ambiguous session prefix {session_ref}: {}",
                prefix_matches
                    .iter()
                    .take(6)
                    .map(|session| preview_id(session.session_id.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let title_matches = sessions
        .iter()
        .filter_map(|summary| {
            let session_title = session_titles.get(&summary.session_id)?;
            session_title_matches_query(Some(session_title.as_str()), session_ref)
                .then_some((summary, session_title.as_str()))
        })
        .collect::<Vec<_>>();
    match title_matches.as_slice() {
        [] => Err(anyhow!(
            "unknown session id, prefix, or session title: {session_ref}"
        )),
        [(summary, _)] => Ok(summary.session_id.clone()),
        _ => Err(anyhow!(
            "ambiguous session title {session_ref}: {}",
            title_matches
                .iter()
                .take(6)
                .map(|(summary, title)| session_title_reference_preview(
                    summary.session_id.as_str(),
                    title
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn resolve_agent_session_reference_from_catalog(
    agent_sessions: &[PersistedAgentSessionSummary],
    agent_session_ref: &str,
) -> Result<PersistedAgentSessionSummary> {
    if let Some(summary) = agent_sessions
        .iter()
        .find(|summary| summary.agent_session_ref == agent_session_ref)
    {
        return Ok(summary.clone());
    }

    let prefix_matches = agent_sessions
        .iter()
        .filter(|summary| summary.agent_session_ref.starts_with(agent_session_ref))
        .collect::<Vec<_>>();
    match prefix_matches.as_slice() {
        [summary] => return Ok((*summary).clone()),
        [] => {}
        _ => {
            return Err(anyhow!(
                "ambiguous agent session prefix {agent_session_ref}: {}",
                prefix_matches
                    .iter()
                    .take(6)
                    .map(|summary| preview_id(summary.agent_session_ref.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let mut session_matches = BTreeMap::new();
    for summary in agent_sessions.iter().filter(|summary| {
        session_title_matches_query(summary.session_title.as_deref(), agent_session_ref)
    }) {
        session_matches
            .entry(summary.session_ref.clone())
            .or_insert_with(Vec::new)
            .push(summary);
    }
    match session_matches.len() {
        0 => Err(anyhow!(
            "unknown agent session id, prefix, or session title: {agent_session_ref}"
        )),
        1 => {
            let summaries = session_matches.into_values().next().unwrap();
            summaries
                .iter()
                .find(|summary| summary.label == "root")
                .or_else(|| (summaries.len() == 1).then_some(&summaries[0]))
                .map(|summary| (*summary).clone())
                .ok_or_else(|| {
                    anyhow!(
                        "ambiguous agent session title {agent_session_ref}: {}",
                        summaries
                            .iter()
                            .take(6)
                            .map(|summary| preview_id(summary.agent_session_ref.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
        }
        _ => Err(anyhow!(
            "ambiguous session title {agent_session_ref}: {}",
            session_matches
                .iter()
                .take(6)
                .map(|(session_ref, summaries)| {
                    let title = summaries
                        .first()
                        .and_then(|summary| summary.session_title.as_deref())
                        .unwrap_or("");
                    session_title_reference_preview(session_ref, title)
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn session_title_matches_query(session_title: Option<&str>, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return false;
    }
    session_title
        .map(str::to_lowercase)
        .is_some_and(|title| title.contains(&query))
}

fn session_title_preview(session_title: &str) -> String {
    format!("session title: {}", session_title.trim())
}

fn session_title_reference_preview(session_ref: &str, session_title: &str) -> String {
    format!(
        "{} ({})",
        preview_id(session_ref),
        session_title.trim().chars().take(32).collect::<String>()
    )
}

fn prepend_session_title_preview(
    result: &mut PersistedSessionSearchMatch,
    session_title: Option<&str>,
    query: &str,
) -> bool {
    if !session_title_matches_query(session_title, query) {
        return false;
    }
    let Some(session_title) = session_title else {
        return false;
    };
    let preview = session_title_preview(session_title);
    if !result.preview_matches.iter().any(|entry| entry == &preview) {
        result.preview_matches.insert(0, preview);
        result.preview_matches.truncate(3);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        SessionHistoryClient, prepend_session_title_preview, resolve_session_reference_from_catalog,
    };
    use crate::backend::session_memory_compaction::session_memory_note_absolute_path;
    use crate::ui::{PersistedSessionSearchMatch, ResumeSupport};
    use agent::types::{
        AgentHandle, AgentId, AgentSessionId, AgentStatus, AgentTaskSpec, Message,
        SessionEventEnvelope, SessionEventKind, SessionId, SessionSummaryTokenUsage,
        SubmittedPromptSnapshot, TaskId, TaskOrigin,
    };
    use nanoclaw_config::CoreConfig;
    use std::collections::BTreeMap;
    use store::{SessionSearchResult, SessionSummary};
    use tempfile::tempdir;

    fn session_summary(session_ref: &str) -> SessionSummary {
        SessionSummary {
            session_id: SessionId::from(session_ref),
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            event_count: 3,
            agent_session_count: 1,
            transcript_message_count: 4,
            last_user_prompt: Some("inspect".to_string()),
            token_usage: Some(SessionSummaryTokenUsage::default()),
        }
    }

    #[test]
    fn title_lookup_resolves_when_ids_do_not_match() {
        let sessions = vec![session_summary("session-a"), session_summary("session-b")];
        let titles = BTreeMap::from([
            (SessionId::from("session-a"), "Alpha workspace".to_string()),
            (SessionId::from("session-b"), "Beta workspace".to_string()),
        ]);

        let resolved =
            resolve_session_reference_from_catalog(&sessions, &titles, "beta workspace").unwrap();

        assert_eq!(resolved, SessionId::from("session-b"));
    }

    #[test]
    fn title_preview_is_promoted_to_the_front_once() {
        let mut result = PersistedSessionSearchMatch {
            summary: crate::ui::PersistedSessionSummary {
                session_ref: "session-a".to_string(),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                worker_session_count: 1,
                transcript_message_count: 4,
                session_title: Some("Alpha workspace".to_string()),
                last_user_prompt: Some("inspect".to_string()),
                token_usage: None,
                resume_support: ResumeSupport::NotYetSupported {
                    reason: "history".to_string(),
                },
            },
            matched_event_count: SessionSearchResult {
                summary: session_summary("session-a"),
                matched_event_count: 1,
                preview_matches: vec!["user> inspect".to_string()],
            }
            .matched_event_count,
            preview_matches: vec!["user> inspect".to_string()],
        };

        assert!(prepend_session_title_preview(
            &mut result,
            Some("Alpha workspace"),
            "workspace"
        ));
        assert_eq!(
            result.preview_matches,
            vec![
                "session title: Alpha workspace".to_string(),
                "user> inspect".to_string(),
            ]
        );

        assert!(prepend_session_title_preview(
            &mut result,
            Some("Alpha workspace"),
            "workspace"
        ));
        assert_eq!(result.preview_matches.len(), 2);
    }

    fn child_task(
        child_session_id: &SessionId,
        child_agent_session_id: &AgentSessionId,
    ) -> SessionEventEnvelope {
        SessionEventEnvelope::new(
            SessionId::from("session-root"),
            AgentSessionId::from("agent-root"),
            None,
            None,
            SessionEventKind::SubagentStart {
                handle: AgentHandle {
                    agent_id: AgentId::from("agent-child"),
                    parent_agent_id: Some(AgentId::from("agent-root")),
                    session_id: child_session_id.clone(),
                    agent_session_id: child_agent_session_id.clone(),
                    task_id: TaskId::from("task-child"),
                    role: "worker".to_string(),
                    status: AgentStatus::Running,
                    worktree_id: None,
                    worktree_root: None,
                },
                task: AgentTaskSpec {
                    task_id: TaskId::from("task-child"),
                    role: "worker".to_string(),
                    prompt: "inspect child".to_string(),
                    origin: TaskOrigin::ChildAgentBacked,
                    steer: None,
                    allowed_tools: Vec::new(),
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                },
            },
        )
    }

    async fn history_client(workspace_root: &std::path::Path) -> SessionHistoryClient {
        SessionHistoryClient::open(&CoreConfig::default(), workspace_root)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn archive_export_and_import_round_trip_root_and_child_sessions() {
        let export_dir = tempdir().unwrap();
        let export_client = history_client(export_dir.path()).await;
        let root_session_id = SessionId::from("session-root");
        let root_agent_session_id = AgentSessionId::from("agent-root");
        let child_session_id = SessionId::from("session-child");
        let child_agent_session_id = AgentSessionId::from("agent-child");
        export_client
            .store
            .append_batch(vec![
                SessionEventEnvelope::new(
                    root_session_id.clone(),
                    root_agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: SubmittedPromptSnapshot::from_text("inspect archive"),
                    },
                ),
                child_task(&child_session_id, &child_agent_session_id),
                SessionEventEnvelope::new(
                    child_session_id.clone(),
                    child_agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("child transcript"),
                    },
                ),
            ])
            .await
            .unwrap();

        let session_note_path =
            session_memory_note_absolute_path(export_dir.path(), &root_session_id);
        tokio::fs::create_dir_all(session_note_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &session_note_path,
            "# Session Title\n\nImported archive title\n",
        )
        .await
        .unwrap();

        let export_artifact = export_client
            .export_session_archive(root_session_id.as_str(), "tmp/archive.json")
            .await
            .unwrap();
        assert_eq!(export_artifact.root_session_id, root_session_id);
        assert_eq!(export_artifact.session_count, 2);
        assert_eq!(export_artifact.event_count, 3);
        assert_eq!(export_artifact.session_note_count, 1);

        let import_dir = tempdir().unwrap();
        let import_client = history_client(import_dir.path()).await;
        let import_artifact = import_client
            .import_session_archive(export_artifact.output_path.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(import_artifact.root_session_id, root_session_id);
        assert_eq!(import_artifact.session_count, 2);
        assert_eq!(import_artifact.event_count, 3);
        assert_eq!(import_artifact.session_note_count, 1);

        let imported_sessions = import_client.list_sessions().await.unwrap();
        assert_eq!(imported_sessions.len(), 2);
        assert!(
            imported_sessions
                .iter()
                .any(|summary| summary.session_ref == root_session_id.as_str())
        );
        assert!(
            imported_sessions
                .iter()
                .any(|summary| summary.session_ref == child_session_id.as_str())
        );

        assert_eq!(
            import_client
                .store
                .events(&child_session_id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            tokio::fs::read_to_string(session_memory_note_absolute_path(
                import_dir.path(),
                &root_session_id
            ))
            .await
            .unwrap(),
            "# Session Title\n\nImported archive title\n"
        );
    }

    #[tokio::test]
    async fn archive_import_rejects_existing_session_ids() {
        let export_dir = tempdir().unwrap();
        let export_client = history_client(export_dir.path()).await;
        export_client
            .store
            .append(SessionEventEnvelope::new(
                SessionId::from("session-root"),
                AgentSessionId::from("agent-root"),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("hello"),
                },
            ))
            .await
            .unwrap();
        let archive = export_client
            .export_session_archive("session-root", "tmp/archive.json")
            .await
            .unwrap();

        let import_dir = tempdir().unwrap();
        let import_client = history_client(import_dir.path()).await;
        import_client
            .import_session_archive(archive.output_path.to_str().unwrap())
            .await
            .unwrap();

        let error = import_client
            .import_session_archive(archive.output_path.to_str().unwrap())
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("archive import would overwrite existing sessions")
        );
    }
}
