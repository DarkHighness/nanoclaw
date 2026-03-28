use crate::managed_files::{current_timestamp_ms, load_managed_memory_file, write_memory_file};
use crate::{MemoryForgetRequest, MemoryMutationResponse, MemoryStateLayout, MemoryStatus, Result};
use std::path::Path;

pub(crate) async fn forget_memory(
    workspace_root: &Path,
    request: MemoryForgetRequest,
) -> Result<MemoryMutationResponse> {
    if matches!(request.status, MemoryStatus::Ready) {
        return Err(crate::MemoryError::invalid(
            "memory_forget requires `stale`, `superseded`, or `archived` status",
        ));
    }
    set_memory_status(workspace_root, &request.path, request.status, "forgotten").await
}

pub(crate) async fn set_memory_status(
    workspace_root: &Path,
    path: &str,
    status: MemoryStatus,
    action: &str,
) -> Result<MemoryMutationResponse> {
    let mut document = load_managed_memory_file(workspace_root, path).await?;
    document.metadata.status = status;
    document.metadata.updated_at_ms = Some(current_timestamp_ms());
    write_memory_file(
        &MemoryStateLayout::new(workspace_root),
        &document.path,
        &document.title,
        &document.body,
        &document.metadata,
        action,
    )
    .await
}
