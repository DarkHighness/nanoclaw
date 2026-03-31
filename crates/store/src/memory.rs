use crate::replay::replay_transcript;
use crate::{
    EventSink, Result, SessionMemoryExportBundle, SessionMemoryExportRequest, SessionSearchResult,
    SessionStore, SessionStoreError, SessionSummary, apply_memory_export_request,
    build_memory_export_record, group_events_for_memory_export, search_session_events,
    sort_memory_export_records, summarize_session_events,
};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use types::{AgentSessionId, Message, SessionEventEnvelope, SessionId};

#[derive(Clone, Default)]
pub struct InMemorySessionStore {
    // Run streams append and enumerate concurrently during transcript replay,
    // search, and memory export. A sharded map removes the global store lock
    // while keeping each session's event vector behavior unchanged.
    events: Arc<DashMap<SessionId, Vec<SessionEventEnvelope>>>,
}

impl InMemorySessionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventSink for InMemorySessionStore {
    async fn append(&self, event: SessionEventEnvelope) -> Result<()> {
        self.events
            .entry(event.session_id.clone())
            .or_default()
            .push(event);
        Ok(())
    }

    async fn append_batch(&self, events: Vec<SessionEventEnvelope>) -> Result<()> {
        for event in events {
            self.events
                .entry(event.session_id.clone())
                .or_default()
                .push(event);
        }
        Ok(())
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let mut sessions = self
            .events
            .iter()
            .filter_map(|entry| summarize_session_events(entry.key(), entry.value()))
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.session_id.as_str().cmp(right.session_id.as_str()))
        });
        Ok(sessions)
    }

    async fn search_sessions(&self, query: &str) -> Result<Vec<SessionSearchResult>> {
        let mut sessions = self
            .events
            .iter()
            .filter_map(|entry| {
                let summary = summarize_session_events(entry.key(), entry.value())?;
                search_session_events(&summary, entry.value(), query)
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
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
                        .session_id
                        .as_str()
                        .cmp(right.summary.session_id.as_str())
                })
        });
        Ok(sessions)
    }

    async fn events(&self, session_id: &SessionId) -> Result<Vec<SessionEventEnvelope>> {
        self.events
            .get(session_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| SessionStoreError::SessionNotFound(session_id.clone()))
    }

    async fn agent_session_ids(&self, session_id: &SessionId) -> Result<Vec<AgentSessionId>> {
        let events = self.events(session_id).await?;
        let mut seen = Vec::new();
        for event in events {
            if !seen
                .iter()
                .any(|value: &AgentSessionId| value == &event.agent_session_id)
            {
                seen.push(event.agent_session_id);
            }
        }
        Ok(seen)
    }

    async fn replay_transcript(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        Ok(replay_transcript(&self.events(session_id).await?))
    }

    async fn export_for_memory(
        &self,
        request: SessionMemoryExportRequest,
    ) -> Result<SessionMemoryExportBundle> {
        let mut bundle = SessionMemoryExportBundle::default();

        for entry in self.events.iter() {
            if let Some(record) = build_memory_export_record(
                crate::MemoryExportScope::Session,
                entry.key(),
                None,
                None,
                None,
                entry.value(),
            ) {
                bundle.sessions.push(record);
            }

            let groups = group_events_for_memory_export(entry.value());

            for (agent_session_id, events) in groups.agent_sessions {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::AgentSession,
                    entry.key(),
                    Some(agent_session_id),
                    None,
                    None,
                    &events,
                ) {
                    bundle.agent_sessions.push(record);
                }
            }

            for group in groups.subagents {
                if let Some(record) = build_memory_export_record(
                    crate::MemoryExportScope::Subagent,
                    entry.key(),
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
                    entry.key(),
                    group.agent_session_id,
                    None,
                    group.task_id,
                    &group.events,
                ) {
                    bundle.tasks.push(record);
                }
            }
        }

        sort_memory_export_records(&mut bundle.sessions);
        sort_memory_export_records(&mut bundle.agent_sessions);
        sort_memory_export_records(&mut bundle.subagents);
        sort_memory_export_records(&mut bundle.tasks);
        apply_memory_export_request(&mut bundle, &request);
        Ok(bundle)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemorySessionStore;
    use crate::{EventSink, SessionMemoryExportRequest, SessionStore};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use types::{
        AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
        AgentSessionId, AgentStatus, AgentTaskSpec, ContextWindowUsage, Message,
        SessionEventEnvelope, SessionEventKind, SessionId, TokenLedgerSnapshot, TokenUsage,
        TokenUsagePhase,
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
            let store = InMemorySessionStore::new();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
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
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id,
                    None,
                    None,
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("world"),
                    },
                ))
                .await
                .unwrap();

            let transcript = store.replay_transcript(&session_id).await.unwrap();
            assert_eq!(transcript.len(), 2);
            assert_eq!(transcript[0].text_content(), "hello");
            assert_eq!(transcript[1].text_content(), "world");
        }
    );

    bounded_async_test!(
        async fn lists_runs_with_latest_first_and_prompt_preview() {
            let store = InMemorySessionStore::new();
            let agent_session_id = AgentSessionId::new();
            let older_run = SessionId::new();
            let newer_run = SessionId::new();
            let mut older_event = SessionEventEnvelope::new(
                older_run.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: "older".to_string(),
                },
            );
            older_event.timestamp_ms = 1;
            let mut newer_event = SessionEventEnvelope::new(
                newer_run.clone(),
                agent_session_id,
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: "newer".to_string(),
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
        async fn searches_runs_by_prompt_or_transcript() {
            let store = InMemorySessionStore::new();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: "prepare release".to_string(),
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
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("release checklist"),
                    },
                ))
                .await
                .unwrap();

            let matches = store.search_sessions("release").await.unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].summary.session_id, session_id);
            assert!(
                matches[0]
                    .preview_matches
                    .iter()
                    .any(|line| line.contains("release"))
            );
        }
    );

    bounded_async_test!(
        async fn search_skips_hidden_compacted_transcript_text() {
            let store = InMemorySessionStore::new();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            let kept =
                Message::user("keep this").with_message_id(types::MessageId::from("msg_keep"));
            let summary =
                Message::system("summary").with_message_id(types::MessageId::from("msg_summary"));
            let after = Message::assistant("after compaction")
                .with_message_id(types::MessageId::from("msg_after"));

            store
                .append_batch(vec![
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::user("older prompt")
                                .with_message_id(types::MessageId::from("msg_older_prompt")),
                        },
                    ),
                    SessionEventEnvelope::new(
                        session_id.clone(),
                        agent_session_id.clone(),
                        None,
                        None,
                        SessionEventKind::TranscriptMessage {
                            message: Message::assistant("older answer")
                                .with_message_id(types::MessageId::from("msg_older_answer")),
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
        }
    );

    bounded_async_test!(
        async fn exports_runs_for_memory_newest_first() {
            let store = InMemorySessionStore::new();
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id.clone(),
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: "deploy release".to_string(),
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
            assert!(exports.sessions[0].search_corpus.contains("deploy release"));
            assert_eq!(exports.agent_sessions.len(), 1);
            assert_eq!(
                exports.agent_sessions[0].summary.agent_session_id.as_ref(),
                Some(&agent_session_id)
            );
        }
    );

    bounded_async_test!(
        async fn exports_subagent_and_task_runtime_records() {
            let store = InMemorySessionStore::new();
            let session_id = SessionId::new();
            let parent_agent_session_id = AgentSessionId::new();
            let child_session_id = SessionId::new();
            let child_agent_session_id = AgentSessionId::new();
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
                session_id: child_session_id.clone(),
                agent_session_id: child_agent_session_id.clone(),
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
                SessionEventKind::UserPromptSubmit {
                    prompt: "review the latest patch".to_string(),
                },
                SessionEventKind::TaskCreated {
                    task: task.clone(),
                    parent_agent_id: None,
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
                    status: AgentStatus::Completed,
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

    bounded_async_test!(
        async fn reports_root_and_subagent_token_usage() {
            let store = InMemorySessionStore::new();
            let session_id = SessionId::new();
            let parent_agent_session_id = AgentSessionId::new();
            let rotated_parent_agent_session_id = AgentSessionId::new();
            let child_session_id = SessionId::new();
            let child_agent_session_id = AgentSessionId::new();
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
        }
    );
}
