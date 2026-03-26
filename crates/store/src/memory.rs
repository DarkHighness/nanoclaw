use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunMemoryExportRecord, RunMemoryExportRequest, RunSearchResult, RunStore,
    RunStoreError, RunSummary, append_search_corpus_line, keep_recent_chars, search_run_events,
    searchable_event_strings, summarize_run_events,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use types::{Message, RunEventEnvelope, RunId, SessionId};

#[derive(Clone, Default)]
pub struct InMemoryRunStore {
    events: Arc<RwLock<HashMap<RunId, Vec<RunEventEnvelope>>>>,
}

impl InMemoryRunStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventSink for InMemoryRunStore {
    async fn append(&self, event: RunEventEnvelope) -> Result<()> {
        let mut guard = self.events.write().expect("in-memory run store write lock");
        guard.entry(event.run_id.clone()).or_default().push(event);
        Ok(())
    }
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let guard = self.events.read().expect("in-memory run store read lock");
        let mut runs = guard
            .iter()
            .filter_map(|(run_id, events)| summarize_run_events(run_id, events))
            .collect::<Vec<_>>();
        runs.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.run_id.as_str().cmp(right.run_id.as_str()))
        });
        Ok(runs)
    }

    async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>> {
        let guard = self.events.read().expect("in-memory run store read lock");
        let mut runs = guard
            .iter()
            .filter_map(|(run_id, events)| {
                let summary = summarize_run_events(run_id, events)?;
                search_run_events(&summary, events, query)
            })
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
        let guard = self.events.read().expect("in-memory run store read lock");
        guard
            .get(run_id)
            .cloned()
            .ok_or_else(|| RunStoreError::RunNotFound(run_id.clone()))
    }

    async fn session_ids(&self, run_id: &RunId) -> Result<Vec<SessionId>> {
        let events = self.events(run_id).await?;
        let mut seen = Vec::new();
        for event in events {
            if !seen
                .iter()
                .any(|value: &SessionId| value == &event.session_id)
            {
                seen.push(event.session_id);
            }
        }
        Ok(seen)
    }

    async fn replay_transcript(&self, run_id: &RunId) -> Result<Vec<Message>> {
        Ok(replay_transcript(&self.events(run_id).await?))
    }

    async fn export_for_memory(
        &self,
        request: RunMemoryExportRequest,
    ) -> Result<Vec<RunMemoryExportRecord>> {
        let guard = self.events.read().expect("in-memory run store read lock");
        let mut records = guard
            .iter()
            .filter_map(|(run_id, events)| {
                Some(RunMemoryExportRecord {
                    summary: summarize_run_events(run_id, events)?,
                    session_ids: collect_session_ids(events),
                    search_corpus: build_search_corpus(events),
                })
            })
            .collect::<Vec<_>>();
        sort_memory_export_records(&mut records);

        if let Some(max_runs) = request.max_runs {
            records.truncate(max_runs);
        }
        if let Some(max_chars) = request.max_search_corpus_chars {
            for record in &mut records {
                record.search_corpus = keep_recent_chars(&record.search_corpus, max_chars);
            }
        }
        Ok(records)
    }
}

fn collect_session_ids(events: &[RunEventEnvelope]) -> Vec<SessionId> {
    let mut seen = Vec::new();
    for event in events {
        if !seen
            .iter()
            .any(|value: &SessionId| value == &event.session_id)
        {
            seen.push(event.session_id.clone());
        }
    }
    seen
}

fn build_search_corpus(events: &[RunEventEnvelope]) -> String {
    let mut corpus = String::new();
    for event in events {
        for value in searchable_event_strings(event) {
            append_search_corpus_line(&mut corpus, &value);
        }
    }
    corpus
}

fn sort_memory_export_records(records: &mut [RunMemoryExportRecord]) {
    records.sort_by(|left, right| {
        right
            .summary
            .last_timestamp_ms
            .cmp(&left.summary.last_timestamp_ms)
            .then_with(|| {
                left.summary
                    .run_id
                    .as_str()
                    .cmp(right.summary.run_id.as_str())
            })
    });
}

#[cfg(test)]
mod tests {
    use super::InMemoryRunStore;
    use crate::{EventSink, RunMemoryExportRequest, RunStore};
    use types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId};

    #[tokio::test]
    async fn replays_basic_transcript() {
        let store = InMemoryRunStore::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
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
        store
            .append(RunEventEnvelope::new(
                run_id.clone(),
                session_id,
                None,
                None,
                RunEventKind::TranscriptMessage {
                    message: Message::assistant("world"),
                },
            ))
            .await
            .unwrap();

        let transcript = store.replay_transcript(&run_id).await.unwrap();
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].text_content(), "hello");
        assert_eq!(transcript[1].text_content(), "world");
    }

    #[tokio::test]
    async fn lists_runs_with_latest_first_and_prompt_preview() {
        let store = InMemoryRunStore::new();
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
    async fn searches_runs_by_prompt_or_transcript() {
        let store = InMemoryRunStore::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
        store
            .append(RunEventEnvelope::new(
                run_id.clone(),
                session_id.clone(),
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "prepare release".to_string(),
                },
            ))
            .await
            .unwrap();
        store
            .append(RunEventEnvelope::new(
                run_id.clone(),
                session_id,
                None,
                None,
                RunEventKind::TranscriptMessage {
                    message: Message::assistant("release checklist"),
                },
            ))
            .await
            .unwrap();

        let matches = store.search_runs("release").await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].summary.run_id, run_id);
        assert!(
            matches[0]
                .preview_matches
                .iter()
                .any(|line| line.contains("release"))
        );
    }

    #[tokio::test]
    async fn exports_runs_for_memory_newest_first() {
        let store = InMemoryRunStore::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
        store
            .append(RunEventEnvelope::new(
                run_id.clone(),
                session_id,
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "deploy release".to_string(),
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
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].summary.run_id, run_id);
        assert!(exports[0].search_corpus.contains("deploy release"));
    }
}
