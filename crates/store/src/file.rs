#[path = "file/index_sidecar.rs"]
mod index_sidecar;

use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, SessionMemoryExportBundle, SessionMemoryExportRequest, SessionSearchResult,
    SessionStore, SessionStoreError, SessionSummary, apply_memory_export_request,
    build_memory_export_record, group_events_for_memory_export, search_session_events_ranked,
    sort_memory_export_records, sort_ranked_session_search_results,
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::{debug, info};
use types::{AgentSessionId, Message, SessionEventEnvelope, SessionId};
const SEARCH_REPLAY_CONCURRENCY_LIMIT: usize = 8;

use self::index_sidecar::{
    FileSessionStoreIndex, IndexedSessionRecord, apply_event_to_record, delete_session_file,
    indexed_record_from_events, load_events_from_path, load_or_rebuild_index, persist_index_file,
    record_matches_query, select_sessions_to_prune,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionStoreRetentionPolicy {
    pub max_sessions: Option<usize>,
    pub max_age: Option<Duration>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSessionStoreOptions {
    pub retention: SessionStoreRetentionPolicy,
}

#[derive(Clone)]
pub struct FileSessionStore {
    root_dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
    index: Arc<RwLock<FileSessionStoreIndex>>,
    options: FileSessionStoreOptions,
}

impl FileSessionStore {
    pub async fn open(root_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with_options(root_dir, FileSessionStoreOptions::default()).await
    }

    pub async fn open_with_options(
        root_dir: impl Into<PathBuf>,
        options: FileSessionStoreOptions,
    ) -> Result<Self> {
        let root_dir = root_dir.into();
        info!(root = %root_dir.display(), "opening file session store");
        fs::create_dir_all(&root_dir).await?;
        let mut index = load_or_rebuild_index(&root_dir).await?;
        let pruned = select_sessions_to_prune(&index, &options.retention, current_timestamp_ms());
        for session_id in &pruned {
            delete_session_file(&root_dir, session_id).await?;
            index.sessions.remove(session_id);
        }
        if !pruned.is_empty() {
            info!(
                root = %root_dir.display(),
                pruned_sessions = pruned.len(),
                "applied session-store retention during open"
            );
        }
        persist_index_file(&root_dir, &index).await?;
        Ok(Self {
            root_dir,
            write_lock: Arc::new(Mutex::new(())),
            index: Arc::new(RwLock::new(index)),
            options,
        })
    }

    #[must_use]
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn session_path(&self, session_id: &SessionId) -> PathBuf {
        self.root_dir.join(format!("{}.jsonl", session_id.as_str()))
    }

    async fn persist_index(&self, index: &FileSessionStoreIndex) -> Result<()> {
        persist_index_file(&self.root_dir, index).await
    }

    async fn append_locked_events(&self, events: &[SessionEventEnvelope]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        let mut active_session_id: Option<SessionId> = None;
        let mut active_file: Option<tokio::fs::File> = None;
        let mut rebuild_sessions = HashSet::new();
        let mut rebuild_fallbacks = HashMap::new();

        for event in events {
            if active_session_id.as_ref() != Some(&event.session_id) {
                if let Some(mut file) = active_file.take() {
                    file.flush().await?;
                }
                let path = self.session_path(&event.session_id);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).await?;
                }
                active_file = Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .await?,
                );
                active_session_id = Some(event.session_id.clone());
            }

            let file = active_file.as_mut().expect("active session file");
            let encoded = serde_json::to_string(event)?;
            file.write_all(encoded.as_bytes()).await?;
            file.write_all(b"\n").await?;

            if rebuild_index_record(&event.event) {
                rebuild_sessions.insert(event.session_id.clone());
                rebuild_fallbacks.insert(event.session_id.clone(), event.clone());
            }
        }

        if let Some(mut file) = active_file.take() {
            file.flush().await?;
        }

        let mut rebuilt_records = HashMap::new();
        for session_id in &rebuild_sessions {
            let path = self.session_path(session_id);
            let rebuilt = indexed_record_from_events(load_events_from_path(&path).await?)
                .unwrap_or_else(|| default_indexed_record(rebuild_fallbacks[session_id].clone()));
            rebuilt_records.insert(session_id.clone(), rebuilt);
        }

        let pruned = {
            let mut index = self.index.write().expect("file session store write lock");
            for event in events {
                if rebuild_sessions.contains(&event.session_id) {
                    continue;
                }
                let record = index
                    .sessions
                    .entry(event.session_id.clone())
                    .or_insert_with(|| default_indexed_record(event.clone()));
                apply_event_to_record(record, event);
            }
            for (session_id, record) in rebuilt_records {
                index.sessions.insert(session_id, record);
            }
            select_sessions_to_prune(&index, &self.options.retention, current_timestamp_ms())
        };

        for session_id in &pruned {
            delete_session_file(&self.root_dir, session_id).await?;
        }
        if !pruned.is_empty() {
            debug!(
                root = %self.root_dir.display(),
                pruned_sessions = pruned.len(),
                "applied session-store retention after append"
            );
        }
        let index_snapshot = {
            let mut index = self.index.write().expect("file session store write lock");
            for session_id in &pruned {
                index.sessions.remove(session_id);
            }
            index.clone()
        };
        self.persist_index(&index_snapshot).await?;
        Ok(())
    }
}

#[async_trait]
impl EventSink for FileSessionStore {
    async fn append(&self, event: SessionEventEnvelope) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.append_locked_events(std::slice::from_ref(&event))
            .await
    }

    async fn append_batch(&self, events: Vec<SessionEventEnvelope>) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.append_locked_events(&events).await
    }
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let index = self.index.read().expect("file session store read lock");
        let mut sessions = index
            .sessions
            .values()
            .map(|record| record.summary.clone())
            .collect::<Vec<_>>();
        sort_session_summaries(&mut sessions);
        Ok(sessions)
    }

    async fn search_sessions(&self, query: &str) -> Result<Vec<SessionSearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(self
                .list_sessions()
                .await?
                .into_iter()
                .map(|summary| SessionSearchResult {
                    summary,
                    matched_event_count: 0,
                    preview_matches: Vec::new(),
                })
                .collect());
        }

        let query_lower = query.to_lowercase();
        let candidate_summaries = {
            let index = self.index.read().expect("file session store read lock");
            index
                .sessions
                .values()
                .filter(|record| record_matches_query(record, &query_lower))
                .map(|record| record.summary.clone())
                .collect::<Vec<_>>()
        };

        // Candidate summaries are already prefiltered by the sidecar index, so
        // the expensive step here is replaying JSONL event streams. Bound that
        // replay fan-out instead of serializing each session one by one.
        let query = query.to_string();
        let mut sessions = stream::iter(candidate_summaries.into_iter().map(|summary| {
            let store = self.clone();
            let query = query.clone();
            async move {
                let events = store.events(&summary.session_id).await?;
                Ok::<_, SessionStoreError>(search_session_events_ranked(&summary, &events, &query))
            }
        }))
        .buffer_unordered(SEARCH_REPLAY_CONCURRENCY_LIMIT)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        sort_ranked_session_search_results(&mut sessions);
        Ok(sessions.into_iter().map(|ranked| ranked.result).collect())
    }

    async fn events(&self, session_id: &SessionId) -> Result<Vec<SessionEventEnvelope>> {
        let path = self.session_path(session_id);
        if !fs::try_exists(&path).await? {
            return Err(SessionStoreError::SessionNotFound(session_id.clone()));
        }
        load_events_from_path(&path).await
    }

    async fn agent_session_ids(&self, session_id: &SessionId) -> Result<Vec<AgentSessionId>> {
        let cached = {
            let index = self.index.read().expect("file session store read lock");
            index
                .sessions
                .get(session_id)
                .map(|record| record.agent_session_ids.clone())
        };
        if let Some(agent_session_ids) = cached {
            return Ok(agent_session_ids);
        }

        let mut seen = HashSet::new();
        let mut ordered = Vec::new();
        for event in self.events(session_id).await? {
            if seen.insert(event.agent_session_id.clone()) {
                ordered.push(event.agent_session_id);
            }
        }
        Ok(ordered)
    }

    async fn replay_transcript(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        Ok(replay_transcript(&self.events(session_id).await?))
    }

    async fn export_for_memory(
        &self,
        request: SessionMemoryExportRequest,
    ) -> Result<SessionMemoryExportBundle> {
        let session_ids = {
            let index = self.index.read().expect("file session store read lock");
            let mut session_ids = index.sessions.keys().cloned().collect::<Vec<_>>();
            session_ids.sort();
            session_ids
        };

        let mut bundle = SessionMemoryExportBundle::default();
        for session_id in session_ids {
            let events = self.events(&session_id).await?;
            if let Some(record) = build_memory_export_record(
                crate::MemoryExportScope::Session,
                &session_id,
                None,
                None,
                None,
                &events,
            ) {
                bundle.sessions.push(record);
            }

            let groups = group_events_for_memory_export(&events);

            for (agent_session_id, session_events) in groups.agent_sessions {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::AgentSession,
                    &session_id,
                    Some(agent_session_id),
                    None,
                    None,
                    &session_events,
                ) {
                    bundle.agent_sessions.push(record);
                }
            }

            for group in groups.subagents {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Subagent,
                    &session_id,
                    group.agent_session_id,
                    group.agent_name,
                    None,
                    &group.events,
                ) {
                    bundle.subagents.push(record);
                }
            }

            for group in groups.tasks {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Task,
                    &session_id,
                    group.agent_session_id,
                    None,
                    group.task_id,
                    &group.events,
                ) {
                    bundle.tasks.push(record);
                }
            }
        }

        sort_export_records(&mut bundle.sessions);
        sort_export_records(&mut bundle.agent_sessions);
        sort_export_records(&mut bundle.subagents);
        sort_export_records(&mut bundle.tasks);
        apply_memory_export_request(&mut bundle, &request);
        Ok(bundle)
    }
}

fn rebuild_index_record(event: &types::SessionEventKind) -> bool {
    // Compaction rewrites the visible transcript window without mutating the
    // older transcript message events, so cached summary counts must be
    // rebuilt from the full session log instead of incrementally adjusted.
    matches!(
        event,
        types::SessionEventKind::TranscriptMessagePatched { .. }
            | types::SessionEventKind::TranscriptMessageRemoved { .. }
            | types::SessionEventKind::CompactionCompleted { .. }
    )
}

fn default_indexed_record(event: SessionEventEnvelope) -> IndexedSessionRecord {
    IndexedSessionRecord {
        summary: SessionSummary {
            session_id: event.session_id,
            first_timestamp_ms: event.timestamp_ms,
            last_timestamp_ms: event.timestamp_ms,
            event_count: 0,
            agent_session_count: 0,
            transcript_message_count: 0,
            last_user_prompt: None,
            token_usage: None,
        },
        agent_session_ids: Vec::new(),
        search_corpus: String::new(),
        agent_session_token_usage: std::collections::BTreeMap::new(),
    }
}

fn sort_export_records(records: &mut [crate::SessionMemoryExportRecord]) {
    sort_memory_export_records(records);
}

fn sort_session_summaries(sessions: &mut [SessionSummary]) {
    sessions.sort_by(|left, right| {
        right
            .last_timestamp_ms
            .cmp(&left.last_timestamp_ms)
            .then_with(|| left.session_id.as_str().cmp(right.session_id.as_str()))
    });
}

fn current_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::{
        FileSessionStore, FileSessionStoreOptions, SessionStoreRetentionPolicy,
        current_timestamp_ms,
    };
    use crate::{EventSink, SessionMemoryExportRequest, SessionStore};
    use nanoclaw_test_support::run_current_thread_test;
    use std::time::Duration;
    use types::{
        AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
        AgentSessionId, AgentStatus, AgentTaskSpec, ContextWindowUsage, Message, MessageId,
        SessionEventEnvelope, SessionEventKind, SessionId, SubmittedPromptSnapshot,
        TokenLedgerSnapshot, TokenUsage, TokenUsagePhase,
    };

    use super::index_sidecar::append_search_text;

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    bounded_async_test!(
        async fn persists_events_across_store_reopen() {
            let dir = tempfile::tempdir().unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();

            let store = FileSessionStore::open(dir.path()).await.unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TranscriptMessage {
                        message: Message::user("hello"),
                    },
                ))
                .await
                .unwrap();
            drop(store);

            let reopened = FileSessionStore::open(dir.path()).await.unwrap();
            let events = reopened.events(&session_id).await.unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].agent_session_id, agent_session_id);
            assert_eq!(
                reopened.replay_transcript(&session_id).await.unwrap().len(),
                1
            );
            assert_eq!(reopened.list_sessions().await.unwrap().len(), 1);
        }
    );

    bounded_async_test!(
        async fn append_batch_persists_multiple_events_in_order() {
            let dir = tempfile::tempdir().unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();

            let store = FileSessionStore::open(dir.path()).await.unwrap();
            store
                .append_batch(vec![
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::UserPromptSubmit {
                            prompt: SubmittedPromptSnapshot::from_text("ship it"),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::assistant("done"),
                        },
                    ),
                ])
                .await
                .unwrap();

            let events = store.events(&session_id).await.unwrap();
            assert_eq!(events.len(), 2);
            assert!(matches!(
                events[0].event,
                SessionEventKind::UserPromptSubmit { .. }
            ));
            assert!(matches!(
                events[1].event,
                SessionEventKind::TranscriptMessage { .. }
            ));
            assert_eq!(
                store.list_sessions().await.unwrap()[0]
                    .last_user_prompt
                    .as_deref(),
                Some("ship it")
            );
        }
    );

    bounded_async_test!(
        async fn returns_session_ids_in_encounter_order() {
            let dir = tempfile::tempdir().unwrap();
            let session_id = SessionId::new();
            let session_a = AgentSessionId::new();
            let session_b = AgentSessionId::new();

            let store = FileSessionStore::open(dir.path()).await.unwrap();
            for session in [&session_a, &session_a, &session_b] {
                store
                    .append(SessionEventEnvelope::new(
                        session_id.clone(),
                        session.clone(),
                        None,
                        None,
                        SessionEventKind::Notification {
                            source: "test".to_string(),
                            message: "ok".to_string(),
                        },
                    ))
                    .await
                    .unwrap();
            }

            let sessions = store.agent_session_ids(&session_id).await.unwrap();
            assert_eq!(sessions, vec![session_a, session_b]);
        }
    );

    bounded_async_test!(
        async fn lists_persisted_runs_newest_first() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let agent_session_id = AgentSessionId::new();
            let older_run = SessionId::new();
            let newer_run = SessionId::new();
            let mut older_event = SessionEventEnvelope::new(
                older_run.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("older"),
                },
            );
            older_event.timestamp_ms = 1;
            let mut newer_event = SessionEventEnvelope::new(
                newer_run.clone(),
                agent_session_id,
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("newer"),
                },
            );
            newer_event.timestamp_ms = 2;

            store.append(older_event).await.unwrap();
            store.append(newer_event).await.unwrap();

            let sessions = store.list_sessions().await.unwrap();
            assert_eq!(sessions.len(), 2);
            assert_eq!(sessions[0].session_id, newer_run);
            assert_eq!(sessions[0].last_user_prompt.as_deref(), Some("newer"));
            assert_eq!(sessions[1].session_id, older_run);
        }
    );

    bounded_async_test!(
        async fn searches_persisted_runs_by_transcript_content() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id,
                    None,
                    None,
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("deploy checklist"),
                    },
                ))
                .await
                .unwrap();

            let matches = store.search_sessions("checklist").await.unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].summary.session_id, session_id);
            assert_eq!(matches[0].matched_event_count, 1);
            assert!(
                matches[0]
                    .preview_matches
                    .iter()
                    .any(|line| line.contains("deploy checklist"))
            );
        }
    );

    bounded_async_test!(
        async fn search_prefers_prompt_matches_over_transcript_only_hits() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let prompt_session_id = SessionId::new();
            let transcript_session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();

            store
                .append_batch(vec![
                    SessionEventEnvelope::new(
                        prompt_session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::UserPromptSubmit {
                            prompt: SubmittedPromptSnapshot::from_text("release planner"),
                        },
                    ),
                    SessionEventEnvelope::new(
                        prompt_session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::assistant("status green"),
                        },
                    ),
                    SessionEventEnvelope::new(
                        transcript_session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::UserPromptSubmit {
                            prompt: SubmittedPromptSnapshot::from_text("status update"),
                        },
                    ),
                    SessionEventEnvelope::new(
                        transcript_session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::assistant("release notes drafted"),
                        },
                    ),
                    SessionEventEnvelope::new(
                        transcript_session_id,
                        agent_session_id,
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::assistant("release checklist ready"),
                        },
                    ),
                ])
                .await
                .unwrap();

            let matches = store.search_sessions("release").await.unwrap();
            assert_eq!(matches.len(), 2);
            assert_eq!(matches[0].summary.session_id, prompt_session_id);
            assert!(
                matches[0]
                    .preview_matches
                    .first()
                    .is_some_and(|line| line.contains("prompt: release planner"))
            );
        }
    );

    bounded_async_test!(
        async fn reports_persisted_root_and_subagent_token_usage() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let parent_agent_session_id = AgentSessionId::new();
            let rotated_parent_agent_session_id = AgentSessionId::new();
            let child_session_id = SessionId::new();
            let child_agent_session_id = AgentSessionId::new();
            let agent_id = AgentId::new();
            let task = AgentTaskSpec {
                task_id: "task-usage".into(),
                role: "reviewer".to_string(),
                prompt: "review the patch".to_string(),
                origin: types::TaskOrigin::ChildAgentBacked,
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            };

            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    parent_agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TokenUsageUpdated {
                        phase: TokenUsagePhase::ResponseCompleted,
                        ledger: TokenLedgerSnapshot {
                            context_window: Some(ContextWindowUsage {
                                used_tokens: 100,
                                max_tokens: 400_000,
                            }),
                            last_usage: Some(TokenUsage::from_input_output(100, 20, 10)),
                            cumulative_usage: TokenUsage::from_input_output(100, 20, 10),
                        },
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    parent_agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::SessionEnd {
                        reason: Some("compaction".to_string()),
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    rotated_parent_agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::SessionStart {
                        reason: Some("compaction".to_string()),
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    rotated_parent_agent_session_id,
                    None,
                    None,
                    SessionEventKind::TokenUsageUpdated {
                        phase: TokenUsagePhase::ResponseCompleted,
                        ledger: TokenLedgerSnapshot {
                            context_window: Some(ContextWindowUsage {
                                used_tokens: 80,
                                max_tokens: 400_000,
                            }),
                            last_usage: Some(TokenUsage::from_input_output(25, 5, 0)),
                            cumulative_usage: TokenUsage::from_input_output(25, 5, 0),
                        },
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    parent_agent_session_id,
                    None,
                    None,
                    SessionEventKind::SubagentStart {
                        handle: AgentHandle {
                            agent_id,
                            parent_agent_id: None,
                            session_id: child_session_id.clone(),
                            agent_session_id: child_agent_session_id.clone(),
                            task_id: task.task_id.clone(),
                            role: task.role.clone(),
                            status: AgentStatus::Running,
                            worktree_id: None,
                            worktree_root: None,
                        },
                        task,
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    child_session_id.clone(),
                    child_agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TokenUsageUpdated {
                        phase: TokenUsagePhase::ResponseCompleted,
                        ledger: TokenLedgerSnapshot {
                            context_window: Some(ContextWindowUsage {
                                used_tokens: 60,
                                max_tokens: 200_000,
                            }),
                            last_usage: Some(TokenUsage::from_input_output(40, 10, 5)),
                            cumulative_usage: TokenUsage::from_input_output(40, 10, 5),
                        },
                    },
                ))
                .await
                .unwrap();

            let report = store.token_usage(&session_id).await.unwrap();
            let summaries = store.list_sessions().await.unwrap();
            assert_eq!(
                report
                    .session
                    .as_ref()
                    .map(|record| record.ledger.cumulative_usage),
                Some(TokenUsage::from_input_output(125, 25, 10))
            );
            assert_eq!(report.agent_sessions.len(), 2);
            assert_eq!(report.subagents.len(), 1);
            assert_eq!(
                report.subagents[0].agent_session_id.as_ref(),
                Some(&child_agent_session_id)
            );
            assert_eq!(report.subagents[0].agent_name.as_deref(), Some("reviewer"));
            assert_eq!(report.tasks.len(), 1);
            assert_eq!(report.tasks[0].task_id.as_deref(), Some("task-usage"));
            assert_eq!(
                report.aggregate_usage,
                TokenUsage::from_input_output(165, 35, 15)
            );
            assert_eq!(summaries.len(), 2);
            assert_eq!(
                summaries
                    .iter()
                    .find(|summary| summary.session_id == session_id)
                    .and_then(|summary| summary.token_usage.clone())
                    .map(|usage| usage.cumulative_usage),
                Some(TokenUsage::from_input_output(125, 25, 10))
            );

            drop(store);
            let reopened = FileSessionStore::open(dir.path()).await.unwrap();
            let reopened_summaries = reopened.list_sessions().await.unwrap();
            assert_eq!(
                reopened_summaries
                    .iter()
                    .find(|summary| summary.session_id == session_id)
                    .and_then(|summary| summary.token_usage.clone())
                    .map(|usage| usage.cumulative_usage),
                Some(TokenUsage::from_input_output(125, 25, 10))
            );
        }
    );

    bounded_async_test!(
        async fn search_rebuilds_index_after_transcript_patch_and_remove_events() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            let checklist_id = MessageId::from("msg_checklist");
            let temporary_id = MessageId::from("msg_2");
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("deploy checklist")
                            .with_message_id(checklist_id.clone()),
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("temporary draft")
                            .with_message_id(temporary_id.clone()),
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::TranscriptMessagePatched {
                        message_id: checklist_id,
                        message: Message::assistant("release checklist"),
                    },
                ))
                .await
                .unwrap();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id,
                    None,
                    None,
                    SessionEventKind::TranscriptMessageRemoved {
                        message_id: temporary_id,
                    },
                ))
                .await
                .unwrap();

            assert!(store.search_sessions("deploy").await.unwrap().is_empty());
            assert!(
                store
                    .search_sessions("temporary draft")
                    .await
                    .unwrap()
                    .is_empty()
            );

            let matches = store.search_sessions("release").await.unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].summary.session_id, session_id);
            assert!(
                matches[0]
                    .preview_matches
                    .iter()
                    .any(|line| line.contains("release checklist"))
            );
        }
    );

    bounded_async_test!(
        async fn list_sessions_counts_visible_transcript_after_compaction() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            let older_prompt =
                Message::user("older prompt").with_message_id(MessageId::from("msg_older_prompt"));
            let older_answer = Message::assistant("older answer")
                .with_message_id(MessageId::from("msg_older_answer"));
            let kept = Message::user("keep this").with_message_id(MessageId::from("msg_keep"));
            let summary =
                Message::system("summary").with_message_id(MessageId::from("msg_summary"));
            let after = Message::assistant("after compaction")
                .with_message_id(MessageId::from("msg_after"));

            store
                .append_batch(vec![
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: older_prompt,
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: older_answer,
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: kept.clone(),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: summary.clone(),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::CompactionCompleted {
                            reason: "manual".to_string(),
                            source_message_count: 2,
                            retained_message_count: 1,
                            summary_chars: 7,
                            summary_message_id: Some(summary.message_id.clone()),
                            retained_tail_message_ids: vec![kept.message_id.clone()],
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id,
                        None,
                        None,
                        SessionEventKind::TranscriptMessage { message: after },
                    ),
                ])
                .await
                .unwrap();

            let sessions = store.list_sessions().await.unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].transcript_message_count, 3);

            drop(store);

            let reopened = FileSessionStore::open(dir.path()).await.unwrap();
            let sessions = reopened.list_sessions().await.unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].transcript_message_count, 3);
        }
    );

    bounded_async_test!(
        async fn search_sessions_skip_hidden_compacted_transcript_text() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            let kept = Message::user("keep this").with_message_id(MessageId::from("msg_keep"));
            let summary =
                Message::system("summary").with_message_id(MessageId::from("msg_summary"));
            let after = Message::assistant("after compaction")
                .with_message_id(MessageId::from("msg_after"));

            store
                .append_batch(vec![
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::user("older prompt")
                                .with_message_id(MessageId::from("msg_older_prompt")),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::assistant("older answer")
                                .with_message_id(MessageId::from("msg_older_answer")),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: kept.clone(),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: summary.clone(),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::CompactionCompleted {
                            reason: "manual".to_string(),
                            source_message_count: 2,
                            retained_message_count: 1,
                            summary_chars: 7,
                            summary_message_id: Some(summary.message_id.clone()),
                            retained_tail_message_ids: vec![kept.message_id.clone()],
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id,
                        None,
                        None,
                        SessionEventKind::TranscriptMessage { message: after },
                    ),
                ])
                .await
                .unwrap();

            assert!(
                store
                    .search_sessions("older answer")
                    .await
                    .unwrap()
                    .is_empty()
            );

            let matches = store.search_sessions("after compaction").await.unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].summary.transcript_message_count, 3);
            assert!(
                matches[0]
                    .preview_matches
                    .iter()
                    .any(|line| line.contains("after compaction"))
            );

            drop(store);

            let reopened = FileSessionStore::open(dir.path()).await.unwrap();
            assert!(
                reopened
                    .search_sessions("older answer")
                    .await
                    .unwrap()
                    .is_empty()
            );
            let matches = reopened.search_sessions("after compaction").await.unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].summary.transcript_message_count, 3);
        }
    );

    bounded_async_test!(
        async fn retention_policy_prunes_oldest_runs_by_count() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open_with_options(
                dir.path(),
                FileSessionStoreOptions {
                    retention: SessionStoreRetentionPolicy {
                        max_sessions: Some(1),
                        max_age: None,
                    },
                },
            )
            .await
            .unwrap();
            let agent_session_id = AgentSessionId::new();
            let older_run = SessionId::new();
            let newer_run = SessionId::new();
            let mut older_event = SessionEventEnvelope::new(
                older_run.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("older"),
                },
            );
            older_event.timestamp_ms = current_timestamp_ms().saturating_sub(10_000);
            let mut newer_event = SessionEventEnvelope::new(
                newer_run.clone(),
                agent_session_id,
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("newer"),
                },
            );
            newer_event.timestamp_ms = current_timestamp_ms();

            store.append(older_event).await.unwrap();
            store.append(newer_event).await.unwrap();

            let sessions = store.list_sessions().await.unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].session_id, newer_run);
            assert!(store.events(&older_run).await.is_err());
        }
    );

    bounded_async_test!(
        async fn retention_policy_prunes_runs_by_age_on_open() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let agent_session_id = AgentSessionId::new();
            let old_run = SessionId::new();
            let fresh_run = SessionId::new();
            let mut old_event = SessionEventEnvelope::new(
                old_run.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("old"),
                },
            );
            old_event.timestamp_ms = current_timestamp_ms().saturating_sub(10_000);
            let mut fresh_event = SessionEventEnvelope::new(
                fresh_run.clone(),
                agent_session_id,
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("fresh"),
                },
            );
            fresh_event.timestamp_ms = current_timestamp_ms();
            store.append(old_event).await.unwrap();
            store.append(fresh_event).await.unwrap();
            drop(store);

            let reopened = FileSessionStore::open_with_options(
                dir.path(),
                FileSessionStoreOptions {
                    retention: SessionStoreRetentionPolicy {
                        max_sessions: None,
                        max_age: Some(Duration::from_secs(1)),
                    },
                },
            )
            .await
            .unwrap();

            let sessions = reopened.list_sessions().await.unwrap();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].session_id, fresh_run);
            assert!(reopened.events(&old_run).await.is_err());
        }
    );

    #[test]
    fn search_corpus_is_capped_to_recent_text() {
        let mut corpus = String::new();
        append_search_text(&mut corpus, &"x".repeat(20_000));
        assert!(corpus.chars().count() <= 16_384);
    }

    bounded_async_test!(
        async fn exports_runs_for_memory_from_index_sidecar() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: SubmittedPromptSnapshot::from_text("ship release"),
                    },
                ))
                .await
                .unwrap();

            let exports = store
                .export_for_memory(SessionMemoryExportRequest {
                    max_sessions: Some(1),
                    max_search_corpus_chars: Some(64),
                })
                .await
                .unwrap();
            assert_eq!(exports.sessions.len(), 1);
            assert_eq!(exports.sessions[0].summary.session_id, session_id);
            assert!(exports.sessions[0].search_corpus.contains("ship release"));
            assert_eq!(exports.agent_sessions.len(), 1);
            assert_eq!(
                exports.agent_sessions[0].summary.agent_session_id.as_ref(),
                Some(&agent_session_id)
            );
        }
    );

    bounded_async_test!(
        async fn exports_subagent_and_task_runtime_records_from_file_store() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileSessionStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let parent_agent_session_id = AgentSessionId::new();
            let child_session_id = SessionId::new();
            let child_agent_session_id = AgentSessionId::new();
            let agent_id = AgentId::new();
            let task = AgentTaskSpec {
                task_id: "task-17".into(),
                role: "reviewer".to_string(),
                prompt: "review the patch".to_string(),
                origin: types::TaskOrigin::ChildAgentBacked,
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: vec!["src/lib.rs".to_string()],
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            };
            let running_handle = AgentHandle {
                agent_id: agent_id.clone(),
                parent_agent_id: None,
                session_id: child_session_id.clone(),
                agent_session_id: child_agent_session_id.clone(),
                task_id: task.task_id.clone(),
                role: task.role.clone(),
                status: AgentStatus::Running,
                worktree_id: None,
                worktree_root: None,
            };
            let result = AgentResultEnvelope {
                agent_id: agent_id.clone(),
                task_id: task.task_id.clone(),
                status: AgentStatus::Completed,
                summary: "review completed".to_string(),
                text: "found no blocking issues".to_string(),
                artifacts: vec![AgentArtifact {
                    kind: "report".to_string(),
                    uri: "reports/review.md".to_string(),
                    label: Some("review report".to_string()),
                    metadata: None,
                }],
                claimed_files: vec!["src/lib.rs".to_string()],
                structured_payload: None,
            };

            for event in [
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("review the latest patch"),
                },
                SessionEventKind::TaskCreated {
                    task: task.clone(),
                    parent_agent_id: None,
                    status: types::TaskStatus::Open,
                    summary: Some(task.prompt.clone()),
                    worktree_id: None,
                    worktree_root: None,
                },
                SessionEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        agent_id.clone(),
                        None,
                        child_session_id.clone(),
                        child_agent_session_id.clone(),
                        AgentEnvelopeKind::SpawnRequested { task: task.clone() },
                    ),
                },
                SessionEventKind::SubagentStart {
                    handle: running_handle.clone(),
                    task: task.clone(),
                },
                SessionEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        agent_id.clone(),
                        None,
                        child_session_id.clone(),
                        child_agent_session_id.clone(),
                        AgentEnvelopeKind::Input {
                            message: Message::user("checked ownership"),
                            delivery: types::AgentInputDelivery::Queue,
                        },
                    ),
                },
                SessionEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        agent_id.clone(),
                        None,
                        child_session_id.clone(),
                        child_agent_session_id.clone(),
                        AgentEnvelopeKind::Result {
                            result: result.clone(),
                        },
                    ),
                },
                SessionEventKind::TaskCompleted {
                    task_id: task.task_id.clone(),
                    agent_id: agent_id.clone(),
                    status: types::TaskStatus::Completed,
                },
                SessionEventKind::SubagentStop {
                    handle: AgentHandle {
                        status: AgentStatus::Completed,
                        ..running_handle.clone()
                    },
                    result: Some(result.clone()),
                    error: None,
                },
            ] {
                store
                    .append(SessionEventEnvelope::new(
                        session_id.clone(),
                        parent_agent_session_id.clone(),
                        None,
                        None,
                        event,
                    ))
                    .await
                    .unwrap();
            }

            let exports = store
                .export_for_memory(SessionMemoryExportRequest::default())
                .await
                .unwrap();
            assert_eq!(exports.subagents.len(), 1);
            assert_eq!(
                exports.subagents[0].summary.agent_session_id.as_ref(),
                Some(&child_agent_session_id)
            );
            assert_eq!(
                exports.subagents[0].summary.agent_name.as_deref(),
                Some("reviewer")
            );
            assert!(
                exports.subagents[0]
                    .search_corpus
                    .contains("found no blocking issues")
            );

            assert_eq!(exports.tasks.len(), 1);
            assert_eq!(
                exports.tasks[0].summary.agent_session_id.as_ref(),
                Some(&child_agent_session_id)
            );
            assert_eq!(exports.tasks[0].summary.task_id.as_deref(), Some("task-17"));
            assert!(exports.tasks[0].search_corpus.contains("checked ownership"));
        }
    );
}
