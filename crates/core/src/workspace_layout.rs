//! Shared workspace-local state layout helpers.
//!
//! Mutable host state belongs under `.nanoclaw/` so every worktree owns its
//! own logs, stores, skills, and memory sidecars without depending on the
//! legacy `.agent-core/` directory.

use std::path::{Path, PathBuf};

pub const NANOCLAW_STATE_DIR_RELATIVE: &str = ".nanoclaw";
pub const NANOCLAW_CONFIG_FILE_RELATIVE: &str = ".nanoclaw/config.toml";
pub const ROOT_CONFIG_FILE_NAME: &str = "agent-core.toml";

#[derive(Clone, Debug)]
pub struct AgentWorkspaceLayout {
    workspace_root: PathBuf,
}

impl AgentWorkspaceLayout {
    #[must_use]
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_STATE_DIR_RELATIVE)
    }

    #[must_use]
    pub fn config_candidates(&self) -> [PathBuf; 2] {
        [
            self.workspace_root.join(ROOT_CONFIG_FILE_NAME),
            self.workspace_root.join(NANOCLAW_CONFIG_FILE_RELATIVE),
        ]
    }

    #[must_use]
    pub fn config_path(&self) -> Option<PathBuf> {
        self.config_candidates()
            .into_iter()
            .find(|candidate| candidate.exists())
    }

    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir().join("logs")
    }

    #[must_use]
    pub fn store_dir(&self) -> PathBuf {
        self.state_dir().join("store")
    }

    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.state_dir().join("skills")
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentWorkspaceLayout, NANOCLAW_CONFIG_FILE_RELATIVE, ROOT_CONFIG_FILE_NAME};
    use tempfile::tempdir;

    #[test]
    fn prefers_root_config_then_workspace_state_config() {
        let dir = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(dir.path());
        assert!(layout.config_path().is_none());

        std::fs::create_dir_all(dir.path().join(".nanoclaw")).unwrap();
        std::fs::write(dir.path().join(NANOCLAW_CONFIG_FILE_RELATIVE), "").unwrap();
        assert_eq!(
            layout.config_path().unwrap(),
            dir.path().join(NANOCLAW_CONFIG_FILE_RELATIVE)
        );

        std::fs::write(dir.path().join(ROOT_CONFIG_FILE_NAME), "").unwrap();
        assert_eq!(
            layout.config_path().unwrap(),
            dir.path().join(ROOT_CONFIG_FILE_NAME)
        );
    }
}
