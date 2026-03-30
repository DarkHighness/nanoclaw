//! App-specific config loading for the code-agent example.
//!
//! Core runtime config comes from `nanoclaw-config`. This module owns only the
//! code-agent-specific LSP helper settings layered on top of the shared core
//! config surface.

use crate::statusline::StatusLineConfig;
use agent_env::{EnvMap, EnvVar};
use anyhow::Result;
use nanoclaw_config::{CoreConfig, load_optional_app_config};
use serde::Deserialize;
use std::path::{Path, PathBuf};

const CODE_AGENT_APP_NAME: &str = "code-agent";
const CODE_AGENT_LSP_ENABLED: EnvVar = EnvVar::new(
    "CODE_AGENT_LSP_ENABLED",
    "Whether code-agent should enable managed LSP-backed code-intel with lexical fallback.",
);
const CODE_AGENT_LSP_AUTO_INSTALL: EnvVar = EnvVar::new(
    "CODE_AGENT_LSP_AUTO_INSTALL",
    "Whether code-agent may auto-install supported LSP servers into the managed workspace cache.",
);
const CODE_AGENT_LSP_INSTALL_ROOT: EnvVar = EnvVar::new(
    "CODE_AGENT_LSP_INSTALL_ROOT",
    "Optional override for the managed LSP install/cache directory used by code-agent.",
);

#[derive(Clone, Debug)]
pub(crate) struct CodeAgentConfig {
    pub core: CoreConfig,
    pub lsp_enabled: bool,
    pub lsp_auto_install: bool,
    pub lsp_install_root: Option<PathBuf>,
    pub statusline: StatusLineConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentAppConfig {
    lsp: CodeAgentLspConfig,
    tui: CodeAgentTuiConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentTuiConfig {
    statusline: StatusLineConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct CodeAgentLspConfig {
    enabled: bool,
    auto_install: bool,
    install_root: Option<String>,
}

impl Default for CodeAgentLspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_install: false,
            install_root: None,
        }
    }
}

impl CodeAgentConfig {
    pub(crate) fn load_from_dir(workspace_root: &Path, env_map: &EnvMap) -> Result<Self> {
        let core = CoreConfig::load_from_dir(workspace_root)?;
        let mut app =
            load_optional_app_config::<CodeAgentAppConfig>(workspace_root, CODE_AGENT_APP_NAME)?;
        if let Some(parsed) = env_map.get_bool_var(CODE_AGENT_LSP_ENABLED) {
            app.lsp.enabled = parsed;
        }
        if let Some(parsed) = env_map.get_bool_var(CODE_AGENT_LSP_AUTO_INSTALL) {
            app.lsp.auto_install = parsed;
        }
        if let Some(value) = env_map.get_non_empty_var(CODE_AGENT_LSP_INSTALL_ROOT) {
            app.lsp.install_root = Some(value);
        }

        Ok(Self {
            core,
            lsp_enabled: app.lsp.enabled,
            lsp_auto_install: app.lsp.auto_install,
            lsp_install_root: app
                .lsp
                .install_root
                .as_deref()
                .map(|value| resolve_path(workspace_root, value)),
            statusline: app.tui.statusline,
        })
    }
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::CodeAgentConfig;
    use agent_env::EnvMap;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn loads_lsp_flags_from_env() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            format!(
                "OPENAI_API_KEY=test-key\nCODE_AGENT_LSP_ENABLED=false\nCODE_AGENT_LSP_AUTO_INSTALL=true\nCODE_AGENT_LSP_INSTALL_ROOT={}\n",
                dir.path().join(".cache/lsp").display()
            ),
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert!(!config.lsp_enabled);
        assert!(config.lsp_auto_install);
        assert_eq!(config.lsp_install_root, Some(dir.path().join(".cache/lsp")));
    }

    #[tokio::test]
    async fn loads_statusline_flags_from_app_config() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"
                [tui.statusline]
                model = false
                repo = true
                branch = false
                clock = false
                session = true
            "#,
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert!(!config.statusline.model);
        assert!(config.statusline.repo);
        assert!(!config.statusline.branch);
        assert!(!config.statusline.clock);
        assert!(config.statusline.session);
    }
}
