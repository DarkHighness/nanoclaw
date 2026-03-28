use crate::Result;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use types::{AgentId, CallId, RunId, SessionId, ToolName, TurnId};

pub trait ToolWriteGuard: Send + Sync {
    fn assert_write_paths(&self, agent_id: Option<&AgentId>, paths: &[PathBuf]) -> Result<()>;
}

#[derive(Clone, Default)]
pub struct ToolExecutionContext {
    pub workspace_root: PathBuf,
    pub worktree_root: Option<PathBuf>,
    pub sandbox_root: Option<PathBuf>,
    pub additional_roots: Vec<PathBuf>,
    pub read_only_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub exec_roots: Vec<PathBuf>,
    pub network_policy: Option<sandbox::NetworkPolicy>,
    pub workspace_only: bool,
    pub container_workdir: Option<String>,
    pub model_context_window_tokens: Option<usize>,
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub agent_id: Option<AgentId>,
    pub tool_name: Option<ToolName>,
    pub tool_call_id: Option<CallId>,
    pub write_guard: Option<Arc<dyn ToolWriteGuard>>,
}

impl fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("workspace_root", &self.workspace_root)
            .field("worktree_root", &self.worktree_root)
            .field("sandbox_root", &self.sandbox_root)
            .field("additional_roots", &self.additional_roots)
            .field("workspace_only", &self.workspace_only)
            .field("container_workdir", &self.container_workdir)
            .field(
                "model_context_window_tokens",
                &self.model_context_window_tokens,
            )
            .field("run_id", &self.run_id)
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("agent_id", &self.agent_id)
            .field("tool_name", &self.tool_name)
            .field("tool_call_id", &self.tool_call_id)
            .finish_non_exhaustive()
    }
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
        let mut roots = vec![self.effective_root()];
        if let Some(worktree_root) = self.worktree_root.as_deref() {
            roots.push(worktree_root);
        }
        roots.extend(self.additional_roots.iter().map(PathBuf::as_path));
        roots.extend(self.read_only_roots.iter().map(PathBuf::as_path));
        roots.extend(self.writable_roots.iter().map(PathBuf::as_path));
        roots.extend(self.exec_roots.iter().map(PathBuf::as_path));
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
        if let Some(write_guard) = &self.write_guard {
            write_guard.assert_write_paths(self.agent_id.as_ref(), &[path.to_path_buf()])?;
        }
        Ok(())
    }

    pub fn assert_path_execute_allowed(&self, path: &Path) -> Result<()> {
        if self.exec_roots.is_empty() {
            return Err(crate::ToolError::invalid(format!(
                "execute access is not granted for {}",
                path.display()
            )));
        }
        sandbox::assert_filesystem_access(
            &self.sandbox_policy(),
            path,
            sandbox::FilesystemAccess::Execute,
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
            read_only_roots: self.read_only_roots.clone(),
            writable_roots: self.writable_roots.clone(),
            exec_roots: self.exec_roots.clone(),
            network_policy: self.network_policy.clone(),
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

    #[must_use]
    pub fn with_agent_scope(
        &self,
        agent_id: AgentId,
        write_guard: Arc<dyn ToolWriteGuard>,
    ) -> Self {
        let mut scoped = self.clone();
        scoped.agent_id = Some(agent_id);
        scoped.write_guard = Some(write_guard);
        scoped
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolExecutionContext, ToolWriteGuard};
    use crate::{Result, ToolError};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use types::{AgentId, CallId, RunId, SessionId, ToolName, TurnId};

    #[derive(Default)]
    struct RecordingWriteGuard {
        calls: Mutex<Vec<(Option<AgentId>, Vec<PathBuf>)>>,
    }

    impl ToolWriteGuard for RecordingWriteGuard {
        fn assert_write_paths(&self, agent_id: Option<&AgentId>, paths: &[PathBuf]) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((agent_id.cloned(), paths.to_vec()));
            Ok(())
        }
    }

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

    #[test]
    fn write_guard_observes_agent_scoped_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let guard = Arc::new(RecordingWriteGuard::default());
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        }
        .with_agent_scope(AgentId::from("agent_1"), guard.clone());

        let target = workspace.path().join("src/lib.rs");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "ok").unwrap();
        context.assert_path_write_allowed(&target).unwrap();

        let calls = guard.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Some(AgentId::from("agent_1")));
        assert_eq!(calls[0].1, vec![target]);
    }

    #[test]
    fn write_guard_failure_bubbles_out() {
        struct RejectingWriteGuard;
        impl ToolWriteGuard for RejectingWriteGuard {
            fn assert_write_paths(
                &self,
                _agent_id: Option<&AgentId>,
                _paths: &[PathBuf],
            ) -> Result<()> {
                Err(ToolError::invalid_state("lease conflict"))
            }
        }

        let workspace = tempfile::tempdir().unwrap();
        let target = workspace.path().join("file.txt");
        std::fs::write(&target, "ok").unwrap();
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        }
        .with_agent_scope(AgentId::from("agent_1"), Arc::new(RejectingWriteGuard));

        let error = context.assert_path_write_allowed(&target).unwrap_err();
        assert!(error.to_string().contains("lease conflict"));
    }
}
