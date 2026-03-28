use crate::managed_files::{
    current_timestamp_ms, load_managed_memory_file, slugify, write_memory_file,
};
use crate::retention::set_memory_status;
use crate::{
    MemoryDocumentMetadata, MemoryPromoteRequest, MemoryScope, MemoryStateLayout, MemoryStatus,
    Result,
};
use std::path::Path;

pub(crate) async fn promote_memory(
    workspace_root: &Path,
    request: MemoryPromoteRequest,
) -> Result<crate::MemoryMutationResponse> {
    if !matches!(
        request.target_scope,
        MemoryScope::Procedural | MemoryScope::Semantic
    ) {
        return Err(crate::MemoryError::invalid(
            "memory_promote target scope must be `procedural` or `semantic`",
        ));
    }

    let source = load_managed_memory_file(workspace_root, &request.source_path).await?;
    let now_ms = current_timestamp_ms();
    let target_slug = slugify(&request.title);
    if target_slug.is_empty() {
        return Err(crate::MemoryError::invalid(
            "memory_promote requires a non-empty `title`",
        ));
    }

    let target_root = match request.target_scope {
        MemoryScope::Procedural => ".nanoclaw/memory/procedural",
        MemoryScope::Semantic => ".nanoclaw/memory/semantic",
        MemoryScope::Episodic | MemoryScope::Working | MemoryScope::Coordination => unreachable!(),
    };
    let target_path = format!("{target_root}/{target_slug}.md");
    let layer = request.layer.unwrap_or_else(|| match request.target_scope {
        MemoryScope::Procedural => "promoted-procedural".to_string(),
        MemoryScope::Semantic => "promoted-semantic".to_string(),
        MemoryScope::Episodic | MemoryScope::Working | MemoryScope::Coordination => unreachable!(),
    });
    let body = if request.content.trim().is_empty() {
        source.body.clone()
    } else {
        format!("# {}\n\n{}", request.title.trim(), request.content.trim())
    };
    let metadata = MemoryDocumentMetadata {
        scope: request.target_scope,
        layer,
        run_id: None,
        session_id: None,
        agent_name: None,
        task_id: None,
        updated_at_ms: Some(now_ms),
        promoted_from: Some(request.source_path.clone()),
        supersedes: vec![request.source_path.clone()],
        tags: merge_lists(source.metadata.tags, request.tags),
        status: MemoryStatus::Ready,
    };

    let response = write_memory_file(
        &MemoryStateLayout::new(workspace_root),
        &target_path,
        request.title.trim(),
        &body,
        &metadata,
        "promoted",
    )
    .await?;

    // Promotion turns transient notes into durable memory, so the source note
    // should stop competing as "fresh working state" during retrieval.
    let _ = set_memory_status(
        workspace_root,
        &request.source_path,
        MemoryStatus::Stale,
        "staled",
    )
    .await?;

    Ok(response)
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
