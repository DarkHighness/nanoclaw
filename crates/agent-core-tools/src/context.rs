use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct ToolExecutionContext {
    pub workspace_root: PathBuf,
    pub sandbox_root: Option<PathBuf>,
    pub workspace_only: bool,
    pub container_workdir: Option<String>,
    pub model_context_window_tokens: Option<usize>,
}

impl ToolExecutionContext {
    #[must_use]
    pub fn effective_root(&self) -> &Path {
        self.sandbox_root
            .as_deref()
            .unwrap_or(self.workspace_root.as_path())
    }
}
