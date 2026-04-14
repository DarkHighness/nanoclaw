use crate::backend::session_catalog;
use crate::backend::session_history::{self, preview_id};
use crate::backend::session_memory_compaction::session_memory_note_absolute_path;
use crate::backend::session_memory_note::session_memory_note_title;
use crate::ui::{
    LoadedSession, PersistedSessionSearchMatch, PersistedSessionSummary, SessionExportArtifact,
};
use agent::types::SessionId;
use anyhow::{Context, Result, anyhow};
use futures::{StreamExt, stream};
use nanoclaw_config::CoreConfig;
use std::collections::{BTreeMap, BTreeSet};
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
    use super::{prepend_session_title_preview, resolve_session_reference_from_catalog};
    use crate::ui::{PersistedSessionSearchMatch, ResumeSupport};
    use agent::types::{SessionId, SessionSummaryTokenUsage};
    use std::collections::BTreeMap;
    use store::{SessionSearchResult, SessionSummary};

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
}
