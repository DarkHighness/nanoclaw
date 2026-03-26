//! Shared workspace-local state layout helpers.
//!
//! Mutable host state belongs under `.nanoclaw/` so every worktree owns its
//! own logs, config, skills, plugins, and tool sidecars without depending on
//! legacy workspace layout variants.

use std::io;
use std::path::{Path, PathBuf};

pub const NANOCLAW_STATE_DIR_RELATIVE: &str = ".nanoclaw";
pub const NANOCLAW_CONFIG_DIR_RELATIVE: &str = ".nanoclaw/config";
pub const NANOCLAW_CORE_CONFIG_FILE_RELATIVE: &str = ".nanoclaw/config/core.toml";
pub const NANOCLAW_LOGS_DIR_RELATIVE: &str = ".nanoclaw/logs";
pub const NANOCLAW_STORE_DIR_RELATIVE: &str = ".nanoclaw/store";
pub const NANOCLAW_SKILLS_DIR_RELATIVE: &str = ".nanoclaw/skills";
pub const NANOCLAW_TOOLS_DIR_RELATIVE: &str = ".nanoclaw/tools";
pub const NANOCLAW_LSP_DIR_RELATIVE: &str = ".nanoclaw/tools/lsp";
pub const NANOCLAW_PLUGINS_DIR_RELATIVE: &str = ".nanoclaw/plugins";
pub const NANOCLAW_APPS_DIR_RELATIVE: &str = ".nanoclaw/apps";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentWorkspaceTemplate {
    directories: Vec<PathBuf>,
}

impl AgentWorkspaceTemplate {
    #[must_use]
    pub fn directories(&self) -> &[PathBuf] {
        &self.directories
    }

    pub fn materialize(&self) -> io::Result<()> {
        for directory in &self.directories {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }
}

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
    pub fn standard_template(&self) -> AgentWorkspaceTemplate {
        AgentWorkspaceTemplate {
            directories: vec![
                self.state_dir(),
                self.config_dir(),
                self.logs_dir(),
                self.store_dir(),
                self.skills_dir(),
                self.tools_dir(),
                self.lsp_dir(),
                self.plugins_dir(),
                self.apps_dir(),
            ],
        }
    }

    pub fn ensure_standard_layout(&self) -> io::Result<()> {
        self.standard_template().materialize()
    }

    #[must_use]
    pub fn config_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_CONFIG_DIR_RELATIVE)
    }

    #[must_use]
    pub fn core_config_path(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_CORE_CONFIG_FILE_RELATIVE)
    }

    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_LOGS_DIR_RELATIVE)
    }

    #[must_use]
    pub fn store_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_STORE_DIR_RELATIVE)
    }

    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_SKILLS_DIR_RELATIVE)
    }

    #[must_use]
    pub fn tools_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_TOOLS_DIR_RELATIVE)
    }

    #[must_use]
    pub fn lsp_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_LSP_DIR_RELATIVE)
    }

    #[must_use]
    pub fn plugins_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_PLUGINS_DIR_RELATIVE)
    }

    #[must_use]
    pub fn apps_dir(&self) -> PathBuf {
        self.workspace_root.join(NANOCLAW_APPS_DIR_RELATIVE)
    }

    #[must_use]
    pub fn app_config_path(&self, app_name: &str) -> PathBuf {
        self.apps_dir().join(format!("{app_name}.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentWorkspaceLayout, NANOCLAW_APPS_DIR_RELATIVE, NANOCLAW_CONFIG_DIR_RELATIVE,
        NANOCLAW_CORE_CONFIG_FILE_RELATIVE, NANOCLAW_LOGS_DIR_RELATIVE, NANOCLAW_LSP_DIR_RELATIVE,
        NANOCLAW_PLUGINS_DIR_RELATIVE, NANOCLAW_SKILLS_DIR_RELATIVE, NANOCLAW_STORE_DIR_RELATIVE,
        NANOCLAW_TOOLS_DIR_RELATIVE,
    };
    use tempfile::tempdir;

    #[test]
    fn resolves_standard_config_paths() {
        let dir = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(dir.path());
        assert_eq!(
            layout.core_config_path(),
            dir.path().join(NANOCLAW_CORE_CONFIG_FILE_RELATIVE)
        );
        assert_eq!(
            layout.app_config_path("reference-tui"),
            dir.path().join(".nanoclaw/apps/reference-tui.toml")
        );
    }

    #[test]
    fn standard_template_materializes_expected_directories() {
        let dir = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(dir.path());

        layout.ensure_standard_layout().unwrap();

        for relative in [
            NANOCLAW_CONFIG_DIR_RELATIVE,
            NANOCLAW_LOGS_DIR_RELATIVE,
            NANOCLAW_STORE_DIR_RELATIVE,
            NANOCLAW_SKILLS_DIR_RELATIVE,
            NANOCLAW_TOOLS_DIR_RELATIVE,
            NANOCLAW_LSP_DIR_RELATIVE,
            NANOCLAW_PLUGINS_DIR_RELATIVE,
            NANOCLAW_APPS_DIR_RELATIVE,
        ] {
            assert!(dir.path().join(relative).is_dir(), "missing {relative}");
        }
    }
}
