#[path = "file/index_sidecar.rs"]
mod index_sidecar;

use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunMemoryExportBundle, RunMemoryExportRequest, RunSearchResult, RunStore,
    RunStoreError, RunSummary, apply_memory_export_request, build_memory_export_record,
    group_events_for_memory_export, search_run_events, sort_memory_export_records,
};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::{debug, info};
use types::{Message, RunEventEnvelope, RunId, SessionId};
const SEARCH_REPLAY_CONCURRENCY_LIMIT: usize = 8;

use self::index_sidecar::{
    FileRunStoreIndex, IndexedRunRecord, apply_event_to_record, delete_run_file,
    load_events_from_path, load_or_rebuild_index, persist_index_file, record_matches_query,
    select_runs_to_prune,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunStoreRetentionPolicy {
    pub max_runs: Option<usize>,
    pub max_age: Option<Duration>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRunStoreOptions {
    pub retention: RunStoreRetentionPolicy,
}

#[derive(Clone)]
pub struct FileRunStore {
    root_dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
    index: Arc<RwLock<FileRunStoreIndex>>,
    options: FileRunStoreOptions,
}

impl FileRunStore {
    pub async fn open(root_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with_options(root_dir, FileRunStoreOptions::default()).await
    }

    pub async fn open_with_options(
        root_dir: impl Into<PathBuf>,
        options: FileRunStoreOptions,
    ) -> Result<Self> {
        let root_dir = root_dir.into();
        info!(root = %root_dir.display(), "opening file run store");
        fs::create_dir_all(&root_dir).await?;
        let mut index = load_or_rebuild_index(&root_dir).await?;
        let pruned = select_runs_to_prune(&index, &options.retention, current_timestamp_ms());
        for run_id in &pruned {
            delete_run_file(&root_dir, run_id).await?;
            index.runs.remove(run_id);
        }
        if !pruned.is_empty() {
            info!(
                root = %root_dir.display(),
                pruned_runs = pruned.len(),
                "applied run-store retention during open"
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

    fn run_path(&self, run_id: &RunId) -> PathBuf {
        self.root_dir.join(format!("{}.jsonl", run_id.as_str()))
    }

    async fn persist_index(&self, index: &FileRunStoreIndex) -> Result<()> {
        persist_index_file(&self.root_dir, index).await
    }
}

#[async_trait]
impl EventSink for FileRunStore {
    async fn append(&self, event: RunEventEnvelope) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let path = self.run_path(&event.run_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let encoded = serde_json::to_string(&event)?;
        file.write_all(encoded.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        let pruned = {
            let mut index = self.index.write().expect("file run store write lock");
            let record =
                index
                    .runs
                    .entry(event.run_id.clone())
                    .or_insert_with(|| IndexedRunRecord {
                        summary: RunSummary {
                            run_id: event.run_id.clone(),
                            first_timestamp_ms: event.timestamp_ms,
                            last_timestamp_ms: event.timestamp_ms,
                            event_count: 0,
                            session_count: 0,
                            transcript_message_count: 0,
                            last_user_prompt: None,
                        },
                        session_ids: Vec::new(),
                        search_corpus: String::new(),
                    });
            apply_event_to_record(record, &event);
            select_runs_to_prune(&index, &self.options.retention, current_timestamp_ms())
        };

        for run_id in &pruned {
            delete_run_file(&self.root_dir, run_id).await?;
        }
        if !pruned.is_empty() {
            debug!(
                root = %self.root_dir.display(),
                pruned_runs = pruned.len(),
                "applied run-store retention after append"
            );
        }
        let index_snapshot = {
            let mut index = self.index.write().expect("file run store write lock");
            for run_id in &pruned {
                index.runs.remove(run_id);
            }
            index.clone()
        };
        self.persist_index(&index_snapshot).await?;
        Ok(())
    }
}

#[async_trait]
impl RunStore for FileRunStore {
    async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let index = self.index.read().expect("file run store read lock");
        let mut runs = index
            .runs
            .values()
            .map(|record| record.summary.clone())
            .collect::<Vec<_>>();
        sort_run_summaries(&mut runs);
        Ok(runs)
    }

    async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(self
                .list_runs()
                .await?
                .into_iter()
                .map(|summary| RunSearchResult {
                    summary,
                    matched_event_count: 0,
                    preview_matches: Vec::new(),
                })
                .collect());
        }

        let query_lower = query.to_lowercase();
        let candidate_summaries = {
            let index = self.index.read().expect("file run store read lock");
            index
                .runs
                .values()
                .filter(|record| record_matches_query(record, &query_lower))
                .map(|record| record.summary.clone())
                .collect::<Vec<_>>()
        };

        // Candidate summaries are already prefiltered by the sidecar index, so
        // the expensive step here is replaying JSONL event streams. Bound that
        // replay fan-out instead of serializing each run one by one.
        let query = query.to_string();
        let mut runs = stream::iter(candidate_summaries.into_iter().map(|summary| {
            let store = self.clone();
            let query = query.clone();
            async move {
                let events = store.events(&summary.run_id).await?;
                Ok::<_, RunStoreError>(search_run_events(&summary, &events, &query))
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
        runs.sort_by(|left, right| {
            right
                .matched_event_count
                .cmp(&left.matched_event_count)
                .then_with(|| {
                    right
                        .summary
                        .last_timestamp_ms
                        .cmp(&left.summary.last_timestamp_ms)
                })
                .then_with(|| {
                    left.summary
                        .run_id
                        .as_str()
                        .cmp(right.summary.run_id.as_str())
                })
        });
        Ok(runs)
    }

    async fn events(&self, run_id: &RunId) -> Result<Vec<RunEventEnvelope>> {
        let path = self.run_path(run_id);
        if !fs::try_exists(&path).await? {
            return Err(RunStoreError::RunNotFound(run_id.clone()));
        }
        load_events_from_path(&path).await
    }

    async fn session_ids(&self, run_id: &RunId) -> Result<Vec<SessionId>> {
        let cached = {
            let index = self.index.read().expect("file run store read lock");
            index
                .runs
                .get(run_id)
                .map(|record| record.session_ids.clone())
        };
        if let Some(session_ids) = cached {
            return Ok(session_ids);
        }

        let mut seen = HashSet::new();
        let mut ordered = Vec::new();
        for event in self.events(run_id).await? {
            if seen.insert(event.session_id.clone()) {
                ordered.push(event.session_id);
            }
        }
        Ok(ordered)
    }

    async fn replay_transcript(&self, run_id: &RunId) -> Result<Vec<Message>> {
        Ok(replay_transcript(&self.events(run_id).await?))
    }

    async fn export_for_memory(
        &self,
        request: RunMemoryExportRequest,
    ) -> Result<RunMemoryExportBundle> {
        let run_ids = {
            let index = self.index.read().expect("file run store read lock");
            let mut run_ids = index.runs.keys().cloned().collect::<Vec<_>>();
            run_ids.sort();
            run_ids
        };

        let mut bundle = RunMemoryExportBundle::default();
        for run_id in run_ids {
            let events = self.events(&run_id).await?;
            if let Some(record) = build_memory_export_record(
                crate::MemoryExportScope::Run,
                &run_id,
                None,
                None,
                None,
                &events,
            ) {
                bundle.runs.push(record);
            }

            let groups = group_events_for_memory_export(&events);

            for (session_id, session_events) in groups.sessions {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Session,
                    &run_id,
                    Some(session_id),
                    None,
                    None,
                    &session_events,
                ) {
                    bundle.sessions.push(record);
                }
            }

            for group in groups.subagents {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Subagent,
                    &run_id,
                    group.session_id,
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
                    &run_id,
                    group.session_id,
                    None,
                    group.task_id,
                    &group.events,
                ) {
                    bundle.tasks.push(record);
                }
            }
        }

        sort_export_records(&mut bundle.runs);
        sort_export_records(&mut bundle.sessions);
        sort_export_records(&mut bundle.subagents);
        sort_export_records(&mut bundle.tasks);
        apply_memory_export_request(&mut bundle, &request);
        Ok(bundle)
    }
}

fn sort_export_records(records: &mut [crate::RunMemoryExportRecord]) {
    sort_memory_export_records(records);
}

fn sort_run_summaries(runs: &mut [RunSummary]) {
    runs.sort_by(|left, right| {
        right
            .last_timestamp_ms
            .cmp(&left.last_timestamp_ms)
            .then_with(|| left.run_id.as_str().cmp(right.run_id.as_str()))
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
    use super::{FileRunStore, FileRunStoreOptions, RunStoreRetentionPolicy, current_timestamp_ms};
    use crate::{EventSink, RunMemoryExportRequest, RunStore};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use std::time::Duration;
    use types::{
        AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
        AgentStatus, AgentTaskSpec, Message, RunEventEnvelope, RunEventKind, RunId, SessionId,
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
            let run_id = RunId::new();
            let session_id = SessionId::new();

            let store = FileRunStore::open(dir.path()).await.unwrap();
            store
                .append(RunEventEnvelope::new(
                    run_id.clone(),
                    session_id.clone(),
                    None,
                    None,
                    RunEventKind::TranscriptMessage {
                        message: Message::user("hello"),
                    },
                ))
                .await
                .unwrap();
            drop(store);

            let reopened = FileRunStore::open(dir.path()).await.unwrap();
            let events = reopened.events(&run_id).await.unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].session_id, session_id);
            assert_eq!(reopened.replay_transcript(&run_id).await.unwrap().len(), 1);
            assert_eq!(reopened.list_runs().await.unwrap().len(), 1);
        }
    );

    bounded_async_test!(
        async fn returns_session_ids_in_encounter_order() {
            let dir = tempfile::tempdir().unwrap();
            let run_id = RunId::new();
            let session_a = SessionId::new();
            let session_b = SessionId::new();

            let store = FileRunStore::open(dir.path()).await.unwrap();
            for session in [&session_a, &session_a, &session_b] {
                store
                    .append(RunEventEnvelope::new(
                        run_id.clone(),
                        session.clone(),
                        None,
                        None,
                        RunEventKind::Notification {
                            source: "test".to_string(),
                            message: "ok".to_string(),
                        },
                    ))
                    .await
                    .unwrap();
            }

            let sessions = store.session_ids(&run_id).await.unwrap();
            assert_eq!(sessions, vec![session_a, session_b]);
        }
    );

    bounded_async_test!(
        async fn lists_persisted_runs_newest_first() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileRunStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let older_run = RunId::new();
            let newer_run = RunId::new();
            let mut older_event = RunEventEnvelope::new(
                older_run.clone(),
                session_id.clone(),
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "older".to_string(),
                },
            );
            older_event.timestamp_ms = 1;
            let mut newer_event = RunEventEnvelope::new(
                newer_run.clone(),
                session_id,
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "newer".to_string(),
                },
            );
            newer_event.timestamp_ms = 2;

            store.append(older_event).await.unwrap();
            store.append(newer_event).await.unwrap();

            let runs = store.list_runs().await.unwrap();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0].run_id, newer_run);
            assert_eq!(runs[0].last_user_prompt.as_deref(), Some("newer"));
            assert_eq!(runs[1].run_id, older_run);
        }
    );

    bounded_async_test!(
        async fn searches_persisted_runs_by_transcript_content() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileRunStore::open(dir.path()).await.unwrap();
            let run_id = RunId::new();
            let session_id = SessionId::new();
            store
                .append(RunEventEnvelope::new(
                    run_id.clone(),
                    session_id,
                    None,
                    None,
                    RunEventKind::TranscriptMessage {
                        message: Message::assistant("deploy checklist"),
                    },
                ))
                .await
                .unwrap();

            let matches = store.search_runs("checklist").await.unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].summary.run_id, run_id);
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
        async fn retention_policy_prunes_oldest_runs_by_count() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileRunStore::open_with_options(
                dir.path(),
                FileRunStoreOptions {
                    retention: RunStoreRetentionPolicy {
                        max_runs: Some(1),
                        max_age: None,
                    },
                },
            )
            .await
            .unwrap();
            let session_id = SessionId::new();
            let older_run = RunId::new();
            let newer_run = RunId::new();
            let mut older_event = RunEventEnvelope::new(
                older_run.clone(),
                session_id.clone(),
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "older".to_string(),
                },
            );
            older_event.timestamp_ms = current_timestamp_ms().saturating_sub(10_000);
            let mut newer_event = RunEventEnvelope::new(
                newer_run.clone(),
                session_id,
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "newer".to_string(),
                },
            );
            newer_event.timestamp_ms = current_timestamp_ms();

            store.append(older_event).await.unwrap();
            store.append(newer_event).await.unwrap();

            let runs = store.list_runs().await.unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].run_id, newer_run);
            assert!(store.events(&older_run).await.is_err());
        }
    );

    bounded_async_test!(
        async fn retention_policy_prunes_runs_by_age_on_open() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileRunStore::open(dir.path()).await.unwrap();
            let session_id = SessionId::new();
            let old_run = RunId::new();
            let fresh_run = RunId::new();
            let mut old_event = RunEventEnvelope::new(
                old_run.clone(),
                session_id.clone(),
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "old".to_string(),
                },
            );
            old_event.timestamp_ms = current_timestamp_ms().saturating_sub(10_000);
            let mut fresh_event = RunEventEnvelope::new(
                fresh_run.clone(),
                session_id,
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "fresh".to_string(),
                },
            );
            fresh_event.timestamp_ms = current_timestamp_ms();
            store.append(old_event).await.unwrap();
            store.append(fresh_event).await.unwrap();
            drop(store);

            let reopened = FileRunStore::open_with_options(
                dir.path(),
                FileRunStoreOptions {
                    retention: RunStoreRetentionPolicy {
                        max_runs: None,
                        max_age: Some(Duration::from_secs(1)),
                    },
                },
            )
            .await
            .unwrap();

            let runs = reopened.list_runs().await.unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].run_id, fresh_run);
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
            let store = FileRunStore::open(dir.path()).await.unwrap();
            let run_id = RunId::new();
            let session_id = SessionId::new();
            store
                .append(RunEventEnvelope::new(
                    run_id.clone(),
                    session_id.clone(),
                    None,
                    None,
                    RunEventKind::UserPromptSubmit {
                        prompt: "ship release".to_string(),
                    },
                ))
                .await
                .unwrap();

            let exports = store
                .export_for_memory(RunMemoryExportRequest {
                    max_runs: Some(1),
                    max_search_corpus_chars: Some(64),
                })
                .await
                .unwrap();
            assert_eq!(exports.runs.len(), 1);
            assert_eq!(exports.runs[0].summary.run_id, run_id);
            assert!(exports.runs[0].search_corpus.contains("ship release"));
            assert_eq!(exports.sessions.len(), 1);
            assert_eq!(
                exports.sessions[0].summary.session_id.as_ref(),
                Some(&session_id)
            );
        }
    );

    bounded_async_test!(
        async fn exports_subagent_and_task_runtime_records_from_file_store() {
            let dir = tempfile::tempdir().unwrap();
            let store = FileRunStore::open(dir.path()).await.unwrap();
            let run_id = RunId::new();
            let parent_session_id = SessionId::new();
            let child_session_id = SessionId::new();
            let child_run_id = RunId::new();
            let agent_id = AgentId::new();
            let task = AgentTaskSpec {
                task_id: "task-17".to_string(),
                role: "reviewer".to_string(),
                prompt: "review the patch".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: vec!["src/lib.rs".to_string()],
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            };
            let running_handle = AgentHandle {
                agent_id: agent_id.clone(),
                parent_agent_id: None,
                run_id: child_run_id.clone(),
                session_id: child_session_id.clone(),
                task_id: task.task_id.clone(),
                role: task.role.clone(),
                status: AgentStatus::Running,
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
                RunEventKind::UserPromptSubmit {
                    prompt: "review the latest patch".to_string(),
                },
                RunEventKind::TaskCreated {
                    task: task.clone(),
                    parent_agent_id: None,
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        agent_id.clone(),
                        None,
                        child_run_id.clone(),
                        child_session_id.clone(),
                        AgentEnvelopeKind::SpawnRequested { task: task.clone() },
                    ),
                },
                RunEventKind::SubagentStart {
                    handle: running_handle.clone(),
                    task: task.clone(),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        agent_id.clone(),
                        None,
                        child_run_id.clone(),
                        child_session_id.clone(),
                        AgentEnvelopeKind::Message {
                            channel: "handoff".to_string(),
                            payload: json!({"note": "checked ownership"}),
                        },
                    ),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        agent_id.clone(),
                        None,
                        child_run_id.clone(),
                        child_session_id.clone(),
                        AgentEnvelopeKind::Result {
                            result: result.clone(),
                        },
                    ),
                },
                RunEventKind::TaskCompleted {
                    task_id: task.task_id.clone(),
                    agent_id: agent_id.clone(),
                    status: AgentStatus::Completed,
                },
                RunEventKind::SubagentStop {
                    handle: AgentHandle {
                        status: AgentStatus::Completed,
                        ..running_handle.clone()
                    },
                    result: Some(result.clone()),
                    error: None,
                },
            ] {
                store
                    .append(RunEventEnvelope::new(
                        run_id.clone(),
                        parent_session_id.clone(),
                        None,
                        None,
                        event,
                    ))
                    .await
                    .unwrap();
            }

            let exports = store
                .export_for_memory(RunMemoryExportRequest::default())
                .await
                .unwrap();
            assert_eq!(exports.subagents.len(), 1);
            assert_eq!(
                exports.subagents[0].summary.session_id.as_ref(),
                Some(&child_session_id)
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
                exports.tasks[0].summary.session_id.as_ref(),
                Some(&child_session_id)
            );
            assert_eq!(exports.tasks[0].summary.task_id.as_deref(), Some("task-17"));
            assert!(exports.tasks[0].search_corpus.contains("checked ownership"));
        }
    );
}
