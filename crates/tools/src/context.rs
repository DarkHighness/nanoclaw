use crate::Result;
use std::path::{Path, PathBuf};
use types::{CallId, RunId, SessionId, ToolName, TurnId};

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
    pub tool_name: Option<ToolName>,
    pub tool_call_id: Option<CallId>,
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
        // This remains a useful debug/introspection view for host code, but
        // tool enforcement should go through the sandbox-derived access checks
        // below so protected subpaths such as `.nanoclaw` stay consistent with
        // process sandboxing.
        let mut roots = vec![self.effective_root()];
        if let Some(worktree_root) = self.worktree_root.as_deref() {
            roots.push(worktree_root);
        }
        roots.extend(self.additional_roots.iter().map(PathBuf::as_path));
        roots
    }

    #[must_use]
    pub fn sandbox_policy(&self) -> sandbox::SandboxPolicy {
        self.sandbox_scope().recommended_policy()
    }

    pub fn assert_path_read_allowed(&self, path: &Path) -> Result<()> {
        sandbox::assert_filesystem_access(
            &self.sandbox_policy(),
            path,
            sandbox::FilesystemAccess::Read,
        )?;
        Ok(())
    }

    pub fn assert_path_write_allowed(&self, path: &Path) -> Result<()> {
        sandbox::assert_filesystem_access(
            &self.sandbox_policy(),
            path,
            sandbox::FilesystemAccess::Write,
        )?;
        Ok(())
    }

    pub fn assert_path_allowed(&self, path: &Path) -> Result<()> {
        self.assert_path_read_allowed(path)
    }

    #[must_use]
    pub fn sandbox_scope(&self) -> sandbox::SandboxScope {
        sandbox::SandboxScope {
            workspace_root: self.effective_root().to_path_buf(),
            worktree_root: self.worktree_root.clone(),
            additional_roots: self.additional_roots.clone(),
            workspace_only: self.workspace_only,
        }
    }

    #[must_use]
    pub fn with_runtime_scope(
        &self,
        run_id: RunId,
        session_id: SessionId,
        turn_id: TurnId,
        tool_name: impl Into<ToolName>,
        tool_call_id: impl Into<CallId>,
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
    use types::{CallId, RunId, SessionId, ToolName, TurnId};

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
            RunId::from("run_1"),
            SessionId::from("session_1"),
            TurnId::from("turn_1"),
            "read",
            "call_1",
        );

        assert_eq!(scoped.workspace_root, base.workspace_root);
        assert!(scoped.workspace_only);
        assert_eq!(scoped.run_id.unwrap().as_str(), "run_1");
        assert_eq!(scoped.session_id.unwrap().as_str(), "session_1");
        assert_eq!(scoped.turn_id.unwrap().as_str(), "turn_1");
        assert_eq!(scoped.tool_name.unwrap(), ToolName::from("read"));
        assert_eq!(scoped.tool_call_id.unwrap(), CallId::from("call_1"));
    }

    #[test]
    fn sandbox_access_checks_protect_internal_state_paths() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".nanoclaw")).unwrap();
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };

        assert!(
            context
                .assert_path_read_allowed(&workspace.path().join(".nanoclaw/state.toml"))
                .is_ok()
        );
        assert!(
            context
                .assert_path_write_allowed(&workspace.path().join(".nanoclaw/state.toml"))
                .is_err()
        );
    }
}
