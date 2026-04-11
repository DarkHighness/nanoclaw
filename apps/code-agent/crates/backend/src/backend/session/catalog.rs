use super::*;

impl CodeAgentSession {
    pub async fn list_sessions(&self) -> Result<Vec<crate::backend::PersistedSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        self.set_stored_session_count(sessions.len());
        let active_session_ref = self.startup_snapshot().active_session_ref;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        Ok(sessions
            .iter()
            .map(|summary| {
                session_catalog::persisted_session_summary(
                    summary,
                    &active_session_ref,
                    session_titles.get(&summary.session_id).cloned(),
                )
            })
            .collect())
    }

    pub async fn search_sessions(
        &self,
        query: &str,
    ) -> Result<Vec<crate::backend::PersistedSessionSearchMatch>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let matches = session_history::search_sessions(&self.store, query).await?;
        let sessions = session_history::list_sessions(&self.store).await?;
        let active_session_ref = self.startup_snapshot().active_session_ref;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let mut seen_session_refs = BTreeSet::new();
        let mut title_matches = Vec::new();
        let mut other_matches = Vec::new();

        for result in matches {
            let session_title = session_titles.get(&result.summary.session_id).cloned();
            let mut persisted = session_catalog::persisted_session_search_match(
                &result,
                &active_session_ref,
                session_title.clone(),
            );
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
                Some(crate::backend::PersistedSessionSearchMatch {
                    summary: session_catalog::persisted_session_summary(
                        summary,
                        &active_session_ref,
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

    pub async fn list_agent_sessions(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<crate::backend::PersistedAgentSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        let filtered_session_id = session_ref
            .map(|session_ref| {
                self.resolve_session_reference_from_catalog(&sessions, &session_titles, session_ref)
            })
            .transpose()?;
        let active_agent_session_ref = self.startup_snapshot().root_agent_session_id;
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
                &active_agent_session_ref,
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

    // Session-note titles are host-owned derived memory, not store-owned
    // transcript metadata, so the catalog layer reads them here instead of
    // widening the session-store schema for one frontend-specific cue.
    async fn load_session_note_titles<I>(&self, session_ids: I) -> BTreeMap<SessionId, String>
    where
        I: IntoIterator<Item = SessionId>,
    {
        let workspace_root = self.workspace_root().to_path_buf();
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

    pub async fn list_tasks(&self, session_ref: Option<&str>) -> Result<Vec<PersistedTaskSummary>> {
        let resolved_session_ref = if let Some(session_ref) = session_ref {
            Some(
                self.resolve_session_reference_from_operator_input(session_ref)
                    .await?
                    .to_string(),
            )
        } else {
            None
        };
        task_history::list_tasks(&self.store, resolved_session_ref.as_deref()).await
    }

    pub async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        session_history::load_session(&self.store, session_id.as_str()).await
    }

    pub async fn load_agent_session(&self, agent_session_ref: &str) -> Result<LoadedAgentSession> {
        let summary = self
            .resolve_agent_session_reference_from_operator_input(agent_session_ref)
            .await?;
        session_history::load_agent_session(&self.store, summary).await
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
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        session_history::export_session_events(
            &self.store,
            self.workspace_root(),
            session_id.as_str(),
            relative_or_absolute,
        )
        .await
    }

    pub async fn export_session_transcript(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        let session_id = self
            .resolve_session_reference_from_operator_input(session_ref)
            .await?;
        session_history::export_session_transcript(
            &self.store,
            self.workspace_root(),
            session_id.as_str(),
            relative_or_absolute,
        )
        .await
    }

    pub async fn refresh_stored_session_count(&self) -> Result<usize> {
        let count = session_history::list_sessions(&self.store).await?.len();
        self.set_stored_session_count(count);
        Ok(count)
    }

    pub(super) async fn resolve_session_reference_from_operator_input(
        &self,
        session_ref: &str,
    ) -> Result<SessionId> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let session_titles = self
            .load_session_note_titles(sessions.iter().map(|summary| summary.session_id.clone()))
            .await;
        self.resolve_session_reference_from_catalog(&sessions, &session_titles, session_ref)
    }

    pub(super) async fn resolve_agent_session_reference_from_operator_input(
        &self,
        agent_session_ref: &str,
    ) -> Result<crate::backend::PersistedAgentSessionSummary> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        self.resolve_agent_session_reference_from_catalog(&agent_sessions, agent_session_ref)
    }

    pub(super) fn resolve_session_reference_from_catalog(
        &self,
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
                return Err(anyhow::anyhow!(
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
            [] => Err(anyhow::anyhow!(
                "unknown session id, prefix, or session title: {session_ref}"
            )),
            [(summary, _)] => Ok(summary.session_id.clone()),
            _ => Err(anyhow::anyhow!(
                "ambiguous session title {session_ref}: {}",
                title_matches
                    .iter()
                    .take(6)
                    .map(|(summary, title)| {
                        session_title_reference_preview(summary.session_id.as_str(), title)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    // Session-title fallback is intentionally host-owned. Claude-style session
    // selectors use human-readable memory cues, but the underlying store should
    // keep stable transcript ids as its only hard reference surface.
    pub(super) fn resolve_agent_session_reference_from_catalog(
        &self,
        agent_sessions: &[crate::backend::PersistedAgentSessionSummary],
        agent_session_ref: &str,
    ) -> Result<crate::backend::PersistedAgentSessionSummary> {
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
                return Err(anyhow::anyhow!(
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
            0 => Err(anyhow::anyhow!(
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
                        anyhow::anyhow!(
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
            _ => Err(anyhow::anyhow!(
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

    fn set_stored_session_count(&self, count: usize) {
        self.startup.write().unwrap().stored_session_count = count;
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
    result: &mut crate::backend::PersistedSessionSearchMatch,
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
