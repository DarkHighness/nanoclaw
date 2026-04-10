use super::{session_catalog::PersistedAgentSessionSummary, session_resume};
use agent::types::{
    AgentHandle, AgentSessionId, AgentStatus, AgentTaskSpec, Message, MessageRole,
    SessionEventEnvelope, SessionEventKind, SessionId,
};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{
    SessionSearchResult, SessionStore, SessionSummary, SessionTokenUsageReport, TokenUsageRecord,
    replay_transcript,
};

#[derive(Clone, Debug)]
pub(crate) struct LoadedSession {
    pub(crate) summary: SessionSummary,
    pub(crate) agent_session_ids: Vec<AgentSessionId>,
    pub(crate) transcript: Vec<Message>,
    pub(crate) events: Vec<SessionEventEnvelope>,
    pub(crate) token_usage: SessionTokenUsageReport,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedAgentSession {
    pub(crate) summary: PersistedAgentSessionSummary,
    pub(crate) transcript: Vec<Message>,
    pub(crate) events: Vec<SessionEventEnvelope>,
    pub(crate) token_usage: Option<TokenUsageRecord>,
    pub(crate) subagents: Vec<LoadedSubagentSession>,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadedSubagentSession {
    pub(crate) handle: AgentHandle,
    pub(crate) task: AgentTaskSpec,
    pub(crate) status: AgentStatus,
    pub(crate) summary: String,
    pub(crate) token_usage: Option<TokenUsageRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionExportKind {
    EventsJsonl,
    TranscriptText,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionExportArtifact {
    pub(crate) kind: SessionExportKind,
    pub(crate) session_id: SessionId,
    pub(crate) output_path: PathBuf,
    pub(crate) item_count: usize,
}

pub(crate) async fn list_sessions(store: &Arc<dyn SessionStore>) -> Result<Vec<SessionSummary>> {
    Ok(store.list_sessions().await?)
}

pub(crate) async fn search_sessions(
    store: &Arc<dyn SessionStore>,
    query: &str,
) -> Result<Vec<SessionSearchResult>> {
    Ok(store.search_sessions(query).await?)
}

pub(crate) async fn load_session(
    store: &Arc<dyn SessionStore>,
    session_ref: &str,
) -> Result<LoadedSession> {
    let (session_id, mut summary) = resolve_session(store, session_ref).await?;
    let (events, agent_session_ids, token_usage) = tokio::try_join!(
        store.events(&session_id),
        store.agent_session_ids(&session_id),
        store.token_usage(&session_id),
    )?;
    let transcript = project_loaded_session_transcript(&events, &agent_session_ids);
    summary.transcript_message_count = transcript.len();
    Ok(LoadedSession {
        summary,
        agent_session_ids,
        transcript,
        events,
        token_usage,
    })
}

pub(crate) async fn load_agent_session(
    store: &Arc<dyn SessionStore>,
    summary: PersistedAgentSessionSummary,
) -> Result<LoadedAgentSession> {
    let session_id = SessionId::from(summary.session_ref.clone());
    let (events, token_usage) =
        tokio::try_join!(store.events(&session_id), store.token_usage(&session_id),)?;
    Ok(project_loaded_agent_session(summary, &events, &token_usage))
}

pub(crate) async fn export_session_events(
    store: &Arc<dyn SessionStore>,
    workspace_root: &Path,
    session_ref: &str,
    relative_or_absolute: &str,
) -> Result<SessionExportArtifact> {
    let (session_id, _) = resolve_session(store, session_ref).await?;
    let events = store.events(&session_id).await?;
    let output_path = write_output_file(
        workspace_root,
        relative_or_absolute,
        encode_session_events_jsonl(&events)?,
    )
    .await?;
    Ok(SessionExportArtifact {
        kind: SessionExportKind::EventsJsonl,
        session_id,
        output_path,
        item_count: events.len(),
    })
}

pub(crate) async fn export_session_transcript(
    store: &Arc<dyn SessionStore>,
    workspace_root: &Path,
    session_ref: &str,
    relative_or_absolute: &str,
) -> Result<SessionExportArtifact> {
    let (session_id, _) = resolve_session(store, session_ref).await?;
    let (events, agent_session_ids) = tokio::try_join!(
        store.events(&session_id),
        store.agent_session_ids(&session_id),
    )?;
    let transcript = project_loaded_session_transcript(&events, &agent_session_ids);
    let output_path = write_output_file(
        workspace_root,
        relative_or_absolute,
        render_transcript_text(&transcript),
    )
    .await?;
    Ok(SessionExportArtifact {
        kind: SessionExportKind::TranscriptText,
        session_id,
        output_path,
        item_count: transcript.len(),
    })
}

async fn resolve_session(
    store: &Arc<dyn SessionStore>,
    session_ref: &str,
) -> Result<(SessionId, SessionSummary)> {
    let sessions = list_sessions(store).await?;
    let session_id = resolve_session_reference(&sessions, session_ref)?;
    let summary = sessions
        .into_iter()
        .find(|summary| summary.session_id == session_id)
        .ok_or_else(|| anyhow!("session missing from store listing: {}", session_id))?;
    Ok((session_id, summary))
}

fn write_output_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

async fn write_output_file(
    workspace_root: &Path,
    relative_or_absolute: &str,
    content: String,
) -> Result<PathBuf> {
    let path = write_output_path(workspace_root, relative_or_absolute);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, content).await?;
    Ok(path)
}

pub(crate) fn resolve_session_reference(
    sessions: &[SessionSummary],
    session_ref: &str,
) -> Result<SessionId> {
    if let Some(session) = sessions
        .iter()
        .find(|summary| summary.session_id.as_str() == session_ref)
    {
        return Ok(session.session_id.clone());
    }

    let matches = sessions
        .iter()
        .filter(|summary| summary.session_id.as_str().starts_with(session_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow!("unknown session id or prefix: {session_ref}")),
        [session] => Ok(session.session_id.clone()),
        _ => Err(anyhow!(
            "ambiguous session prefix {session_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|session| preview_id(session.session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub(crate) fn encode_session_events_jsonl(events: &[SessionEventEnvelope]) -> Result<String> {
    let mut lines = Vec::with_capacity(events.len());
    for event in events {
        lines.push(serde_json::to_string(event)?);
    }
    Ok(if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    })
}

pub(crate) fn render_transcript_text(transcript: &[Message]) -> String {
    let lines = transcript.iter().map(message_to_text).collect::<Vec<_>>();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n\n"))
    }
}

pub(crate) fn message_to_text(message: &Message) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!("{role}> {}", agent::types::message_operator_text(message))
}

pub(crate) fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn project_loaded_agent_session(
    mut summary: PersistedAgentSessionSummary,
    events: &[SessionEventEnvelope],
    token_usage: &SessionTokenUsageReport,
) -> LoadedAgentSession {
    let agent_session_id = AgentSessionId::from(summary.agent_session_ref.clone());
    let scoped_events = events
        .iter()
        .filter(|event| event.agent_session_id == agent_session_id)
        .cloned()
        .collect::<Vec<_>>();
    // Agent-session inspection should mirror the runtime-visible transcript
    // shape when compaction metadata is available, but still degrade to raw
    // replay for older checkpoints instead of failing the history view.
    let transcript = session_resume::reconstruct_runtime_session(&scoped_events, &agent_session_id)
        .map(|session| session_resume::visible_transcript(&session))
        .unwrap_or_else(|_| replay_transcript(&scoped_events));
    summary.transcript_message_count = transcript.len();
    let agent_token_usage = token_usage
        .agent_sessions
        .iter()
        .find(|record| record.agent_session_id.as_ref() == Some(&agent_session_id))
        .cloned();
    let subagents = collect_loaded_subagents(&scoped_events, token_usage);

    LoadedAgentSession {
        summary,
        transcript,
        events: scoped_events,
        token_usage: agent_token_usage,
        subagents,
    }
}

fn project_loaded_session_transcript(
    events: &[SessionEventEnvelope],
    agent_session_ids: &[AgentSessionId],
) -> Vec<Message> {
    if agent_session_ids.is_empty() {
        return replay_transcript(events);
    }

    let mut transcript = Vec::new();
    for agent_session_id in agent_session_ids {
        let scoped_events = events
            .iter()
            .filter(|event| &event.agent_session_id == agent_session_id)
            .cloned()
            .collect::<Vec<_>>();
        let projected =
            session_resume::reconstruct_runtime_session(&scoped_events, agent_session_id)
                .map(|session| session_resume::visible_transcript(&session))
                .unwrap_or_else(|_| replay_transcript(&scoped_events));
        transcript.extend(projected);
    }
    transcript
}

fn collect_loaded_subagents(
    events: &[SessionEventEnvelope],
    token_usage: &SessionTokenUsageReport,
) -> Vec<LoadedSubagentSession> {
    #[derive(Clone, Debug)]
    struct SubagentAccumulator {
        handle: AgentHandle,
        task: AgentTaskSpec,
        status: AgentStatus,
        summary: String,
    }

    let mut by_session = BTreeMap::<SessionId, SubagentAccumulator>::new();
    for event in events {
        match &event.event {
            SessionEventKind::SubagentStart { handle, task } => {
                by_session
                    .entry(handle.session_id.clone())
                    .or_insert_with(|| SubagentAccumulator {
                        handle: handle.clone(),
                        task: task.clone(),
                        status: handle.status.clone(),
                        summary: "running".to_string(),
                    });
            }
            SessionEventKind::SubagentStop {
                handle,
                result,
                error,
            } => {
                let entry = by_session
                    .entry(handle.session_id.clone())
                    .or_insert_with(|| SubagentAccumulator {
                        handle: handle.clone(),
                        task: AgentTaskSpec {
                            task_id: "unknown".to_string(),
                            role: "worker".to_string(),
                            prompt: String::new(),
                            steer: None,
                            allowed_tools: Vec::new(),
                            requested_write_set: Vec::new(),
                            dependency_ids: Vec::new(),
                            timeout_seconds: None,
                        },
                        status: handle.status.clone(),
                        summary: "stopped".to_string(),
                    });
                entry.handle = handle.clone();
                entry.status = result
                    .as_ref()
                    .map(|result| result.status.clone())
                    .unwrap_or_else(|| handle.status.clone());
                entry.summary = result
                    .as_ref()
                    .map(|result| result.summary.clone())
                    .or_else(|| error.clone())
                    .unwrap_or_else(|| "stopped".to_string());
            }
            _ => {}
        }
    }

    let mut subagents = by_session
        .into_values()
        .map(|entry| LoadedSubagentSession {
            token_usage: token_usage
                .subagents
                .iter()
                .find(|record| record.session_id == entry.handle.session_id)
                .cloned(),
            handle: entry.handle,
            task: entry.task,
            status: entry.status,
            summary: entry.summary,
        })
        .collect::<Vec<_>>();
    subagents.sort_by(|left, right| left.task.task_id.cmp(&right.task.task_id));
    subagents
}

#[cfg(test)]
mod tests {
    use super::{
        encode_session_events_jsonl, project_loaded_agent_session,
        project_loaded_session_transcript, render_transcript_text, resolve_session_reference,
    };
    use agent::types::{
        AgentHandle, AgentResultEnvelope, AgentSessionId, AgentStatus, AgentTaskSpec, Message,
        MessagePart, MessageRole, SessionEventEnvelope, SessionEventKind, SessionId,
        SubmittedPromptSnapshot, TokenLedgerSnapshot,
    };
    use store::{
        SessionSummary, SessionTokenUsageReport, TokenUsageRecord,
        TokenUsageScope as StoreTokenUsageScope,
    };

    #[test]
    fn resolves_unique_session_prefix() {
        let sessions = vec![
            SessionSummary {
                session_id: SessionId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("first".to_string()),
                token_usage: None,
            },
            SessionSummary {
                session_id: SessionId::from("def67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("second".to_string()),
                token_usage: None,
            },
        ];

        assert_eq!(
            resolve_session_reference(&sessions, "abc").unwrap(),
            SessionId::from("abc12345")
        );
    }

    #[test]
    fn rejects_ambiguous_session_prefix() {
        let sessions = vec![
            SessionSummary {
                session_id: SessionId::from("abc12345"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
                token_usage: None,
            },
            SessionSummary {
                session_id: SessionId::from("abc67890"),
                first_timestamp_ms: 4,
                last_timestamp_ms: 5,
                event_count: 6,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: None,
                token_usage: None,
            },
        ];

        assert!(resolve_session_reference(&sessions, "abc").is_err());
    }

    #[test]
    fn transcript_export_joins_messages_with_blank_lines() {
        let transcript = vec![
            Message::new(
                MessageRole::User,
                vec![MessagePart::Text {
                    text: "first".to_string(),
                }],
            ),
            Message::new(
                MessageRole::Assistant,
                vec![MessagePart::Text {
                    text: "second".to_string(),
                }],
            ),
        ];

        assert_eq!(
            render_transcript_text(&transcript),
            "user> first\n\nassistant> second\n"
        );
    }

    #[test]
    fn transcript_export_keeps_attachment_and_reference_markers() {
        let transcript = vec![Message::new(
            MessageRole::User,
            vec![
                MessagePart::ImageUrl {
                    url: "https://example.com/failure.png".to_string(),
                    mime_type: None,
                },
                MessagePart::Reference {
                    kind: "skill".to_string(),
                    name: Some("openai-docs".to_string()),
                    uri: None,
                    text: Some("Use official docs".to_string()),
                },
            ],
        )];

        assert_eq!(
            render_transcript_text(&transcript),
            "user> [image_url:https://example.com/failure.png]\n[reference:skill openai-docs Use official docs]\n"
        );
    }

    #[test]
    fn event_export_writes_jsonl_lines() {
        let events = vec![SessionEventEnvelope::new(
            SessionId::from("session-1"),
            AgentSessionId::from("session-1"),
            None,
            None,
            SessionEventKind::SessionStart { reason: None },
        )];

        let encoded = encode_session_events_jsonl(&events).unwrap();
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("\"kind\":\"session_start\""));
    }

    #[test]
    fn projects_agent_session_transcript_and_spawned_subagents() {
        let session_id = SessionId::from("session-root");
        let root_agent_session_id = AgentSessionId::from("agent-root");
        let child_session_id = SessionId::from("session-child");
        let child_agent_session_id = AgentSessionId::from("agent-child");
        let handle = AgentHandle {
            agent_id: "agent-reviewer".into(),
            parent_agent_id: None,
            session_id: child_session_id.clone(),
            agent_session_id: child_agent_session_id.clone(),
            task_id: "review-task".to_string(),
            role: "reviewer".to_string(),
            status: AgentStatus::Completed,
        };
        let task = AgentTaskSpec {
            task_id: "review-task".to_string(),
            role: "reviewer".to_string(),
            prompt: "inspect the patch".to_string(),
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::UserPromptSubmit {
                    prompt: SubmittedPromptSnapshot::from_text("inspect"),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("inspect"),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::SubagentStart {
                    handle: handle.clone(),
                    task: task.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::SubagentStop {
                    handle: handle.clone(),
                    result: Some(AgentResultEnvelope {
                        agent_id: handle.agent_id.clone(),
                        task_id: task.task_id.clone(),
                        status: AgentStatus::Completed,
                        summary: "looks good".to_string(),
                        text: "looks good".to_string(),
                        artifacts: Vec::new(),
                        claimed_files: Vec::new(),
                        structured_payload: None,
                    }),
                    error: None,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                root_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::assistant("done"),
                },
            ),
        ];
        let summary = super::PersistedAgentSessionSummary {
            agent_session_ref: root_agent_session_id.to_string(),
            session_ref: session_id.to_string(),
            label: "root".to_string(),
            first_timestamp_ms: 1,
            last_timestamp_ms: 5,
            event_count: events.len(),
            transcript_message_count: 2,
            session_title: None,
            last_user_prompt: Some("inspect".to_string()),
            resume_support: super::super::session_catalog::ResumeSupport::AttachedToActiveRuntime,
        };
        let token_usage = SessionTokenUsageReport {
            session: None,
            agent_sessions: vec![TokenUsageRecord {
                scope: StoreTokenUsageScope::AgentSession,
                session_id: session_id.clone(),
                agent_session_id: Some(root_agent_session_id.clone()),
                agent_name: None,
                task_id: None,
                ledger: TokenLedgerSnapshot::default(),
            }],
            subagents: vec![TokenUsageRecord {
                scope: StoreTokenUsageScope::Subagent,
                session_id: child_session_id.clone(),
                agent_session_id: Some(child_agent_session_id.clone()),
                agent_name: Some("reviewer".to_string()),
                task_id: Some(task.task_id.clone()),
                ledger: TokenLedgerSnapshot::default(),
            }],
            tasks: Vec::new(),
            aggregate_usage: Default::default(),
        };

        let loaded = project_loaded_agent_session(summary, &events, &token_usage);

        assert_eq!(loaded.transcript.len(), 2);
        assert_eq!(loaded.subagents.len(), 1);
        assert_eq!(loaded.subagents[0].task.role, "reviewer");
        assert_eq!(loaded.subagents[0].status, AgentStatus::Completed);
        assert_eq!(loaded.subagents[0].summary, "looks good");
        assert!(loaded.subagents[0].token_usage.is_some());
    }

    #[test]
    fn projects_agent_session_visible_transcript_for_compacted_history() {
        let session_id = SessionId::from("session-root");
        let agent_session_id = AgentSessionId::from("agent-root");
        let older_prompt = Message::user("older prompt").with_message_id("msg-1");
        let older_answer = Message::assistant("older answer").with_message_id("msg-2");
        let kept_prompt = Message::user("kept prompt").with_message_id("msg-3");
        let summary = Message::system("compaction summary").with_message_id("msg-4");
        let follow_up = Message::assistant("after compaction").with_message_id("msg-5");
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: older_prompt,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: older_answer,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: kept_prompt.clone(),
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
                    summary_chars: 17,
                    summary_message_id: Some(summary.message_id.clone()),
                    retained_tail_message_ids: vec![kept_prompt.message_id.clone()],
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage { message: follow_up },
            ),
        ];
        let summary = super::PersistedAgentSessionSummary {
            agent_session_ref: agent_session_id.to_string(),
            session_ref: session_id.to_string(),
            label: "root".to_string(),
            first_timestamp_ms: 1,
            last_timestamp_ms: 6,
            event_count: events.len(),
            transcript_message_count: 5,
            session_title: None,
            last_user_prompt: Some("kept prompt".to_string()),
            resume_support: super::super::session_catalog::ResumeSupport::AttachedToActiveRuntime,
        };
        let token_usage = SessionTokenUsageReport::default();

        let loaded = project_loaded_agent_session(summary, &events, &token_usage);

        assert_eq!(loaded.summary.transcript_message_count, 3);
        assert_eq!(
            loaded
                .transcript
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "compaction summary".to_string(),
                "kept prompt".to_string(),
                "after compaction".to_string(),
            ]
        );
    }

    #[test]
    fn projects_session_visible_transcript_across_agent_session_boundaries() {
        let session_id = SessionId::from("session-root");
        let first_agent_session_id = AgentSessionId::from("agent-root-1");
        let second_agent_session_id = AgentSessionId::from("agent-root-2");
        let older_prompt = Message::user("older prompt").with_message_id("msg-1");
        let older_answer = Message::assistant("older answer").with_message_id("msg-2");
        let kept_prompt = Message::user("kept prompt").with_message_id("msg-3");
        let summary = Message::system("compaction summary").with_message_id("msg-4");
        let first_follow_up = Message::assistant("after compaction").with_message_id("msg-5");
        let second_prompt = Message::user("fresh session prompt").with_message_id("msg-6");
        let second_answer = Message::assistant("fresh session answer").with_message_id("msg-7");
        let events = vec![
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: older_prompt,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: older_answer,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: kept_prompt.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: summary.clone(),
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::CompactionCompleted {
                    reason: "manual".to_string(),
                    source_message_count: 2,
                    retained_message_count: 1,
                    summary_chars: 17,
                    summary_message_id: Some(summary.message_id.clone()),
                    retained_tail_message_ids: vec![kept_prompt.message_id.clone()],
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                first_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: first_follow_up,
                },
            ),
            SessionEventEnvelope::new(
                session_id.clone(),
                second_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: second_prompt,
                },
            ),
            SessionEventEnvelope::new(
                session_id,
                second_agent_session_id.clone(),
                None,
                None,
                SessionEventKind::TranscriptMessage {
                    message: second_answer,
                },
            ),
        ];

        let transcript = project_loaded_session_transcript(
            &events,
            &[first_agent_session_id, second_agent_session_id],
        );

        assert_eq!(transcript.len(), 5);
        assert_eq!(
            transcript
                .iter()
                .map(Message::text_content)
                .collect::<Vec<_>>(),
            vec![
                "compaction summary".to_string(),
                "kept prompt".to_string(),
                "after compaction".to_string(),
                "fresh session prompt".to_string(),
                "fresh session answer".to_string(),
            ]
        );
    }
}
