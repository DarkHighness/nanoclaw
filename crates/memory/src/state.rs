use crate::{MemoryError, MemoryScope, MemoryVectorStoreKind, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MEMORY_STATE_ROOT_RELATIVE: &str = ".nanoclaw/memory";
pub const MEMORY_PROCEDURAL_RELATIVE: &str = ".nanoclaw/memory/procedural";
pub const MEMORY_SEMANTIC_RELATIVE: &str = ".nanoclaw/memory/semantic";
pub const MEMORY_EPISODIC_RELATIVE: &str = ".nanoclaw/memory/episodic";
pub const MEMORY_EPISODIC_RUNS_RELATIVE: &str = ".nanoclaw/memory/episodic/runs";
pub const MEMORY_EPISODIC_SESSIONS_RELATIVE: &str = ".nanoclaw/memory/episodic/sessions";
pub const MEMORY_EPISODIC_SUBAGENTS_RELATIVE: &str = ".nanoclaw/memory/episodic/subagents";
pub const MEMORY_EPISODIC_TASKS_RELATIVE: &str = ".nanoclaw/memory/episodic/tasks";
pub const MEMORY_WORKING_RELATIVE: &str = ".nanoclaw/memory/working";
pub const MEMORY_WORKING_SESSIONS_RELATIVE: &str = ".nanoclaw/memory/working/sessions";
pub const MEMORY_WORKING_TASKS_RELATIVE: &str = ".nanoclaw/memory/working/tasks";
pub const MEMORY_COORDINATION_RELATIVE: &str = ".nanoclaw/memory/coordination";
pub const MEMORY_COORDINATION_PLANS_RELATIVE: &str = ".nanoclaw/memory/coordination/plans";
pub const MEMORY_COORDINATION_CLAIMS_RELATIVE: &str = ".nanoclaw/memory/coordination/claims";
pub const MEMORY_COORDINATION_HANDOFFS_RELATIVE: &str = ".nanoclaw/memory/coordination/handoffs";
pub const MEMORY_RUNTIME_EXPORTS_RELATIVE: &str = MEMORY_EPISODIC_RELATIVE;
pub const MEMORY_INDEXES_RELATIVE: &str = ".nanoclaw/memory/indexes";
pub const MEMORY_LIFECYCLE_RELATIVE: &str = ".nanoclaw/memory/lifecycle";
pub const MEMORY_CORE_SQLITE_INDEX_RELATIVE: &str = ".nanoclaw/memory/indexes/memory-core.sqlite";
pub const MEMORY_EMBED_SQLITE_INDEX_RELATIVE: &str = ".nanoclaw/memory/indexes/memory-embed.sqlite";
pub const MEMORY_EMBED_LANCEDB_DIR_RELATIVE: &str = ".nanoclaw/memory/indexes/memory-embed-lancedb";

#[derive(Clone, Debug)]
pub struct MemoryStateLayout {
    workspace_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedStatePath {
    absolute: PathBuf,
    relative: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySidecarStatus {
    #[default]
    Ready,
    Rebuilding,
    Skipped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySidecarLifecycle {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub status: MemorySidecarStatus,
    #[serde(default)]
    pub vector_store: String,
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub config_fingerprint: String,
    #[serde(default)]
    pub indexed_chunk_count: usize,
    #[serde(default)]
    pub indexed_document_count: usize,
    #[serde(default)]
    pub exported_run_count: usize,
    #[serde(default)]
    pub exported_agent_session_count: usize,
    #[serde(default)]
    pub exported_subagent_count: usize,
    #[serde(default)]
    pub exported_task_count: usize,
    #[serde(default)]
    pub exported_document_count: usize,
    #[serde(default)]
    pub artifact_path: String,
    #[serde(default)]
    pub document_snapshots: BTreeMap<String, String>,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl ResolvedStatePath {
    #[must_use]
    pub fn absolute_path(&self) -> &Path {
        &self.absolute
    }

    #[must_use]
    pub fn relative_path(&self) -> &Path {
        &self.relative
    }

    #[must_use]
    pub fn relative_display(&self) -> String {
        self.relative.display().to_string()
    }
}

impl MemoryStateLayout {
    #[must_use]
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    #[must_use]
    pub fn root_dir(&self) -> PathBuf {
        self.workspace_root.join(MEMORY_STATE_ROOT_RELATIVE)
    }

    #[must_use]
    pub fn default_runtime_exports_dir(&self) -> PathBuf {
        self.workspace_root.join(MEMORY_RUNTIME_EXPORTS_RELATIVE)
    }

    #[must_use]
    pub fn default_indexes_dir(&self) -> PathBuf {
        self.workspace_root.join(MEMORY_INDEXES_RELATIVE)
    }

    #[must_use]
    pub fn default_lifecycle_dir(&self) -> PathBuf {
        self.workspace_root.join(MEMORY_LIFECYCLE_RELATIVE)
    }

    pub fn resolve_index_path(
        &self,
        configured: Option<&Path>,
        default_relative: &Path,
    ) -> Result<ResolvedStatePath> {
        let candidate = configured.unwrap_or(default_relative);
        self.resolve_path_within_state_root(candidate)
    }

    pub fn resolve_vector_store_path(
        &self,
        configured: Option<&Path>,
        kind: MemoryVectorStoreKind,
    ) -> Result<ResolvedStatePath> {
        let default_relative = match kind {
            MemoryVectorStoreKind::Sqlite => Path::new(MEMORY_EMBED_SQLITE_INDEX_RELATIVE),
            MemoryVectorStoreKind::Lancedb => Path::new(MEMORY_EMBED_LANCEDB_DIR_RELATIVE),
        };
        self.resolve_index_path(configured, default_relative)
    }

    pub fn resolve_runtime_exports_dir(&self, configured: &Path) -> Result<ResolvedStatePath> {
        self.resolve_path_within_state_root(configured)
    }

    pub fn resolve_managed_memory_path(&self, relative: &Path) -> Result<ResolvedStatePath> {
        self.resolve_path_within_state_root(relative)
    }

    pub fn resolve_scope_dir(&self, scope: MemoryScope) -> Result<ResolvedStatePath> {
        let relative = match scope {
            MemoryScope::Procedural => Path::new(MEMORY_PROCEDURAL_RELATIVE),
            MemoryScope::Semantic => Path::new(MEMORY_SEMANTIC_RELATIVE),
            MemoryScope::Episodic => Path::new(MEMORY_EPISODIC_RELATIVE),
            MemoryScope::Working => Path::new(MEMORY_WORKING_RELATIVE),
            MemoryScope::Coordination => Path::new(MEMORY_COORDINATION_RELATIVE),
        };
        self.resolve_path_within_state_root(relative)
    }

    pub fn resolve_lifecycle_path(&self, backend: &str) -> Result<ResolvedStatePath> {
        let relative = PathBuf::from(format!("{MEMORY_LIFECYCLE_RELATIVE}/{backend}.toml"));
        self.resolve_path_within_state_root(&relative)
    }

    pub fn load_lifecycle(&self, backend: &str) -> Result<Option<MemorySidecarLifecycle>> {
        let path = self.resolve_lifecycle_path(backend)?;
        let raw = match std::fs::read_to_string(path.absolute_path()) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(toml::from_str(&raw).map_err(|error| {
            MemoryError::invalid(format!(
                "failed to decode lifecycle manifest `{}`: {error}",
                path.relative_display()
            ))
        })?))
    }

    pub fn write_lifecycle(
        &self,
        backend: &str,
        mut lifecycle: MemorySidecarLifecycle,
    ) -> Result<ResolvedStatePath> {
        let path = self.resolve_lifecycle_path(backend)?;
        if let Some(parent) = path.absolute_path().parent() {
            std::fs::create_dir_all(parent)?;
        }
        if lifecycle.updated_at_ms == 0 {
            lifecycle.updated_at_ms = current_timestamp_ms();
        }
        let encoded = toml::to_string_pretty(&lifecycle).map_err(|error| {
            MemoryError::invalid(format!(
                "failed to encode lifecycle manifest `{}`: {error}",
                path.relative_display()
            ))
        })?;
        std::fs::write(path.absolute_path(), encoded)?;
        Ok(path)
    }

    fn resolve_path_within_state_root(&self, configured: &Path) -> Result<ResolvedStatePath> {
        reject_parent_components(configured)?;
        let absolute = if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            self.workspace_root.join(configured)
        };
        if !absolute.starts_with(&self.workspace_root) {
            return Err(MemoryError::PathOutsideWorkspace(
                configured.display().to_string(),
            ));
        }
        let root_dir = self.root_dir();
        // Memory sidecars are host-generated mutable state. Constraining them to
        // `.nanoclaw/memory` keeps lifecycle ownership local to the active
        // worktree and avoids accidentally indexing arbitrary workspace paths.
        if !absolute.starts_with(&root_dir) {
            return Err(MemoryError::PathOutsideWorkspace(
                configured.display().to_string(),
            ));
        }
        let relative = absolute
            .strip_prefix(&self.workspace_root)
            .map_err(|_| MemoryError::PathOutsideWorkspace(absolute.display().to_string()))?
            .to_path_buf();
        Ok(ResolvedStatePath { absolute, relative })
    }
}

fn reject_parent_components(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(MemoryError::invalid(format!(
            "path `{}` cannot contain parent traversal",
            path.display()
        )));
    }
    Ok(())
}

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        MEMORY_EMBED_SQLITE_INDEX_RELATIVE, MEMORY_RUNTIME_EXPORTS_RELATIVE,
        MemorySidecarLifecycle, MemoryStateLayout,
    };
    use crate::MemoryError;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn resolves_default_runtime_export_dir_inside_workspace_state_root() {
        let layout = MemoryStateLayout::new("/workspace");
        let resolved = layout
            .resolve_runtime_exports_dir(Path::new(MEMORY_RUNTIME_EXPORTS_RELATIVE))
            .unwrap();
        assert_eq!(
            resolved.absolute_path(),
            Path::new("/workspace/.nanoclaw/memory/episodic")
        );
        assert_eq!(
            resolved.relative_path(),
            Path::new(".nanoclaw/memory/episodic")
        );
    }

    #[test]
    fn rejects_parent_traversal_runtime_export_path() {
        let layout = MemoryStateLayout::new("/workspace");
        let err = layout
            .resolve_runtime_exports_dir(Path::new(".nanoclaw/memory/../escape"))
            .unwrap_err();
        assert!(matches!(err, MemoryError::InvalidInput(_)));
    }

    #[test]
    fn rejects_runtime_export_dir_outside_memory_state_root() {
        let layout = MemoryStateLayout::new("/workspace");
        let err = layout
            .resolve_runtime_exports_dir(Path::new("memory/runtime"))
            .unwrap_err();
        assert!(matches!(err, MemoryError::PathOutsideWorkspace(_)));
    }

    #[test]
    fn resolves_default_embed_sqlite_path_inside_state_root() {
        let layout = MemoryStateLayout::new("/workspace");
        let resolved = layout
            .resolve_index_path(None, Path::new(MEMORY_EMBED_SQLITE_INDEX_RELATIVE))
            .unwrap();
        assert_eq!(
            resolved.absolute_path(),
            Path::new("/workspace/.nanoclaw/memory/indexes/memory-embed.sqlite")
        );
        assert_eq!(
            resolved.relative_path(),
            Path::new(".nanoclaw/memory/indexes/memory-embed.sqlite")
        );
    }

    #[test]
    fn lifecycle_manifest_round_trips() {
        let dir = tempdir().unwrap();
        let layout = MemoryStateLayout::new(dir.path());
        let expected = MemorySidecarLifecycle {
            backend: "memory-embed".to_string(),
            vector_store: "sqlite".to_string(),
            schema_version: 3,
            config_fingerprint: "abc123".to_string(),
            indexed_chunk_count: 4,
            ..MemorySidecarLifecycle::default()
        };

        layout
            .write_lifecycle("memory-embed", expected.clone())
            .unwrap();

        let actual = layout.load_lifecycle("memory-embed").unwrap().unwrap();
        assert_eq!(actual.backend, expected.backend);
        assert_eq!(actual.vector_store, expected.vector_store);
        assert_eq!(actual.schema_version, expected.schema_version);
        assert_eq!(actual.config_fingerprint, expected.config_fingerprint);
        assert_eq!(actual.indexed_chunk_count, expected.indexed_chunk_count);
    }
}
