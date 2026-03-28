use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, RunMemoryExportBundle, RunMemoryExportRequest, RunSearchResult, RunStore,
    RunStoreError, RunSummary, apply_memory_export_request, build_memory_export_record,
    group_events_for_memory_export, search_run_events, sort_memory_export_records,
    summarize_run_events,
};
use async_trait::async_trait;
use dashmap::DashMap;
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

    async fn append_batch(&self, events: Vec<RunEventEnvelope>) -> Result<()> {
        for event in events {
            self.events
                .entry(event.run_id.clone())
                .or_default()
                .push(event);
        }
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

            let groups = group_events_for_memory_export(entry.value());

            for (session_id, events) in groups.sessions {
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

            for group in groups.subagents {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Subagent,
                    entry.key(),
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
                    entry.key(),
                    group.session_id,
                    None,
                    group.task_id,
                    &group.events,
                ) {
                    bundle.tasks.push(record);
                }
            }
        }

        sort_memory_export_records(&mut bundle.runs);
        sort_memory_export_records(&mut bundle.sessions);
        sort_memory_export_records(&mut bundle.subagents);
        sort_memory_export_records(&mut bundle.tasks);
        apply_memory_export_request(&mut bundle, &request);
        Ok(bundle)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryRunStore;
    use crate::{EventSink, RunMemoryExportRequest, RunStore};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use types::{
        AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
        AgentStatus, AgentTaskSpec, ContextWindowUsage, Message, RunEventEnvelope, RunEventKind,
        RunId, SessionId, TokenLedgerSnapshot, TokenUsage, TokenUsagePhase,
    };

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    bounded_async_test!(
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
    );

    bounded_async_test!(
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
    );

    bounded_async_test!(
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
    );

    bounded_async_test!(
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
    );

    bounded_async_test!(
        async fn exports_subagent_and_task_runtime_records() {
            let store = InMemoryRunStore::new();
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

    bounded_async_test!(
        async fn reports_root_and_subagent_token_usage() {
            let store = InMemoryRunStore::new();
            let run_id = RunId::new();
            let parent_session_id = SessionId::new();
            let child_run_id = RunId::new();
            let child_session_id = SessionId::new();
            let agent_id = AgentId::new();
            let task = AgentTaskSpec {
                task_id: "task-usage".to_string(),
                role: "reviewer".to_string(),
                prompt: "review the patch".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            };

            store
                .append(RunEventEnvelope::new(
                    run_id.clone(),
                    parent_session_id.clone(),
                    None,
                    None,
                    RunEventKind::TokenUsageUpdated {
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
                .append(RunEventEnvelope::new(
                    run_id.clone(),
                    parent_session_id,
                    None,
                    None,
                    RunEventKind::SubagentStart {
                        handle: AgentHandle {
                            agent_id,
                            parent_agent_id: None,
                            run_id: child_run_id.clone(),
                            session_id: child_session_id.clone(),
                            task_id: task.task_id.clone(),
                            role: task.role.clone(),
                            status: AgentStatus::Running,
                        },
                        task,
                    },
                ))
                .await
                .unwrap();
            store
                .append(RunEventEnvelope::new(
                    child_run_id.clone(),
                    child_session_id.clone(),
                    None,
                    None,
                    RunEventKind::TokenUsageUpdated {
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

            let report = store.token_usage(&run_id).await.unwrap();
            assert_eq!(
                report
                    .run
                    .as_ref()
                    .map(|record| record.ledger.cumulative_usage),
                Some(TokenUsage::from_input_output(100, 20, 10))
            );
            assert_eq!(report.sessions.len(), 1);
            assert_eq!(report.subagents.len(), 1);
            assert_eq!(
                report.subagents[0].session_id.as_ref(),
                Some(&child_session_id)
            );
            assert_eq!(report.subagents[0].agent_name.as_deref(), Some("reviewer"));
            assert_eq!(report.tasks.len(), 1);
            assert_eq!(report.tasks[0].task_id.as_deref(), Some("task-usage"));
            assert_eq!(
                report.aggregate_usage,
                TokenUsage::from_input_output(140, 30, 15)
            );
        }
    );
}
