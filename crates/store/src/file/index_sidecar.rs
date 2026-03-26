use super::RunStoreRetentionPolicy;
use crate::{
    Result, RunSummary, append_search_corpus_line, keep_recent_chars, searchable_event_strings,
    summarize_run_events,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use types::{RunEventEnvelope, RunEventKind, RunId, SessionId};

pub(super) const INDEX_FILE_NAME: &str = "runs.index.json";
const INDEX_VERSION: u32 = 2;
const MAX_SEARCH_CORPUS_CHARS: usize = 16_384;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct IndexedRunRecord {
    pub(super) summary: RunSummary,
    // Session ids must preserve first-seen order because hosts may use the
    // sequence to reconstruct hand-offs across attached sessions. A sorted set
    // makes the durable transcript look different from the original stream.
    pub(super) session_ids: Vec<SessionId>,
    // The sidecar keeps only a bounded search corpus for prefiltering. The
    // append-only JSONL transcript remains the source of truth for replay and
    // exact preview generation.
    pub(super) search_corpus: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct FileRunStoreIndex {
    pub(super) version: u32,
    pub(super) runs: std::collections::BTreeMap<RunId, IndexedRunRecord>,
}

impl Default for FileRunStoreIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            runs: std::collections::BTreeMap::new(),
        }
    }
}

pub(super) fn apply_event_to_record(record: &mut IndexedRunRecord, event: &RunEventEnvelope) {
    record.summary.first_timestamp_ms = record.summary.first_timestamp_ms.min(event.timestamp_ms);
    record.summary.last_timestamp_ms = record.summary.last_timestamp_ms.max(event.timestamp_ms);
    record.summary.event_count += 1;
    if push_unique_session_id(&mut record.session_ids, &event.session_id) {
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

pub(super) fn append_search_text(search_corpus: &mut String, value: &str) {
    append_search_corpus_line(search_corpus, value);
    let total_chars = search_corpus.chars().count();
    if total_chars > MAX_SEARCH_CORPUS_CHARS {
        *search_corpus = keep_recent_chars(search_corpus, MAX_SEARCH_CORPUS_CHARS);
    }
}

pub(super) fn record_matches_query(record: &IndexedRunRecord, query_lower: &str) -> bool {
    record
        .summary
        .run_id
        .as_str()
        .to_lowercase()
        .contains(query_lower)
        || record
            .summary
            .last_user_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.to_lowercase().contains(query_lower))
        || record.search_corpus.to_lowercase().contains(query_lower)
}

pub(super) fn select_runs_to_prune(
    index: &FileRunStoreIndex,
    retention: &RunStoreRetentionPolicy,
    now_ms: u128,
) -> Vec<RunId> {
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

pub(super) async fn load_or_rebuild_index(root_dir: &Path) -> Result<FileRunStoreIndex> {
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

pub(super) async fn load_events_from_path(path: &Path) -> Result<Vec<RunEventEnvelope>> {
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

pub(super) async fn persist_index_file(root_dir: &Path, index: &FileRunStoreIndex) -> Result<()> {
    let path = root_dir.join(INDEX_FILE_NAME);
    let temp_path = root_dir.join(format!("{INDEX_FILE_NAME}.tmp"));
    let encoded = serde_json::to_vec_pretty(index)?;
    fs::write(&temp_path, encoded).await?;
    fs::rename(&temp_path, &path).await?;
    Ok(())
}

pub(super) async fn delete_run_file(root_dir: &Path, run_id: &RunId) -> Result<()> {
    let path = root_dir.join(format!("{run_id}.jsonl"));
    if fs::try_exists(&path).await? {
        fs::remove_file(path).await?;
    }
    Ok(())
}

fn push_unique_session_id(session_ids: &mut Vec<SessionId>, session_id: &SessionId) -> bool {
    if session_ids.iter().any(|existing| existing == session_id) {
        return false;
    }
    session_ids.push(session_id.clone());
    true
}

async fn rebuild_index(root_dir: &Path, run_files: BTreeSet<RunId>) -> Result<FileRunStoreIndex> {
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

fn indexed_record_from_events(events: Vec<RunEventEnvelope>) -> Option<IndexedRunRecord> {
    let run_id = events.first()?.run_id.clone();
    let summary = summarize_run_events(&run_id, &events)?;
    let mut session_ids = Vec::new();
    for event in &events {
        push_unique_session_id(&mut session_ids, &event.session_id);
    }
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

async fn list_run_file_ids(root_dir: &Path) -> Result<BTreeSet<RunId>> {
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
            ids.insert(RunId::from(stem));
        }
    }
    Ok(ids)
}
