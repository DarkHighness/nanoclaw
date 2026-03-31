use crate::{
    MemoryCorpus, MemoryCorpusConfig, MemorySidecarLifecycle, MemorySidecarStatus,
    MemoryStateLayout, ResolvedStatePath, Result, load_memory_corpus,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{
    MemoryExportScope, SessionMemoryExportBundle, SessionMemoryExportRecord,
    SessionMemoryExportRequest, SessionStore,
};
use tokio::fs;
use types::AgentSessionId;

const RUNTIME_EXPORTS_LIFECYCLE_ID: &str = "runtime-exports";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryRuntimeExportStats {
    pub exported_sessions: usize,
    pub exported_agent_sessions: usize,
    pub exported_subagents: usize,
    pub exported_tasks: usize,
    pub exported_documents: usize,
    pub output_dir: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeExportLoadMode {
    Materialize,
    ReadExisting,
}

pub async fn load_configured_memory_corpus(
    workspace_root: &Path,
    config: &MemoryCorpusConfig,
    session_store: Option<&Arc<dyn SessionStore>>,
) -> Result<(MemoryCorpus, MemoryRuntimeExportStats)> {
    load_configured_memory_corpus_with_mode(
        workspace_root,
        config,
        session_store,
        RuntimeExportLoadMode::Materialize,
    )
    .await
}

pub async fn load_configured_memory_corpus_read_only(
    workspace_root: &Path,
    config: &MemoryCorpusConfig,
) -> Result<(MemoryCorpus, MemoryRuntimeExportStats)> {
    load_configured_memory_corpus_with_mode(
        workspace_root,
        config,
        None,
        RuntimeExportLoadMode::ReadExisting,
    )
    .await
}

async fn load_configured_memory_corpus_with_mode(
    workspace_root: &Path,
    config: &MemoryCorpusConfig,
    session_store: Option<&Arc<dyn SessionStore>>,
    mode: RuntimeExportLoadMode,
) -> Result<(MemoryCorpus, MemoryRuntimeExportStats)> {
    let mut effective = config.clone();
    let stats = if config.runtime_export.enabled {
        let layout = MemoryStateLayout::new(workspace_root);
        let output_dir = layout.resolve_runtime_exports_dir(&config.runtime_export.output_dir)?;
        // Runtime exports are persisted as readable Markdown sidecars so memory
        // retrieval can cite them with `memory_get` instead of pointing at an
        // opaque index-only record.
        effective
            .extra_paths
            .push(output_dir.relative_path().to_path_buf());
        match mode {
            RuntimeExportLoadMode::Materialize => {
                materialize_runtime_exports(&layout, config, session_store, &output_dir).await?
            }
            // Read paths should not rewrite runtime-export Markdown or lifecycle
            // state. They index whatever the last explicit/background sync left
            // behind and report those persisted stats back to the caller.
            RuntimeExportLoadMode::ReadExisting => read_runtime_export_stats(&layout, &output_dir)?,
        }
    } else {
        MemoryRuntimeExportStats::default()
    };
    let corpus = load_memory_corpus(workspace_root, &effective).await?;
    Ok((corpus, stats))
}

fn read_runtime_export_stats(
    layout: &MemoryStateLayout,
    output_dir: &ResolvedStatePath,
) -> Result<MemoryRuntimeExportStats> {
    let Some(lifecycle) = layout.load_lifecycle(RUNTIME_EXPORTS_LIFECYCLE_ID)? else {
        return Ok(MemoryRuntimeExportStats {
            output_dir: Some(output_dir.relative_display()),
            ..MemoryRuntimeExportStats::default()
        });
    };
    Ok(MemoryRuntimeExportStats {
        exported_sessions: lifecycle.exported_session_count,
        exported_agent_sessions: lifecycle.exported_agent_session_count,
        exported_subagents: lifecycle.exported_subagent_count,
        exported_tasks: lifecycle.exported_task_count,
        exported_documents: lifecycle.exported_document_count,
        output_dir: Some(output_dir.relative_display()),
    })
}

async fn materialize_runtime_exports(
    layout: &MemoryStateLayout,
    config: &MemoryCorpusConfig,
    session_store: Option<&Arc<dyn SessionStore>>,
    output_dir: &ResolvedStatePath,
) -> Result<MemoryRuntimeExportStats> {
    let Some(session_store) = session_store else {
        layout.write_lifecycle(
            RUNTIME_EXPORTS_LIFECYCLE_ID,
            MemorySidecarLifecycle {
                backend: RUNTIME_EXPORTS_LIFECYCLE_ID.to_string(),
                status: MemorySidecarStatus::Skipped,
                artifact_path: output_dir.relative_display(),
                ..MemorySidecarLifecycle::default()
            },
        )?;
        return Ok(MemoryRuntimeExportStats {
            output_dir: Some(output_dir.relative_display()),
            ..MemoryRuntimeExportStats::default()
        });
    };

    layout.write_lifecycle(
        RUNTIME_EXPORTS_LIFECYCLE_ID,
        MemorySidecarLifecycle {
            backend: RUNTIME_EXPORTS_LIFECYCLE_ID.to_string(),
            status: MemorySidecarStatus::Rebuilding,
            artifact_path: output_dir.relative_display(),
            ..MemorySidecarLifecycle::default()
        },
    )?;

    let bundle = session_store
        .export_for_memory(SessionMemoryExportRequest {
            max_sessions: Some(config.runtime_export.max_sessions.max(1)),
            max_search_corpus_chars: Some(config.runtime_export.max_search_corpus_chars),
        })
        .await
        .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;

    fs::create_dir_all(output_dir.absolute_path()).await?;
    let keep = write_export_bundle(
        output_dir.absolute_path(),
        &bundle,
        config.runtime_export.include_search_corpus,
    )
    .await?;
    prune_stale_runtime_exports(output_dir.absolute_path(), &keep).await?;

    let stats = stats_from_bundle(&bundle, Some(output_dir.relative_display()));
    layout.write_lifecycle(
        RUNTIME_EXPORTS_LIFECYCLE_ID,
        MemorySidecarLifecycle {
            backend: RUNTIME_EXPORTS_LIFECYCLE_ID.to_string(),
            status: MemorySidecarStatus::Ready,
            artifact_path: output_dir.relative_display(),
            exported_session_count: stats.exported_sessions,
            exported_agent_session_count: stats.exported_agent_sessions,
            exported_subagent_count: stats.exported_subagents,
            exported_task_count: stats.exported_tasks,
            exported_document_count: stats.exported_documents,
            ..MemorySidecarLifecycle::default()
        },
    )?;

    Ok(stats)
}

async fn write_export_bundle(
    root_dir: &Path,
    bundle: &SessionMemoryExportBundle,
    include_search_corpus: bool,
) -> Result<BTreeMap<&'static str, BTreeSet<String>>> {
    let mut keep = BTreeMap::new();
    write_scope_records(
        root_dir,
        "sessions",
        &bundle.sessions,
        include_search_corpus,
        &mut keep,
    )
    .await?;
    write_scope_records(
        root_dir,
        "agent-sessions",
        &bundle.agent_sessions,
        include_search_corpus,
        &mut keep,
    )
    .await?;
    write_scope_records(
        root_dir,
        "subagents",
        &bundle.subagents,
        include_search_corpus,
        &mut keep,
    )
    .await?;
    write_scope_records(
        root_dir,
        "tasks",
        &bundle.tasks,
        include_search_corpus,
        &mut keep,
    )
    .await?;
    Ok(keep)
}

async fn write_scope_records(
    root_dir: &Path,
    directory: &'static str,
    records: &[SessionMemoryExportRecord],
    include_search_corpus: bool,
    keep: &mut BTreeMap<&'static str, BTreeSet<String>>,
) -> Result<()> {
    let scope_dir = root_dir.join(directory);
    fs::create_dir_all(&scope_dir).await?;
    let keep_scope = keep.entry(directory).or_default();

    for record in records {
        let file_name = export_file_name(record);
        keep_scope.insert(file_name.clone());
        fs::write(
            scope_dir.join(&file_name),
            render_export_markdown(record, include_search_corpus),
        )
        .await?;
    }

    Ok(())
}

async fn prune_stale_runtime_exports(
    root_dir: &Path,
    keep: &BTreeMap<&'static str, BTreeSet<String>>,
) -> Result<()> {
    for directory in ["sessions", "agent-sessions", "subagents", "tasks"] {
        prune_scope_dir(root_dir.join(directory), keep.get(directory)).await?;
    }
    Ok(())
}

async fn prune_scope_dir(path: PathBuf, keep: Option<&BTreeSet<String>>) -> Result<()> {
    let mut entries = match fs::read_dir(&path).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        if !entry_path.is_file() {
            continue;
        }
        let Some(name) = entry_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if keep.is_some_and(|keep| keep.contains(name)) {
            continue;
        }
        fs::remove_file(entry_path).await?;
    }
    Ok(())
}

fn export_file_name(record: &SessionMemoryExportRecord) -> String {
    match record.summary.scope {
        MemoryExportScope::Session => format!("{}.md", record.summary.session_id.as_str()),
        MemoryExportScope::AgentSession => format!(
            "{}.md",
            record
                .summary
                .agent_session_id
                .as_ref()
                .map(AgentSessionId::as_str)
                .unwrap_or(record.summary.session_id.as_str())
        ),
        MemoryExportScope::Subagent => format!(
            "{}.md",
            export_name_parts([
                Some(record.summary.session_id.as_str()),
                record
                    .summary
                    .agent_session_id
                    .as_ref()
                    .map(AgentSessionId::as_str),
                record.summary.agent_name.as_deref(),
            ])
        ),
        MemoryExportScope::Task => format!(
            "{}.md",
            export_name_parts([
                Some(record.summary.session_id.as_str()),
                record
                    .summary
                    .agent_session_id
                    .as_ref()
                    .map(AgentSessionId::as_str),
                record.summary.task_id.as_deref(),
            ])
        ),
    }
}

fn export_name_parts<const N: usize>(parts: [Option<&str>; N]) -> String {
    parts
        .into_iter()
        .flatten()
        .map(slug_fragment)
        .collect::<Vec<_>>()
        .join("--")
}

fn slug_fragment(value: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in value.chars() {
        let normalized = match ch {
            'a'..='z' | '0'..='9' => Some(ch),
            'A'..='Z' => Some(ch.to_ascii_lowercase()),
            _ => None,
        };
        if let Some(normalized) = normalized {
            out.push(normalized);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn render_export_markdown(
    record: &SessionMemoryExportRecord,
    include_search_corpus: bool,
) -> String {
    let mut out = vec![
        "---".to_string(),
        "scope: episodic".to_string(),
        format!("layer: {}", scope_layer(record.summary.scope)),
        format!("session_id: {}", record.summary.session_id),
        format!("updated_at_ms: {}", record.summary.last_timestamp_ms),
        "status: ready".to_string(),
        "tags:".to_string(),
        "  - runtime-export".to_string(),
    ];

    if let Some(agent_session_id) = &record.summary.agent_session_id {
        out.push(format!("agent_session_id: {agent_session_id}"));
    }
    if let Some(agent_name) = &record.summary.agent_name {
        out.push(format!("agent_name: {agent_name}"));
    }
    if let Some(task_id) = &record.summary.task_id {
        out.push(format!("task_id: {task_id}"));
    }

    out.push("---".to_string());
    out.push(String::new());
    out.push(export_heading(record));
    out.push(String::new());
    out.push(format!(
        "- Last updated: {}",
        record.summary.last_timestamp_ms
    ));
    out.push(format!("- Events: {}", record.summary.event_count));
    out.push(format!(
        "- Transcript messages: {}",
        record.summary.transcript_message_count
    ));

    if let Some(prompt) = record.summary.last_user_prompt.as_deref()
        && !prompt.trim().is_empty()
    {
        out.push(String::new());
        out.push("## Prompt".to_string());
        out.push(String::new());
        out.push(prompt.trim().to_string());
    }

    push_list_section(&mut out, "Tool Summary", &record.sections.tool_summary);
    push_list_section(&mut out, "Decisions", &record.sections.decisions);
    push_list_section(&mut out, "Failures", &record.sections.failures);
    push_list_section(
        &mut out,
        "Produced Artifacts",
        &record.sections.produced_artifacts,
    );
    push_list_section(&mut out, "Follow-up", &record.sections.follow_up);

    if include_search_corpus && !record.search_corpus.trim().is_empty() {
        out.push(String::new());
        out.push("## Recent Search Corpus".to_string());
        out.push(String::new());
        out.push(record.search_corpus.trim().to_string());
    }

    out.push(String::new());
    out.join("\n")
}

fn push_list_section(lines: &mut Vec<String>, title: &str, entries: &[String]) {
    if entries.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("## {title}"));
    lines.push(String::new());
    for entry in entries {
        lines.push(format!("- {entry}"));
    }
}

fn export_heading(record: &SessionMemoryExportRecord) -> String {
    match record.summary.scope {
        MemoryExportScope::Session => format!("# Run {}", record.summary.session_id),
        MemoryExportScope::AgentSession => format!(
            "# Agent Session {}",
            record
                .summary
                .agent_session_id
                .as_ref()
                .map(AgentSessionId::as_str)
                .unwrap_or(record.summary.session_id.as_str())
        ),
        MemoryExportScope::Subagent => format!(
            "# Subagent {}",
            record.summary.agent_name.as_deref().unwrap_or("unknown")
        ),
        MemoryExportScope::Task => format!(
            "# Task {}",
            record.summary.task_id.as_deref().unwrap_or("unknown")
        ),
    }
}

fn scope_layer(scope: MemoryExportScope) -> &'static str {
    match scope {
        MemoryExportScope::Session => "runtime-session",
        MemoryExportScope::AgentSession => "runtime-agent-session",
        MemoryExportScope::Subagent => "runtime-subagent",
        MemoryExportScope::Task => "runtime-task",
    }
}

fn stats_from_bundle(
    bundle: &SessionMemoryExportBundle,
    output_dir: Option<String>,
) -> MemoryRuntimeExportStats {
    MemoryRuntimeExportStats {
        exported_sessions: bundle.sessions.len(),
        exported_agent_sessions: bundle.agent_sessions.len(),
        exported_subagents: bundle.subagents.len(),
        exported_tasks: bundle.tasks.len(),
        exported_documents: bundle.sessions.len()
            + bundle.agent_sessions.len()
            + bundle.subagents.len()
            + bundle.tasks.len(),
        output_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::{load_configured_memory_corpus, load_configured_memory_corpus_read_only};
    use crate::{MemoryCorpusConfig, MemoryError, MemorySidecarStatus, MemoryStateLayout};
    use async_trait::async_trait;
    use nanoclaw_test_support::run_current_thread_test;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use store::{
        EventSink, InMemorySessionStore, MemoryExportScope, SessionMemoryExportBundle,
        SessionMemoryExportRecord, SessionMemoryExportRequest, SessionStore, SessionStoreError,
    };
    use tempfile::tempdir;
    use types::{
        AgentSessionId, ExperimentEventEnvelope, ExperimentId, Message, SessionEventEnvelope,
        SessionEventKind, SessionId,
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
        async fn writes_runtime_export_markdown_sidecars() {
            let dir = tempdir().unwrap();
            let store = Arc::new(InMemorySessionStore::new());
            let session_id = SessionId::new();
            let agent_session_id = AgentSessionId::new();
            store
                .append(SessionEventEnvelope::new(
                    session_id.clone(),
                    agent_session_id,
                    None,
                    None,
                    SessionEventKind::UserPromptSubmit {
                        prompt: "why did deploy fail?".to_string(),
                    },
                ))
                .await
                .unwrap();

            let mut config = MemoryCorpusConfig::default();
            config.runtime_export.enabled = true;
            let session_store: Arc<dyn store::SessionStore> = store;
            let (corpus, stats) =
                load_configured_memory_corpus(dir.path(), &config, Some(&session_store))
                    .await
                    .unwrap();
            assert_eq!(stats.exported_sessions, 1);
            assert_eq!(stats.exported_agent_sessions, 1);
            assert!(corpus.documents.iter().any(|doc| {
                doc.path.starts_with(".nanoclaw/memory/episodic/sessions/")
                    && doc.path.ends_with(".md")
            }));
            assert!(corpus.documents.iter().any(|doc| {
                doc.path.starts_with(".nanoclaw/memory/episodic/sessions/")
                    && doc.path.ends_with(".md")
            }));
            let lifecycle = MemoryStateLayout::new(dir.path())
                .load_lifecycle("runtime-exports")
                .unwrap()
                .unwrap();
            assert_eq!(lifecycle.status, MemorySidecarStatus::Ready);
            assert_eq!(lifecycle.exported_session_count, 1);
            assert_eq!(lifecycle.exported_agent_session_count, 1);
            assert_eq!(lifecycle.exported_document_count, 2);
            assert_eq!(lifecycle.artifact_path, ".nanoclaw/memory/episodic");
        }
    );

    bounded_async_test!(
        async fn writes_run_session_subagent_and_task_sidecars() {
            let dir = tempdir().unwrap();
            let mut config = MemoryCorpusConfig::default();
            config.runtime_export.enabled = true;
            let store: Arc<dyn SessionStore> = Arc::new(FixtureSessionStore::default());

            let (corpus, stats) = load_configured_memory_corpus(dir.path(), &config, Some(&store))
                .await
                .unwrap();

            assert_eq!(stats.exported_sessions, 1);
            assert_eq!(stats.exported_agent_sessions, 1);
            assert_eq!(stats.exported_subagents, 1);
            assert_eq!(stats.exported_tasks, 1);
            assert_eq!(stats.exported_documents, 4);
            assert!(corpus.documents.iter().any(|doc| {
            doc.path
                == ".nanoclaw/memory/episodic/subagents/session-fixture--session-fixture--reviewer.md"
        }));
            assert!(corpus.documents.iter().any(|doc| {
                doc.path
                    == ".nanoclaw/memory/episodic/tasks/session-fixture--session-fixture--task-17.md"
            }));
        }
    );

    bounded_async_test!(
        async fn writes_skipped_lifecycle_when_runtime_exports_have_no_session_store() {
            let dir = tempdir().unwrap();
            let mut config = MemoryCorpusConfig::default();
            config.runtime_export.enabled = true;

            let (_corpus, stats) = load_configured_memory_corpus(dir.path(), &config, None)
                .await
                .unwrap();
            assert_eq!(stats.exported_sessions, 0);
            assert_eq!(stats.exported_documents, 0);

            let lifecycle = MemoryStateLayout::new(dir.path())
                .load_lifecycle("runtime-exports")
                .unwrap()
                .unwrap();
            assert_eq!(lifecycle.status, MemorySidecarStatus::Skipped);
            assert_eq!(lifecycle.artifact_path, ".nanoclaw/memory/episodic");
        }
    );

    bounded_async_test!(
        async fn rejects_runtime_export_output_outside_memory_state_root() {
            let dir = tempdir().unwrap();
            let mut config = MemoryCorpusConfig::default();
            config.runtime_export.enabled = true;
            config.runtime_export.output_dir = "memory/runtime".into();

            let err = load_configured_memory_corpus(dir.path(), &config, None)
                .await
                .unwrap_err();
            assert!(matches!(err, MemoryError::PathOutsideWorkspace(_)));
        }
    );

    #[derive(Default)]
    struct FixtureSessionStore {
        export_calls: AtomicUsize,
    }

    impl FixtureSessionStore {
        fn export_call_count(&self) -> usize {
            self.export_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EventSink for FixtureSessionStore {
        async fn append(
            &self,
            _event: SessionEventEnvelope,
        ) -> std::result::Result<(), SessionStoreError> {
            Ok(())
        }
    }

    #[async_trait]
    impl SessionStore for FixtureSessionStore {
        async fn list_sessions(
            &self,
        ) -> std::result::Result<Vec<store::SessionSummary>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn search_sessions(
            &self,
            _query: &str,
        ) -> std::result::Result<Vec<store::SessionSearchResult>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn events(
            &self,
            _session_id: &SessionId,
        ) -> std::result::Result<Vec<SessionEventEnvelope>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn agent_session_ids(
            &self,
            _session_id: &SessionId,
        ) -> std::result::Result<Vec<AgentSessionId>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn replay_transcript(
            &self,
            _session_id: &SessionId,
        ) -> std::result::Result<Vec<Message>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn append_experiment(
            &self,
            _event: ExperimentEventEnvelope,
        ) -> std::result::Result<(), SessionStoreError> {
            Ok(())
        }

        async fn list_experiments(
            &self,
        ) -> std::result::Result<Vec<store::ExperimentSummary>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn experiment_events(
            &self,
            _experiment_id: &ExperimentId,
        ) -> std::result::Result<Vec<ExperimentEventEnvelope>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn export_for_memory(
            &self,
            _request: SessionMemoryExportRequest,
        ) -> std::result::Result<SessionMemoryExportBundle, SessionStoreError> {
            self.export_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SessionMemoryExportBundle {
                sessions: vec![fixture_record(MemoryExportScope::Session, None, None)],
                agent_sessions: vec![fixture_record(
                    MemoryExportScope::AgentSession,
                    Some("session-fixture"),
                    None,
                )],
                subagents: vec![fixture_record(
                    MemoryExportScope::Subagent,
                    Some("session-fixture"),
                    Some("reviewer"),
                )],
                tasks: vec![fixture_record(
                    MemoryExportScope::Task,
                    Some("session-fixture"),
                    Some("task-17"),
                )],
            })
        }
    }

    bounded_async_test!(
        async fn read_only_load_reuses_persisted_runtime_export_sidecars() {
            let dir = tempdir().unwrap();
            let mut config = MemoryCorpusConfig::default();
            config.runtime_export.enabled = true;
            let store = Arc::new(FixtureSessionStore::default());
            let session_store: Arc<dyn SessionStore> = store.clone();

            let (_corpus, materialized_stats) =
                load_configured_memory_corpus(dir.path(), &config, Some(&session_store))
                    .await
                    .unwrap();
            assert_eq!(materialized_stats.exported_documents, 4);
            assert_eq!(store.export_call_count(), 1);

            let (read_only_corpus, read_only_stats) =
                load_configured_memory_corpus_read_only(dir.path(), &config)
                    .await
                    .unwrap();
            assert_eq!(store.export_call_count(), 1);
            assert_eq!(read_only_stats, materialized_stats);
            assert!(read_only_corpus.documents.iter().any(|doc| {
                doc.path
                    == ".nanoclaw/memory/episodic/tasks/session-fixture--session-fixture--task-17.md"
            }));
        }
    );

    fn fixture_record(
        scope: MemoryExportScope,
        agent_session_id: Option<&str>,
        detail: Option<&str>,
    ) -> SessionMemoryExportRecord {
        SessionMemoryExportRecord {
            summary: store::MemoryExportSummary {
                scope,
                session_id: SessionId::from("session-fixture"),
                agent_session_id: agent_session_id.map(AgentSessionId::from),
                agent_name: (scope == MemoryExportScope::Subagent)
                    .then(|| detail.unwrap_or("reviewer").to_string()),
                task_id: (scope == MemoryExportScope::Task)
                    .then(|| detail.unwrap_or("task-17").to_string()),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                transcript_message_count: 1,
                last_user_prompt: Some("summarize failures".to_string()),
            },
            search_corpus: "summarize failures".to_string(),
            sections: store::MemoryExportSections {
                tool_summary: vec!["read completed".to_string()],
                decisions: vec!["review outcome recorded".to_string()],
                failures: Vec::new(),
                produced_artifacts: vec!["reports/review.md".to_string()],
                follow_up: vec!["open a follow-up task".to_string()],
            },
        }
    }
}
