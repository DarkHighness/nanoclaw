use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunSearchResult, RunStore, RunStoreError, RunSummary, search_run_events,
    summarize_run_events,
};
use agent_core_types::{Message, RunEventEnvelope, RunId, SessionId};
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct FileRunStore {
    root_dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl FileRunStore {
    pub async fn open(root_dir: impl Into<PathBuf>) -> Result<Self> {
        let root_dir = root_dir.into();
        fs::create_dir_all(&root_dir).await?;
        Ok(Self {
            root_dir,
            write_lock: Arc::new(Mutex::new(())),
        })
    }

    #[must_use]
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn run_path(&self, run_id: &RunId) -> PathBuf {
        self.root_dir.join(format!("{}.jsonl", run_id.0))
    }

    async fn load_events_from_path(&self, path: &Path) -> Result<Vec<RunEventEnvelope>> {
        let file = fs::File::open(path).await?;
        let mut lines = BufReader::new(file).lines();
        let mut events = Vec::new();
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            events.push(serde_json::from_str(&line)?);
        }
        Ok(events)
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
            .open(path)
            .await?;
        let encoded = serde_json::to_string(&event)?;
        file.write_all(encoded.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok(())
    }
}

#[async_trait]
impl RunStore for FileRunStore {
    async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let mut runs = Vec::new();
        let mut entries = fs::read_dir(&self.root_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let events = self.load_events_from_path(&path).await?;
            if events.is_empty() {
                continue;
            }
            let run_id = events[0].run_id.clone();
            if let Some(summary) = summarize_run_events(&run_id, &events) {
                runs.push(summary);
            }
        }
        runs.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.run_id.0.cmp(&right.run_id.0))
        });
        Ok(runs)
    }

    async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>> {
        let mut runs = Vec::new();
        let mut entries = fs::read_dir(&self.root_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let events = self.load_events_from_path(&path).await?;
            if events.is_empty() {
                continue;
            }
            let run_id = events[0].run_id.clone();
            let Some(summary) = summarize_run_events(&run_id, &events) else {
                continue;
            };
            if let Some(result) = search_run_events(&summary, &events, query) {
                runs.push(result);
            }
        }
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
                .then_with(|| left.summary.run_id.0.cmp(&right.summary.run_id.0))
        });
        Ok(runs)
    }

    async fn events(&self, run_id: &RunId) -> Result<Vec<RunEventEnvelope>> {
        let path = self.run_path(run_id);
        if !fs::try_exists(&path).await? {
            return Err(RunStoreError::RunNotFound(run_id.0.clone()));
        }
        self.load_events_from_path(&path).await
    }

    async fn session_ids(&self, run_id: &RunId) -> Result<Vec<SessionId>> {
        let mut seen = HashSet::new();
        let mut ordered = Vec::new();
        for event in self.events(run_id).await? {
            if seen.insert(event.session_id.0.clone()) {
                ordered.push(event.session_id);
            }
        }
        Ok(ordered)
    }

    async fn replay_transcript(&self, run_id: &RunId) -> Result<Vec<Message>> {
        Ok(replay_transcript(&self.events(run_id).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::FileRunStore;
    use crate::{EventSink, RunStore};
    use agent_core_types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId};

    #[tokio::test]
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
    }

    #[tokio::test]
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

    #[tokio::test]
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

    #[tokio::test]
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
}
