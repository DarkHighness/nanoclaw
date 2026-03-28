use crate::corpus::{parse_memory_text, stable_hash};
use crate::state::{
    MEMORY_COORDINATION_CLAIMS_RELATIVE, MEMORY_COORDINATION_HANDOFFS_RELATIVE,
    MEMORY_COORDINATION_PLANS_RELATIVE, MEMORY_WORKING_SESSIONS_RELATIVE,
    MEMORY_WORKING_TASKS_RELATIVE,
};
use crate::{
    MemoryDocumentMetadata, MemoryError, MemoryMutationResponse, MemoryRecordRequest, MemoryScope,
    MemoryStateLayout, MemoryStatus, Result,
};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::fs;
use types::{RunId, SessionId};

#[derive(Clone, Debug)]
pub(crate) struct ManagedMemoryFile {
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) metadata: MemoryDocumentMetadata,
}

pub(crate) async fn record_memory(
    workspace_root: &Path,
    request: MemoryRecordRequest,
    default_run_id: Option<&RunId>,
    default_session_id: Option<&SessionId>,
) -> Result<MemoryMutationResponse> {
    let layout = MemoryStateLayout::new(workspace_root);
    let now_ms = current_timestamp_ms();
    let run_id = request.run_id.clone().or_else(|| default_run_id.cloned());
    let session_id = request
        .session_id
        .clone()
        .or_else(|| default_session_id.cloned());
    let target = resolve_record_target(&request, session_id.as_ref())?;
    let existing = load_managed_memory_file(workspace_root, &target.relative_path)
        .await
        .ok();

    let mut metadata = existing
        .as_ref()
        .map(|document| document.metadata.clone())
        .unwrap_or_default();
    metadata.scope = request.scope;
    metadata.layer = target.layer.clone();
    metadata.run_id = run_id;
    metadata.session_id = session_id;
    metadata.agent_name = normalize_optional(request.agent_name);
    metadata.task_id = normalize_optional(request.task_id);
    metadata.updated_at_ms = Some(now_ms);
    metadata.status = MemoryStatus::Ready;
    metadata.tags = merge_lists(
        existing
            .as_ref()
            .map(|document| document.metadata.tags.clone())
            .unwrap_or_default(),
        request.tags,
    );

    let section_heading = format!(
        "## {} [{}]",
        request.title.trim(),
        format_timestamp_ms(now_ms)
    );
    let existing_body = existing
        .as_ref()
        .map(|document| document.body.as_str())
        .unwrap_or("");
    let body = append_section(
        existing_body,
        &target.document_title,
        &section_heading,
        &request.content,
    );

    write_memory_file(
        &layout,
        &target.relative_path,
        &target.document_title,
        &body,
        &metadata,
        "recorded",
    )
    .await
}

pub(crate) async fn load_managed_memory_file(
    workspace_root: &Path,
    relative_path: &str,
) -> Result<ManagedMemoryFile> {
    let absolute_path = resolve_workspace_path(workspace_root, relative_path)?;
    let text = fs::read_to_string(&absolute_path).await?;
    let modified_at_ms = file_timestamp_ms(&absolute_path).await?;
    let (metadata, title) = parse_memory_text(relative_path, &text, modified_at_ms)?;
    Ok(ManagedMemoryFile {
        path: relative_path.to_string(),
        title,
        body: strip_frontmatter(&text),
        metadata,
    })
}

pub(crate) async fn write_memory_file(
    layout: &MemoryStateLayout,
    relative_path: &str,
    title: &str,
    body: &str,
    metadata: &MemoryDocumentMetadata,
    action: &str,
) -> Result<MemoryMutationResponse> {
    let resolved = layout.resolve_managed_memory_path(Path::new(relative_path))?;
    if let Some(parent) = resolved.absolute_path().parent() {
        fs::create_dir_all(parent).await?;
    }

    let encoded = render_memory_markdown(title, body, metadata);
    fs::write(resolved.absolute_path(), &encoded).await?;
    Ok(MemoryMutationResponse {
        action: action.to_string(),
        path: resolved.relative_display(),
        snapshot_id: stable_hash(&encoded),
        metadata: metadata.clone(),
    })
}

pub(crate) fn render_memory_markdown(
    title: &str,
    body: &str,
    metadata: &MemoryDocumentMetadata,
) -> String {
    let mut out = vec![
        "---".to_string(),
        format!("scope: {}", metadata.scope.as_str()),
        format!("layer: {}", metadata.layer),
        format!("status: {}", metadata.status.as_str()),
    ];
    if let Some(run_id) = &metadata.run_id {
        out.push(format!("run_id: {run_id}"));
    }
    if let Some(session_id) = &metadata.session_id {
        out.push(format!("session_id: {session_id}"));
    }
    if let Some(agent_name) = &metadata.agent_name {
        out.push(format!("agent_name: {agent_name}"));
    }
    if let Some(task_id) = &metadata.task_id {
        out.push(format!("task_id: {task_id}"));
    }
    if let Some(updated_at_ms) = metadata.updated_at_ms {
        out.push(format!("updated_at_ms: {updated_at_ms}"));
    }
    if let Some(promoted_from) = &metadata.promoted_from {
        out.push(format!("promoted_from: {promoted_from}"));
    }
    if !metadata.supersedes.is_empty() {
        out.push("supersedes:".to_string());
        for entry in &metadata.supersedes {
            out.push(format!("  - {entry}"));
        }
    }
    if !metadata.tags.is_empty() {
        out.push("tags:".to_string());
        for tag in &metadata.tags {
            out.push(format!("  - {tag}"));
        }
    }
    out.push("---".to_string());
    out.push(String::new());

    let trimmed_body = body.trim();
    if trimmed_body.is_empty() {
        out.push(format!("# {title}"));
        out.push(String::new());
    } else {
        out.push(trimmed_body.to_string());
        if !trimmed_body.ends_with('\n') {
            out.push(String::new());
        }
    }

    out.join("\n")
}

pub(crate) fn slugify(value: &str) -> String {
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

pub(crate) fn current_timestamp_ms() -> u64 {
    OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .div_euclid(1_000_000)
        .try_into()
        .unwrap_or(u64::MAX)
}

fn resolve_record_target(
    request: &MemoryRecordRequest,
    session_id: Option<&SessionId>,
) -> Result<RecordTarget> {
    match request.scope {
        MemoryScope::Working => {
            if let Some(task_id) = request.task_id.as_deref().map(str::trim)
                && !task_id.is_empty()
            {
                return Ok(RecordTarget {
                    relative_path: format!(
                        "{MEMORY_WORKING_TASKS_RELATIVE}/{}.md",
                        slugify(task_id)
                    ),
                    layer: "working-task".to_string(),
                    document_title: format!("Task {task_id}"),
                });
            }

            let session_id = session_id.ok_or_else(|| {
                MemoryError::invalid("working memory record requires `session_id` or `task_id`")
            })?;
            Ok(RecordTarget {
                relative_path: format!(
                    "{MEMORY_WORKING_SESSIONS_RELATIVE}/{}.md",
                    session_id.as_str()
                ),
                layer: "working-session".to_string(),
                document_title: format!("Session {}", session_id.as_str()),
            })
        }
        MemoryScope::Coordination => {
            let collection = request
                .layer
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("plans");
            let (root, layer) = match collection {
                "plans" => (MEMORY_COORDINATION_PLANS_RELATIVE, "coordination-plan"),
                "claims" => (MEMORY_COORDINATION_CLAIMS_RELATIVE, "coordination-claim"),
                "handoffs" => (
                    MEMORY_COORDINATION_HANDOFFS_RELATIVE,
                    "coordination-handoff",
                ),
                _ => {
                    return Err(MemoryError::invalid(
                        "coordination memory layer must be one of `plans`, `claims`, or `handoffs`",
                    ));
                }
            };
            let slug_source = request
                .task_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&request.title);
            let slug = slugify(slug_source);
            if slug.is_empty() {
                return Err(MemoryError::invalid(
                    "coordination memory record requires a non-empty `title` or `task_id`",
                ));
            }
            Ok(RecordTarget {
                relative_path: format!("{root}/{slug}.md"),
                layer: layer.to_string(),
                document_title: request.title.trim().to_string(),
            })
        }
        MemoryScope::Procedural | MemoryScope::Semantic | MemoryScope::Episodic => {
            Err(MemoryError::invalid(
                "memory_record currently supports only working or coordination scopes",
            ))
        }
    }
}

fn append_section(
    existing_body: &str,
    document_title: &str,
    heading: &str,
    content: &str,
) -> String {
    let section_body = content.trim();
    let mut body = if existing_body.trim().is_empty() {
        format!("# {document_title}")
    } else {
        existing_body.trim().to_string()
    };
    body.push_str("\n\n");
    body.push_str(heading);
    body.push_str("\n\n");
    body.push_str(section_body);
    body
}

fn strip_frontmatter(text: &str) -> String {
    if !text.starts_with("---\n") && text.trim() != "---" {
        return text.trim().to_string();
    }

    let mut lines = text.lines();
    let _ = lines.next();
    let mut closed = false;
    let mut remaining = Vec::new();
    for line in lines {
        if !closed {
            if line.trim() == "---" {
                closed = true;
            }
            continue;
        }
        remaining.push(line);
    }

    if closed {
        remaining.join("\n").trim().to_string()
    } else {
        text.trim().to_string()
    }
}

fn format_timestamp_ms(timestamp_ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_ms) * 1_000_000)
        .ok()
        .and_then(|value| value.format(&Rfc3339).ok())
        .unwrap_or_else(|| timestamp_ms.to_string())
}

fn merge_lists(existing: Vec<String>, added: Vec<String>) -> Vec<String> {
    let mut values = existing
        .into_iter()
        .chain(added)
        .filter_map(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn resolve_workspace_path(workspace_root: &Path, relative_path: &str) -> Result<PathBuf> {
    let absolute_path = workspace_root.join(relative_path);
    if !absolute_path.starts_with(workspace_root) {
        return Err(MemoryError::PathOutsideWorkspace(relative_path.to_string()));
    }
    Ok(absolute_path)
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

struct RecordTarget {
    relative_path: String,
    layer: String,
    document_title: String,
}
