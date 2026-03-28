use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunMemoryExportBundle, RunMemoryExportRequest, RunSearchResult, RunStore,
    RunStoreError, RunSummary, apply_memory_export_request, build_memory_export_record,
    search_run_events, summarize_run_events,
};
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::BTreeMap;
use std::sync::Arc;
use types::{Message, RunEventEnvelope, RunId, SessionId};

#[derive(Clone, Default)]
pub struct InMemoryRunStore {
    // Run streams append and enumerate concurrently during transcript replay,
    // search, and memory export. A sharded map removes the global store lock
    // while keeping each run's event vector behavior unchanged.
    events: Arc<DashMap<RunId, Vec<RunEventEnvelope>>>,
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
        self.events
            .entry(event.run_id.clone())
            .or_default()
            .push(event);
        Ok(())
    }
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let mut runs = self
            .events
            .iter()
            .filter_map(|entry| summarize_run_events(entry.key(), entry.value()))
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
        let mut runs = self
            .events
            .iter()
            .filter_map(|entry| {
                let summary = summarize_run_events(entry.key(), entry.value())?;
                search_run_events(&summary, entry.value(), query)
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
        self.events
            .get(run_id)
            .map(|entry| entry.value().clone())
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
    ) -> Result<RunMemoryExportBundle> {
        let mut bundle = RunMemoryExportBundle::default();

        for entry in self.events.iter() {
            if let Some(record) = build_memory_export_record(
                crate::MemoryExportScope::Run,
                entry.key(),
                None,
                None,
                None,
                entry.value(),
            ) {
                bundle.runs.push(record);
            }

            for (session_id, events) in session_event_groups(entry.value()) {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Session,
                    entry.key(),
                    Some(session_id),
                    None,
                    None,
                    &events,
                ) {
                    bundle.sessions.push(record);
                }
            }
        }

        sort_memory_export_records(&mut bundle.runs);
        sort_memory_export_records(&mut bundle.sessions);
        apply_memory_export_request(&mut bundle, &request);
        Ok(bundle)
    }
}

fn session_event_groups(events: &[RunEventEnvelope]) -> Vec<(SessionId, Vec<RunEventEnvelope>)> {
    let mut grouped = BTreeMap::<SessionId, Vec<RunEventEnvelope>>::new();
    for event in events {
        grouped
            .entry(event.session_id.clone())
            .or_default()
            .push(event.clone());
    }
    grouped.into_iter().collect()
}

fn sort_memory_export_records(records: &mut [crate::RunMemoryExportRecord]) {
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
                session_id.clone(),
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
        assert_eq!(exports.runs.len(), 1);
        assert_eq!(exports.runs[0].summary.run_id, run_id);
        assert!(exports.runs[0].search_corpus.contains("deploy release"));
        assert_eq!(exports.sessions.len(), 1);
        assert_eq!(
            exports.sessions[0].summary.session_id.as_ref(),
            Some(&session_id)
        );
    }
}
