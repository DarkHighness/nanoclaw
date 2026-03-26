use crate::{
    MemoryCorpus, MemoryCorpusConfig, MemorySidecarLifecycle, MemorySidecarStatus,
    MemoryStateLayout, ResolvedStatePath, Result, load_memory_corpus,
};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use store::{RunMemoryExportRecord, RunMemoryExportRequest, RunStore};
use tokio::fs;

const RUNTIME_EXPORTS_LIFECYCLE_ID: &str = "runtime-exports";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryRuntimeExportStats {
    pub exported_runs: usize,
    pub output_dir: Option<String>,
}

pub async fn load_configured_memory_corpus(
    workspace_root: &Path,
    config: &MemoryCorpusConfig,
    run_store: Option<&Arc<dyn RunStore>>,
) -> Result<(MemoryCorpus, MemoryRuntimeExportStats)> {
    let mut effective = config.clone();
    let stats = if config.runtime_export.enabled {
        let layout = MemoryStateLayout::new(workspace_root);
        let output_dir = layout.resolve_runtime_exports_dir(&config.runtime_export.output_dir)?;
        // Runtime exports are materialized as real Markdown sidecars so search
        // hits remain readable through `memory_get` instead of pointing at an
        // in-memory-only synthetic document.
        effective
            .extra_paths
            .push(output_dir.relative_path().to_path_buf());
        materialize_runtime_exports(&layout, config, run_store, &output_dir).await?
    } else {
        MemoryRuntimeExportStats::default()
    };
    let corpus = load_memory_corpus(workspace_root, &effective).await?;
    Ok((corpus, stats))
}

async fn materialize_runtime_exports(
    layout: &MemoryStateLayout,
    config: &MemoryCorpusConfig,
    run_store: Option<&Arc<dyn RunStore>>,
    output_dir: &ResolvedStatePath,
) -> Result<MemoryRuntimeExportStats> {
    let Some(run_store) = run_store else {
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
            exported_runs: 0,
            output_dir: Some(output_dir.relative_display()),
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

    let records = run_store
        .export_for_memory(RunMemoryExportRequest {
            max_runs: Some(config.runtime_export.max_runs.max(1)),
            max_search_corpus_chars: Some(config.runtime_export.max_search_corpus_chars),
        })
        .await
        .map_err(|error| crate::MemoryError::invalid(error.to_string()))?;
    fs::create_dir_all(output_dir.absolute_path()).await?;

    let keep = records
        .iter()
        .map(export_file_name)
        .collect::<BTreeSet<_>>();
    prune_stale_runtime_exports(output_dir.absolute_path(), &keep).await?;

    for record in &records {
        let markdown =
            render_run_export_markdown(record, config.runtime_export.include_search_corpus);
        fs::write(
            output_dir.absolute_path().join(export_file_name(record)),
            markdown,
        )
        .await?;
    }

    layout.write_lifecycle(
        RUNTIME_EXPORTS_LIFECYCLE_ID,
        MemorySidecarLifecycle {
            backend: RUNTIME_EXPORTS_LIFECYCLE_ID.to_string(),
            status: MemorySidecarStatus::Ready,
            artifact_path: output_dir.relative_display(),
            exported_run_count: records.len(),
            ..MemorySidecarLifecycle::default()
        },
    )?;

    Ok(MemoryRuntimeExportStats {
        exported_runs: records.len(),
        output_dir: Some(output_dir.relative_display()),
    })
}

async fn prune_stale_runtime_exports(output_dir: &Path, keep: &BTreeSet<String>) -> Result<()> {
    let mut entries = match fs::read_dir(output_dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if keep.contains(name) {
            continue;
        }
        fs::remove_file(path).await?;
    }
    Ok(())
}

fn export_file_name(record: &RunMemoryExportRecord) -> String {
    format!("{}.md", record.summary.run_id.as_str())
}

fn render_run_export_markdown(
    record: &RunMemoryExportRecord,
    include_search_corpus: bool,
) -> String {
    let mut out = vec![
        format!("# Run {}", record.summary.run_id),
        String::new(),
        format!("- Last updated: {}", record.summary.last_timestamp_ms),
        format!("- Sessions: {}", record.summary.session_count),
        format!("- Events: {}", record.summary.event_count),
        format!(
            "- Transcript messages: {}",
            record.summary.transcript_message_count
        ),
    ];

    if !record.session_ids.is_empty() {
        out.push(format!(
            "- Session ids: {}",
            record
                .session_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if let Some(prompt) = record.summary.last_user_prompt.as_deref()
        && !prompt.trim().is_empty()
    {
        out.push(String::new());
        out.push("## Last User Prompt".to_string());
        out.push(String::new());
        out.push(prompt.trim().to_string());
    }

    if include_search_corpus && !record.search_corpus.trim().is_empty() {
        out.push(String::new());
        out.push("## Recent Search Corpus".to_string());
        out.push(String::new());
        out.push(record.search_corpus.trim().to_string());
    }

    out.push(String::new());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::load_configured_memory_corpus;
    use crate::{MemoryCorpusConfig, MemoryError, MemorySidecarStatus, MemoryStateLayout};
    use std::sync::Arc;
    use store::{EventSink, InMemoryRunStore};
    use tempfile::tempdir;
    use types::{RunEventEnvelope, RunEventKind, RunId, SessionId};

    #[tokio::test]
    async fn writes_runtime_export_markdown_sidecars() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemoryRunStore::new());
        let run_id = RunId::new();
        let session_id = SessionId::new();
        store
            .append(RunEventEnvelope::new(
                run_id.clone(),
                session_id,
                None,
                None,
                RunEventKind::UserPromptSubmit {
                    prompt: "why did deploy fail?".to_string(),
                },
            ))
            .await
            .unwrap();

        let mut config = MemoryCorpusConfig::default();
        config.runtime_export.enabled = true;
        let run_store: Arc<dyn store::RunStore> = store;
        let (corpus, stats) = load_configured_memory_corpus(dir.path(), &config, Some(&run_store))
            .await
            .unwrap();
        assert_eq!(stats.exported_runs, 1);
        assert!(corpus.documents.iter().any(|doc| {
            doc.path.starts_with(".nanoclaw/memory/runtime/") && doc.path.ends_with(".md")
        }));
        let lifecycle = MemoryStateLayout::new(dir.path())
            .load_lifecycle("runtime-exports")
            .unwrap()
            .unwrap();
        assert_eq!(lifecycle.status, MemorySidecarStatus::Ready);
        assert_eq!(lifecycle.exported_run_count, 1);
        assert_eq!(lifecycle.artifact_path, ".nanoclaw/memory/runtime");
    }

    #[tokio::test]
    async fn writes_skipped_lifecycle_when_runtime_exports_have_no_run_store() {
        let dir = tempdir().unwrap();
        let mut config = MemoryCorpusConfig::default();
        config.runtime_export.enabled = true;

        let (_corpus, stats) = load_configured_memory_corpus(dir.path(), &config, None)
            .await
            .unwrap();
        assert_eq!(stats.exported_runs, 0);

        let lifecycle = MemoryStateLayout::new(dir.path())
            .load_lifecycle("runtime-exports")
            .unwrap()
            .unwrap();
        assert_eq!(lifecycle.status, MemorySidecarStatus::Skipped);
        assert_eq!(lifecycle.artifact_path, ".nanoclaw/memory/runtime");
    }

    #[tokio::test]
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
}
