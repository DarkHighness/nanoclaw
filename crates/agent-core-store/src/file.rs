use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunSearchResult, RunStore, RunStoreError, RunSummary, search_run_events,
    searchable_event_strings, summarize_run_events,
};
use agent_core_types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

const INDEX_FILE_NAME: &str = "runs.index.json";
const INDEX_VERSION: u32 = 1;
const MAX_SEARCH_CORPUS_CHARS: usize = 16_384;

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
    index: Arc<Mutex<FileRunStoreIndex>>,
    options: FileRunStoreOptions,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexedRunRecord {
    summary: RunSummary,
    session_ids: BTreeSet<String>,
    // The sidecar keeps only a bounded search corpus for prefiltering. The
    // append-only JSONL transcript remains the source of truth for replay and
    // exact preview generation.
    search_corpus: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileRunStoreIndex {
    version: u32,
    runs: BTreeMap<String, IndexedRunRecord>,
}

impl Default for FileRunStoreIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            runs: BTreeMap::new(),
        }
    }
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
        fs::create_dir_all(&root_dir).await?;
        let mut index = load_or_rebuild_index(&root_dir).await?;
        let pruned = select_runs_to_prune(&index, &options.retention, current_timestamp_ms());
        for run_id in &pruned {
            delete_run_file(&root_dir, run_id).await?;
            index.runs.remove(run_id);
        }
        persist_index_file(&root_dir, &index).await?;
        Ok(Self {
            root_dir,
            write_lock: Arc::new(Mutex::new(())),
            index: Arc::new(Mutex::new(index)),
            options,
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

        let mut index = self.index.lock().await;
        let record = index
            .runs
            .entry(event.run_id.0.clone())
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
                session_ids: BTreeSet::new(),
                search_corpus: String::new(),
            });
        apply_event_to_record(record, &event);

        let pruned = select_runs_to_prune(&index, &self.options.retention, current_timestamp_ms());
        for run_id in &pruned {
            delete_run_file(&self.root_dir, run_id).await?;
            index.runs.remove(run_id);
        }
        self.persist_index(&index).await?;
        Ok(())
    }
}

#[async_trait]
impl RunStore for FileRunStore {
    async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let index = self.index.lock().await;
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
            let index = self.index.lock().await;
            index
                .runs
                .values()
                .filter(|record| record_matches_query(record, &query_lower))
                .map(|record| record.summary.clone())
                .collect::<Vec<_>>()
        };

        let mut runs = Vec::new();
        for summary in candidate_summaries {
            let events = self.events(&summary.run_id).await?;
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
        let index = self.index.lock().await;
        if let Some(record) = index.runs.get(&run_id.0) {
            return Ok(record
                .session_ids
                .iter()
                .cloned()
                .map(SessionId)
                .collect::<Vec<_>>());
        }

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

fn apply_event_to_record(record: &mut IndexedRunRecord, event: &RunEventEnvelope) {
    record.summary.first_timestamp_ms = record.summary.first_timestamp_ms.min(event.timestamp_ms);
    record.summary.last_timestamp_ms = record.summary.last_timestamp_ms.max(event.timestamp_ms);
    record.summary.event_count += 1;
    if record.session_ids.insert(event.session_id.0.clone()) {
        record.summary.session_count = record.session_ids.len();
    }
    if matches!(&event.event, RunEventKind::TranscriptMessage { .. }) {
        record.summary.transcript_message_count += 1;
    }
    if let RunEventKind::UserPromptSubmit { prompt } = &event.event {
        record.summary.last_user_prompt = Some(prompt.clone());
    }
    for value in searchable_event_strings(event) {
        append_search_text(&mut record.search_corpus, &value);
    }
}

fn append_search_text(search_corpus: &mut String, value: &str) {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return;
    }
    if !search_corpus.is_empty() {
        search_corpus.push('\n');
    }
    search_corpus.push_str(&normalized);
    let total_chars = search_corpus.chars().count();
    if total_chars > MAX_SEARCH_CORPUS_CHARS {
        let keep_from = total_chars - MAX_SEARCH_CORPUS_CHARS;
        *search_corpus = search_corpus.chars().skip(keep_from).collect::<String>();
    }
}

fn record_matches_query(record: &IndexedRunRecord, query_lower: &str) -> bool {
    record.summary.run_id.0.to_lowercase().contains(query_lower)
        || record
            .summary
            .last_user_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.to_lowercase().contains(query_lower))
        || record.search_corpus.to_lowercase().contains(query_lower)
}

fn sort_run_summaries(runs: &mut [RunSummary]) {
    runs.sort_by(|left, right| {
        right
            .last_timestamp_ms
            .cmp(&left.last_timestamp_ms)
            .then_with(|| left.run_id.0.cmp(&right.run_id.0))
    });
}

fn select_runs_to_prune(
    index: &FileRunStoreIndex,
    retention: &RunStoreRetentionPolicy,
    now_ms: u128,
) -> Vec<String> {
    let mut prune = BTreeSet::new();
    if let Some(max_age) = retention.max_age {
        let max_age_ms = max_age.as_millis();
        for (run_id, record) in &index.runs {
            if now_ms.saturating_sub(record.summary.last_timestamp_ms) > max_age_ms {
                prune.insert(run_id.clone());
            }
        }
    }
    if let Some(max_runs) = retention.max_runs {
        let mut remaining = index
            .runs
            .iter()
            .filter(|(run_id, _)| !prune.contains(*run_id))
            .map(|(run_id, record)| (run_id.clone(), record.summary.last_timestamp_ms))
            .collect::<Vec<_>>();
        remaining.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        for (run_id, _) in remaining.into_iter().skip(max_runs) {
            prune.insert(run_id);
        }
    }
    prune.into_iter().collect()
}

async fn load_or_rebuild_index(root_dir: &Path) -> Result<FileRunStoreIndex> {
    let run_files = list_run_file_ids(root_dir).await?;
    let index_path = root_dir.join(INDEX_FILE_NAME);
    if fs::try_exists(&index_path).await? {
        let contents = fs::read_to_string(&index_path).await?;
        if let Ok(index) = serde_json::from_str::<FileRunStoreIndex>(&contents) {
            let indexed_runs = index.runs.keys().cloned().collect::<BTreeSet<_>>();
            // A stale or partially-written sidecar should not hide durable run
            // transcripts. Rebuild whenever the file set diverges.
            if index.version == INDEX_VERSION && indexed_runs == run_files {
                return Ok(index);
            }
        }
    }
    rebuild_index(root_dir, run_files).await
}

async fn rebuild_index(root_dir: &Path, run_files: BTreeSet<String>) -> Result<FileRunStoreIndex> {
    let mut index = FileRunStoreIndex::default();
    for run_id in run_files {
        let path = root_dir.join(format!("{run_id}.jsonl"));
        let events = load_events_from_path(&path).await?;
        if let Some(record) = indexed_record_from_events(events) {
            index.runs.insert(run_id, record);
        }
    }
    Ok(index)
}

async fn load_events_from_path(path: &Path) -> Result<Vec<RunEventEnvelope>> {
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

fn indexed_record_from_events(events: Vec<RunEventEnvelope>) -> Option<IndexedRunRecord> {
    let run_id = events.first()?.run_id.clone();
    let summary = summarize_run_events(&run_id, &events)?;
    let session_ids = events
        .iter()
        .map(|event| event.session_id.0.clone())
        .collect::<BTreeSet<_>>();
    let mut search_corpus = String::new();
    for event in &events {
        for value in searchable_event_strings(event) {
            append_search_text(&mut search_corpus, &value);
        }
    }
    Some(IndexedRunRecord {
        summary,
        session_ids,
        search_corpus,
    })
}

async fn list_run_file_ids(root_dir: &Path) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    let mut entries = fs::read_dir(root_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.file_name().and_then(|value| value.to_str()) == Some(INDEX_FILE_NAME) {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
            ids.insert(stem.to_string());
        }
    }
    Ok(ids)
}

async fn persist_index_file(root_dir: &Path, index: &FileRunStoreIndex) -> Result<()> {
    let path = root_dir.join(INDEX_FILE_NAME);
    let temp_path = root_dir.join(format!("{INDEX_FILE_NAME}.tmp"));
    let encoded = serde_json::to_vec_pretty(index)?;
    fs::write(&temp_path, encoded).await?;
    fs::rename(&temp_path, &path).await?;
    Ok(())
}

async fn delete_run_file(root_dir: &Path, run_id: &str) -> Result<()> {
    let path = root_dir.join(format!("{run_id}.jsonl"));
    if fs::try_exists(&path).await? {
        fs::remove_file(path).await?;
    }
    Ok(())
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
        FileRunStore, FileRunStoreOptions, RunStoreRetentionPolicy, append_search_text,
        current_timestamp_ms,
    };
    use crate::{EventSink, RunStore};
    use agent_core_types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId};
    use std::time::Duration;

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
        assert_eq!(reopened.list_runs().await.unwrap().len(), 1);
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

    #[tokio::test]
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

    #[tokio::test]
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

    #[test]
    fn search_corpus_is_capped_to_recent_text() {
        let mut corpus = String::new();
        append_search_text(&mut corpus, &"x".repeat(20_000));
        assert!(corpus.chars().count() <= 16_384);
    }
}
