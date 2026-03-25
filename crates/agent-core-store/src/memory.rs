use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunSearchResult, RunStore, RunStoreError, RunSummary, search_run_events,
    summarize_run_events,
};
use agent_core_types::{Message, RunEventEnvelope, RunId, SessionId};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct InMemoryRunStore {
    events: Arc<RwLock<HashMap<String, Vec<RunEventEnvelope>>>>,
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
        let mut guard = self.events.write().await;
        guard.entry(event.run_id.0.clone()).or_default().push(event);
        Ok(())
    }
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let guard = self.events.read().await;
        let mut runs = guard
            .iter()
            .filter_map(|(run_id, events)| summarize_run_events(&RunId(run_id.clone()), events))
            .collect::<Vec<_>>();
        runs.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.run_id.0.cmp(&right.run_id.0))
        });
        Ok(runs)
    }

    async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>> {
        let guard = self.events.read().await;
        let mut runs = guard
            .iter()
            .filter_map(|(run_id, events)| {
                let summary = summarize_run_events(&RunId(run_id.clone()), events)?;
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
                .then_with(|| left.summary.run_id.0.cmp(&right.summary.run_id.0))
        });
        Ok(runs)
    }

    async fn events(&self, run_id: &RunId) -> Result<Vec<RunEventEnvelope>> {
        let guard = self.events.read().await;
        guard
            .get(&run_id.0)
            .cloned()
            .ok_or_else(|| RunStoreError::RunNotFound(run_id.0.clone()))
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
}

#[cfg(test)]
mod tests {
    use super::InMemoryRunStore;
    use crate::{EventSink, RunStore};
    use agent_core_types::{Message, RunEventEnvelope, RunEventKind, RunId, SessionId};

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
}
