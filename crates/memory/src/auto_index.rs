use crate::corpus::parse_memory_text;
use crate::managed_files::{current_timestamp_ms, render_memory_markdown};
use crate::state::MEMORY_AUTO_INDEX_RELATIVE;
use crate::{
    MemoryDocumentMetadata, MemoryScope, MemoryStateLayout, MemoryStatus, MemoryType, Result,
};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoMemoryIndexEntry {
    path: String,
    title: String,
    metadata: MemoryDocumentMetadata,
}

const MAX_AUTO_INDEX_LINES: usize = 200;
const MAX_AUTO_INDEX_BYTES: usize = 25_000;
const MAX_INDEX_HOOK_CHARS: usize = 120;

pub(crate) async fn refresh_auto_memory_index(workspace_root: &Path) -> Result<()> {
    let layout = MemoryStateLayout::new(workspace_root);
    let root_dir = layout.root_dir();
    fs::create_dir_all(&root_dir).await?;

    // The generated index is the Claude-style single entry point for managed
    // auto memory. Detailed notes stay in topic files while `MEMORY.md`
    // remains short enough for a future lightweight primer flow.
    let entries = collect_auto_memory_entries(workspace_root, &root_dir).await?;
    let rendered = render_auto_memory_index(&entries);
    let resolved = layout.resolve_managed_memory_path(Path::new(MEMORY_AUTO_INDEX_RELATIVE))?;
    if let Some(parent) = resolved.absolute_path().parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(
        resolved.absolute_path(),
        render_memory_markdown(
            "Managed Memory Index",
            &rendered,
            &MemoryDocumentMetadata {
                scope: MemoryScope::Semantic,
                memory_type: None,
                description: Some(
                    "Index of durable auto memories and transient memory summaries.".to_string(),
                ),
                layer: "auto-memory-index".to_string(),
                session_id: None,
                agent_session_id: None,
                agent_name: None,
                task_id: None,
                updated_at_ms: Some(current_timestamp_ms()),
                promoted_from: None,
                supersedes: Vec::new(),
                tags: vec!["auto-memory".to_string()],
                status: MemoryStatus::Ready,
            },
        ),
    )
    .await?;
    Ok(())
}

async fn collect_auto_memory_entries(
    workspace_root: &Path,
    root_dir: &Path,
) -> Result<Vec<AutoMemoryIndexEntry>> {
    let mut entries = Vec::new();
    for absolute_path in discover_markdown_paths(root_dir)? {
        let relative = normalize_relative_path(
            absolute_path
                .strip_prefix(workspace_root)
                .expect("managed memory path stays within workspace"),
        );
        if relative == MEMORY_AUTO_INDEX_RELATIVE {
            continue;
        }
        let text = fs::read_to_string(&absolute_path).await?;
        let modified_at_ms = file_timestamp_ms(&absolute_path).await?;
        let (metadata, title) = parse_memory_text(&relative, &text, modified_at_ms)?;
        entries.push(AutoMemoryIndexEntry {
            path: relative,
            title,
            metadata,
        });
    }
    entries.sort_by(|left, right| {
        left.metadata
            .scope
            .cmp(&right.metadata.scope)
            .then_with(|| {
                right
                    .metadata
                    .updated_at_ms
                    .cmp(&left.metadata.updated_at_ms)
            })
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

fn discover_markdown_paths(root_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = WalkBuilder::new(root_dir)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn render_auto_memory_index(entries: &[AutoMemoryIndexEntry]) -> String {
    let mut lines = vec![
        "# Managed Memory Index".to_string(),
        String::new(),
        "This file is generated from `.nanoclaw/memory/` and should stay as a concise map of durable memories.".to_string(),
        String::new(),
        "## Durable Memory".to_string(),
        String::new(),
    ];

    let durable = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.metadata.scope,
                MemoryScope::Procedural | MemoryScope::Semantic
            ) && entry.metadata.layer != "auto-memory-index"
        })
        .collect::<Vec<_>>();

    if durable.is_empty() {
        lines.push("- No durable memories yet.".to_string());
    } else {
        for entry in durable {
            lines.push(render_durable_hook_line(entry));
        }
    }

    lines.push(String::new());
    lines.push("## Runtime Memory Summary".to_string());
    lines.push(String::new());
    push_runtime_scope_summary(&mut lines, entries, MemoryScope::Episodic, "Episodic");
    push_runtime_scope_summary(&mut lines, entries, MemoryScope::Working, "Working");
    push_runtime_scope_summary(
        &mut lines,
        entries,
        MemoryScope::Coordination,
        "Coordination",
    );
    lines.push(String::new());

    finalize_index(lines)
}

fn render_durable_hook_line(entry: &AutoMemoryIndexEntry) -> String {
    format!(
        "- [{}]({}) — {}",
        entry.title,
        managed_memory_link(&entry.path),
        truncate_hook(&entry_hook(entry), MAX_INDEX_HOOK_CHARS),
    )
}

fn push_runtime_scope_summary(
    lines: &mut Vec<String>,
    entries: &[AutoMemoryIndexEntry],
    scope: MemoryScope,
    label: &str,
) {
    let scoped = entries
        .iter()
        .filter(|entry| entry.metadata.scope == scope)
        .collect::<Vec<_>>();
    if let Some(latest) = scoped
        .iter()
        .max_by_key(|entry| entry.metadata.updated_at_ms.unwrap_or_default())
    {
        let kind = latest
            .metadata
            .memory_type
            .map(MemoryType::as_str)
            .unwrap_or("runtime");
        lines.push(format!(
            "- {label}: {} notes, latest [{}]({}) — {} ({}, updated {})",
            scoped.len(),
            latest.title,
            managed_memory_link(&latest.path),
            truncate_hook(&entry_hook(latest), MAX_INDEX_HOOK_CHARS),
            kind,
            format_timestamp_ms(latest.metadata.updated_at_ms),
        ));
    } else {
        lines.push(format!("- {label}: 0 notes"));
    }
}

fn entry_hook(entry: &AutoMemoryIndexEntry) -> String {
    if let Some(description) = entry.metadata.description.as_deref() {
        return description.to_string();
    }
    match entry.metadata.memory_type {
        Some(MemoryType::User) => "user profile and collaboration context".to_string(),
        Some(MemoryType::Feedback) => {
            "validated guidance about how to approach future work".to_string()
        }
        Some(MemoryType::Project) => {
            "project context that is not derivable from the code".to_string()
        }
        Some(MemoryType::Reference) => "pointer to external context worth checking".to_string(),
        None => match entry.metadata.scope {
            MemoryScope::Procedural => "durable procedural guidance".to_string(),
            MemoryScope::Semantic => "durable project memory".to_string(),
            MemoryScope::Episodic => "runtime-derived episodic note".to_string(),
            MemoryScope::Working => "active working note for the current task".to_string(),
            MemoryScope::Coordination => "coordination state shared across agents".to_string(),
        },
    }
}

fn truncate_hook(hook: &str, max_chars: usize) -> String {
    let trimmed = hook.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let truncated = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{truncated}…")
}

fn finalize_index(mut lines: Vec<String>) -> String {
    let overflow_note = "> Additional entries were omitted to keep this index concise. Use `memory_list` for the full inventory.";
    if lines.len() <= MAX_AUTO_INDEX_LINES && lines.join("\n").len() <= MAX_AUTO_INDEX_BYTES {
        return lines.join("\n");
    }

    lines.truncate(MAX_AUTO_INDEX_LINES.saturating_sub(2));
    while !lines.is_empty() {
        let tentative = if lines.last().is_some_and(|line| line.is_empty()) {
            format!("{}\n{}", lines.join("\n"), overflow_note)
        } else {
            format!("{}\n\n{}", lines.join("\n"), overflow_note)
        };
        if tentative.len() <= MAX_AUTO_INDEX_BYTES {
            return tentative;
        }
        lines.pop();
    }

    overflow_note.to_string()
}

fn managed_memory_link(path: &str) -> &str {
    path.strip_prefix(".nanoclaw/memory/").unwrap_or(path)
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

async fn file_timestamp_ms(path: &Path) -> Result<Option<u64>> {
    let metadata = fs::metadata(path).await?;
    let modified = match metadata.modified() {
        Ok(modified) => modified,
        Err(_) => return Ok(None),
    };
    let duration = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration,
        Err(_) => return Ok(None),
    };
    Ok(Some(
        duration
            .as_millis()
            .min(u128::from(u64::MAX))
            .try_into()
            .unwrap_or(u64::MAX),
    ))
}

fn format_timestamp_ms(updated_at_ms: Option<u64>) -> String {
    let Some(updated_at_ms) = updated_at_ms else {
        return "unknown".to_string();
    };
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(updated_at_ms) * 1_000_000)
        .map(|timestamp| {
            timestamp
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| updated_at_ms.to_string())
        })
        .unwrap_or_else(|_| updated_at_ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::refresh_auto_memory_index;
    use crate::managed_files::record_memory;
    use crate::promotion::promote_memory;
    use crate::{MemoryPromoteRequest, MemoryRecordRequest, MemoryScope, MemoryType};
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn refresh_builds_auto_memory_index_from_managed_notes() {
        let dir = tempdir().unwrap();
        let recorded = record_memory(
            dir.path(),
            MemoryRecordRequest {
                scope: MemoryScope::Working,
                title: "Scratch".to_string(),
                content: "temporary finding".to_string(),
                memory_type: None,
                description: Some("Transient deploy debugging note.".to_string()),
                layer: None,
                tags: Vec::new(),
                session_id: None,
                agent_session_id: Some("agent_session_1".into()),
                agent_name: None,
                task_id: None,
            },
            None,
            None,
        )
        .await
        .unwrap();

        promote_memory(
            dir.path(),
            MemoryPromoteRequest {
                source_path: recorded.path,
                target_scope: MemoryScope::Semantic,
                title: "Deploy Memory".to_string(),
                content: "Use a canary deploy before restart.".to_string(),
                memory_type: Some(MemoryType::Project),
                description: Some("Canary deploy requirement before restart.".to_string()),
                layer: None,
                tags: vec!["deploy".to_string()],
            },
        )
        .await
        .unwrap();

        refresh_auto_memory_index(dir.path()).await.unwrap();

        let rendered = fs::read_to_string(dir.path().join(".nanoclaw/memory/MEMORY.md"))
            .await
            .unwrap();
        assert!(rendered.contains("layer: auto-memory-index"));
        assert!(rendered.contains("## Durable Memory"));
        assert!(rendered.contains(
            "- [Deploy Memory](semantic/deploy-memory.md) — Canary deploy requirement before restart."
        ));
        assert!(rendered.contains("## Runtime Memory Summary"));
        assert!(rendered.contains("Working: 1 notes"));
    }
}
