use crate::{Result, fs::assert_path_inside_allowed_roots};
use agent_core_types::{RunId, SessionId, TurnId};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct ToolExecutionContext {
    pub workspace_root: PathBuf,
    pub worktree_root: Option<PathBuf>,
    pub sandbox_root: Option<PathBuf>,
    pub additional_roots: Vec<PathBuf>,
    pub workspace_only: bool,
    pub container_workdir: Option<String>,
    pub model_context_window_tokens: Option<usize>,
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
}

impl ToolExecutionContext {
    #[must_use]
    pub fn effective_root(&self) -> &Path {
        self.sandbox_root
            .as_deref()
            .unwrap_or(self.workspace_root.as_path())
    }

    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        self.worktree_root
            .as_deref()
            .unwrap_or(self.workspace_root.as_path())
    }

    #[must_use]
    pub fn accessible_roots(&self) -> Vec<&Path> {
        // Tools should validate against the same root set the host/runtime
        // exposes, instead of open-coding path policy per tool implementation.
        let mut roots = vec![self.effective_root()];
        if let Some(worktree_root) = self.worktree_root.as_deref() {
            roots.push(worktree_root);
        }
        roots.extend(self.additional_roots.iter().map(PathBuf::as_path));
        roots
    }

    pub fn assert_path_allowed(&self, path: &Path) -> Result<()> {
        assert_path_inside_allowed_roots(path, self.accessible_roots())
    }

    #[must_use]
    pub fn with_runtime_scope(
        &self,
        run_id: RunId,
        session_id: SessionId,
        turn_id: TurnId,
        tool_name: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        let mut scoped = self.clone();
        scoped.run_id = Some(run_id);
        scoped.session_id = Some(session_id);
        scoped.turn_id = Some(turn_id);
        scoped.tool_name = Some(tool_name.into());
        scoped.tool_call_id = Some(tool_call_id.into());
        scoped
    }
}

#[cfg(test)]
mod tests {
    use super::ToolExecutionContext;
    use agent_core_types::{RunId, SessionId, TurnId};

    #[test]
    fn accessible_roots_include_workspace_worktree_and_additional_roots() {
        let workspace = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let context = ToolExecutionContext {
            workspace_root: workspace.path().join("workspace"),
            worktree_root: Some(workspace.path().join("worktree")),
            sandbox_root: Some(workspace.path().join("sandbox")),
            additional_roots: vec![extra.path().to_path_buf()],
            ..Default::default()
        };

        let roots = context
            .accessible_roots()
            .into_iter()
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(roots[0], workspace.path().join("sandbox"));
        assert_eq!(roots[1], workspace.path().join("worktree"));
        assert_eq!(roots[2], extra.path().to_path_buf());
    }

    #[test]
    fn with_runtime_scope_preserves_existing_context_and_adds_ids() {
        let workspace = tempfile::tempdir().unwrap();
        let base = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        };

        let scoped = base.with_runtime_scope(
            RunId("run_1".to_string()),
            SessionId("session_1".to_string()),
            TurnId("turn_1".to_string()),
            "read",
            "call_1",
        );

        assert_eq!(scoped.workspace_root, base.workspace_root);
        assert!(scoped.workspace_only);
        assert_eq!(scoped.run_id.unwrap().0, "run_1");
        assert_eq!(scoped.session_id.unwrap().0, "session_1");
        assert_eq!(scoped.turn_id.unwrap().0, "turn_1");
        assert_eq!(scoped.tool_name.unwrap(), "read");
        assert_eq!(scoped.tool_call_id.unwrap(), "call_1");
    }
}
