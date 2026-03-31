use super::SessionStoreRetentionPolicy;
use crate::{
    Result, SessionSummary, append_search_corpus_line, build_search_corpus, keep_recent_chars,
    message_search_text, searchable_session_event_strings, summarize_session_events,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use types::{AgentSessionId, SessionEventEnvelope, SessionEventKind, SessionId};

pub(super) const INDEX_FILE_NAME: &str = "sessions.index.json";
// Visible transcript counts now fold compaction checkpoints, so older sidecars
// that stored raw replay counts must rebuild on open.
const INDEX_VERSION: u32 = 3;
const MAX_SEARCH_CORPUS_CHARS: usize = 16_384;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct IndexedSessionRecord {
    pub(super) summary: SessionSummary,
    // Session ids must preserve first-seen order because hosts may use the
    // sequence to reconstruct hand-offs across attached sessions. A sorted set
    // makes the durable transcript look different from the original stream.
    pub(super) agent_session_ids: Vec<AgentSessionId>,
    // The sidecar keeps only a bounded search corpus for prefiltering. The
    // append-only JSONL transcript remains the source of truth for replay and
    // exact preview generation.
    pub(super) search_corpus: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct FileSessionStoreIndex {
    pub(super) version: u32,
    pub(super) sessions: std::collections::BTreeMap<SessionId, IndexedSessionRecord>,
}

impl Default for FileSessionStoreIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            sessions: std::collections::BTreeMap::new(),
        }
    }
}

pub(super) fn apply_event_to_record(
    record: &mut IndexedSessionRecord,
    event: &SessionEventEnvelope,
) {
    record.summary.first_timestamp_ms = record.summary.first_timestamp_ms.min(event.timestamp_ms);
    record.summary.last_timestamp_ms = record.summary.last_timestamp_ms.max(event.timestamp_ms);
    record.summary.event_count += 1;
    if push_unique_session_id(&mut record.agent_session_ids, &event.agent_session_id) {
        record.summary.agent_session_count = record.agent_session_ids.len();
    }
    if matches!(&event.event, SessionEventKind::TranscriptMessage { .. }) {
        record.summary.transcript_message_count += 1;
    }
    if let SessionEventKind::UserPromptSubmit { prompt } = &event.event {
        record.summary.last_user_prompt = Some(prompt.clone());
    }
    match &event.event {
        SessionEventKind::TranscriptMessage { message } => {
            append_search_text(&mut record.search_corpus, &message_search_text(message));
        }
        SessionEventKind::TranscriptMessagePatched { .. }
        | SessionEventKind::TranscriptMessageRemoved { .. } => {}
        _ => {
            for value in searchable_session_event_strings(event) {
                append_search_text(&mut record.search_corpus, &value);
            }
        }
    }
}

pub(super) fn append_search_text(search_corpus: &mut String, value: &str) {
    append_search_corpus_line(search_corpus, value);
    let total_chars = search_corpus.chars().count();
    if total_chars > MAX_SEARCH_CORPUS_CHARS {
        *search_corpus = keep_recent_chars(search_corpus, MAX_SEARCH_CORPUS_CHARS);
    }
}

pub(super) fn record_matches_query(record: &IndexedSessionRecord, query_lower: &str) -> bool {
    record
        .summary
        .session_id
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

pub(super) fn select_sessions_to_prune(
    index: &FileSessionStoreIndex,
    retention: &SessionStoreRetentionPolicy,
    now_ms: u128,
) -> Vec<SessionId> {
    let mut prune = BTreeSet::new();
    if let Some(max_age) = retention.max_age {
        let max_age_ms = max_age.as_millis();
        for (session_id, record) in &index.sessions {
            if now_ms.saturating_sub(record.summary.last_timestamp_ms) > max_age_ms {
                prune.insert(session_id.clone());
            }
        }
    }
    if let Some(max_sessions) = retention.max_sessions {
        let mut remaining = index
            .sessions
            .iter()
            .filter(|(session_id, _)| !prune.contains(*session_id))
            .map(|(session_id, record)| (session_id.clone(), record.summary.last_timestamp_ms))
            .collect::<Vec<_>>();
        remaining.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        for (session_id, _) in remaining.into_iter().skip(max_sessions) {
            prune.insert(session_id);
        }
    }
    prune.into_iter().collect()
}

pub(super) async fn load_or_rebuild_index(root_dir: &Path) -> Result<FileSessionStoreIndex> {
    let session_files = list_session_file_ids(root_dir).await?;
    let index_path = root_dir.join(INDEX_FILE_NAME);
    if fs::try_exists(&index_path).await? {
        let contents = fs::read_to_string(&index_path).await?;
        if let Ok(index) = serde_json::from_str::<FileSessionStoreIndex>(&contents) {
            let indexed_sessions = index.sessions.keys().cloned().collect::<BTreeSet<_>>();
            // A stale or partially-written sidecar should not hide durable
            // session transcripts. Rebuild whenever the file set diverges.
            if index.version == INDEX_VERSION && indexed_sessions == session_files {
                return Ok(index);
            }
        }
    }
    rebuild_index(root_dir, session_files).await
}

pub(super) async fn load_events_from_path(path: &Path) -> Result<Vec<SessionEventEnvelope>> {
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

pub(super) async fn persist_index_file(
    root_dir: &Path,
    index: &FileSessionStoreIndex,
) -> Result<()> {
    let path = root_dir.join(INDEX_FILE_NAME);
    let temp_path = root_dir.join(format!("{INDEX_FILE_NAME}.tmp"));
    let encoded = serde_json::to_vec_pretty(index)?;
    fs::write(&temp_path, encoded).await?;
    fs::rename(&temp_path, &path).await?;
    Ok(())
}

pub(super) async fn delete_session_file(root_dir: &Path, session_id: &SessionId) -> Result<()> {
    let path = root_dir.join(format!("{session_id}.jsonl"));
    if fs::try_exists(&path).await? {
        fs::remove_file(path).await?;
    }
    Ok(())
}

fn push_unique_session_id(
    agent_session_ids: &mut Vec<AgentSessionId>,
    agent_session_id: &AgentSessionId,
) -> bool {
    if agent_session_ids
        .iter()
        .any(|existing| existing == agent_session_id)
    {
        return false;
    }
    agent_session_ids.push(agent_session_id.clone());
    true
}

async fn rebuild_index(
    root_dir: &Path,
    session_files: BTreeSet<SessionId>,
) -> Result<FileSessionStoreIndex> {
    let mut index = FileSessionStoreIndex::default();
    for session_id in session_files {
        let path = root_dir.join(format!("{session_id}.jsonl"));
        let events = load_events_from_path(&path).await?;
        if let Some(record) = indexed_record_from_events(events) {
            index.sessions.insert(session_id, record);
        }
    }
    Ok(index)
}

pub(super) fn indexed_record_from_events(
    events: Vec<SessionEventEnvelope>,
) -> Option<IndexedSessionRecord> {
    let session_id = events.first()?.session_id.clone();
    let summary = summarize_session_events(&session_id, &events)?;
    let mut agent_session_ids = Vec::new();
    for event in &events {
        push_unique_session_id(&mut agent_session_ids, &event.agent_session_id);
    }
    Some(IndexedSessionRecord {
        summary,
        agent_session_ids,
        search_corpus: build_search_corpus(&events),
    })
}

async fn list_session_file_ids(root_dir: &Path) -> Result<BTreeSet<SessionId>> {
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
            ids.insert(SessionId::from(stem));
        }
    }
    Ok(ids)
}
