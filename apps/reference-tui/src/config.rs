//! App-specific config merging for the reference TUI shell.
//!
//! Core substrate behavior is loaded from `nanoclaw-config`, while this module
//! owns only the reference shell's private TOML/env surface.

use agent::mcp::McpServerConfig;
use agent_env::{EnvMap, EnvVar};
use anyhow::Result;
use nanoclaw_config::{
    CoreConfig, PluginsConfig, ProviderConfig, RuntimeConfig, app_config_path,
    load_optional_app_config,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const REFERENCE_TUI_APP_NAME: &str = "reference-tui";
const REFERENCE_TUI_COMMAND_PREFIX: EnvVar = EnvVar::new(
    "NANOCLAW_REFERENCE_TUI_COMMAND_PREFIX",
    "Slash-command prefix override for the reference-tui app.",
);

pub use nanoclaw_config::ProviderKind;

#[cfg_attr(not(test), allow(dead_code))]
pub fn core_config_path(dir: impl AsRef<Path>) -> PathBuf {
    nanoclaw_config::core_config_path(dir)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    pub command_prefix: String,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            command_prefix: "/".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ReferenceTuiAppConfig {
    pub tui: TuiConfig,
}

#[derive(Clone, Debug, Default)]
pub struct AgentCoreConfig {
    pub runtime: RuntimeConfig,
    pub provider: ProviderConfig,
    pub mcp_servers: Vec<McpServerConfig>,
    pub hook_env: BTreeMap<String, String>,
    pub tui: TuiConfig,
    pub system_prompt: Option<String>,
    pub skill_roots: Vec<String>,
    pub plugins: PluginsConfig,
}

impl AgentCoreConfig {
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let core = CoreConfig::load_from_dir(dir)?;
        let mut app =
            load_optional_app_config::<ReferenceTuiAppConfig>(dir, REFERENCE_TUI_APP_NAME)?;
        let env_map = EnvMap::from_workspace_dir(dir)?;
        if let Some(value) = env_map.get_non_empty_var(REFERENCE_TUI_COMMAND_PREFIX) {
            app.tui.command_prefix = value;
        }

        let mut merged = Self::from_core(core);
        merged.tui = app.tui;
        Ok(merged)
    }

    pub fn with_override(mut self, update: impl FnOnce(&mut Self)) -> Self {
        update(&mut self);
        self
    }

    #[must_use]
    pub fn app_config_path(dir: impl AsRef<Path>) -> PathBuf {
        app_config_path(dir, REFERENCE_TUI_APP_NAME)
    }

    #[must_use]
    pub fn resolved_skill_roots(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        self.as_core_config().resolved_skill_roots(dir)
    }

    #[must_use]
    pub fn resolved_store_dir(&self, dir: impl AsRef<Path>) -> PathBuf {
        self.as_core_config().resolved_store_dir(dir)
    }

    #[must_use]
    pub fn resolved_plugin_roots(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        self.as_core_config().resolved_plugin_roots(dir)
    }

    fn from_core(core: CoreConfig) -> Self {
        Self {
            runtime: core.runtime,
            provider: core.provider,
            mcp_servers: core.mcp_servers,
            hook_env: core.hook_env,
            tui: TuiConfig::default(),
            system_prompt: core.system_prompt,
            skill_roots: core.skill_roots,
            plugins: core.plugins,
        }
    }

    fn as_core_config(&self) -> CoreConfig {
        CoreConfig {
            runtime: self.runtime.clone(),
            provider: self.provider.clone(),
            mcp_servers: self.mcp_servers.clone(),
            hook_env: self.hook_env.clone(),
            system_prompt: self.system_prompt.clone(),
            skill_roots: self.skill_roots.clone(),
            plugins: self.plugins.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentCoreConfig, ProviderKind, REFERENCE_TUI_COMMAND_PREFIX};
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;
    use tokio::fs;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn merges_core_and_reference_tui_configs() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/config"))
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/apps"))
            .await
            .unwrap();
        fs::write(
            super::core_config_path(dir.path()),
            r#"
                system_prompt = "Work carefully and be concise."
                skill_roots = ["skills", "/tmp/global-skills"]

                [provider]
                kind = "anthropic"
                model = "claude-3-7-sonnet"
                temperature = 0.2
                max_tokens = 4096
                additional_params = { metadata = { tier = "standard" } }

                [runtime]
                workspace_only = false
                compact_preserve_recent_messages = 5
                store_dir = ".nanoclaw/custom-store"
                sandbox_fail_if_unavailable = true

                [plugins]
                roots = ["plugins", "/tmp/global-plugins"]
                allow = ["memory-core"]
                include_builtin = true

                [plugins.slots]
                memory = "memory-core"

                [plugins.entries.memory-core]
                enabled = true

                [plugins.entries.memory-core.config]
                vector_store = { kind = "sqlite", path = ".nanoclaw/memory/indexes/test.sqlite" }
            "#,
        )
        .await
        .unwrap();
        fs::write(
            AgentCoreConfig::app_config_path(dir.path()),
            r#"
                [tui]
                command_prefix = ":"
            "#,
        )
        .await
        .unwrap();

        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        assert_eq!(config.provider.kind, Some(ProviderKind::Anthropic));
        assert_eq!(config.provider.model.as_deref(), Some("claude-3-7-sonnet"));
        assert_eq!(config.provider.temperature, Some(0.2));
        assert_eq!(config.provider.max_tokens, Some(4096));
        assert_eq!(
            config.provider.additional_params,
            Some(serde_json::json!({"metadata":{"tier":"standard"}}))
        );
        assert!(!config.runtime.workspace_only);
        assert_eq!(config.runtime.compact_preserve_recent_messages, Some(5));
        assert_eq!(
            config.runtime.store_dir.as_deref(),
            Some(".nanoclaw/custom-store")
        );
        assert!(config.runtime.sandbox_fail_if_unavailable);
        assert_eq!(config.tui.command_prefix, ":");
        assert_eq!(
            config.system_prompt.as_deref(),
            Some("Work carefully and be concise.")
        );

        let skill_roots = config.resolved_skill_roots(dir.path());
        assert_eq!(skill_roots[0], dir.path().join("skills"));
        assert_eq!(skill_roots[1], PathBuf::from("/tmp/global-skills"));
        let plugin_roots = config.resolved_plugin_roots(dir.path());
        assert_eq!(plugin_roots[0], dir.path().join("plugins"));
        assert_eq!(plugin_roots[1], PathBuf::from("/tmp/global-plugins"));
        assert_eq!(config.plugins.allow, vec!["memory-core".to_string()]);
        assert_eq!(config.plugins.slots.memory.as_deref(), Some("memory-core"));
        assert_eq!(
            config
                .plugins
                .entries
                .get("memory-core")
                .and_then(|entry| entry.enabled),
            Some(true)
        );
        assert_eq!(
            config
                .plugins
                .entries
                .get("memory-core")
                .and_then(|entry| entry.config.get("vector_store"))
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("path"))
                .and_then(toml::Value::as_str),
            Some(".nanoclaw/memory/indexes/test.sqlite")
        );
        assert_eq!(
            config.resolved_store_dir(dir.path()),
            dir.path().join(".nanoclaw/custom-store")
        );
    }

    #[tokio::test]
    async fn core_env_overrides_flow_through_merged_config() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "NANOCLAW_CORE_MODEL=from_env\nNANOCLAW_CORE_WORKSPACE_ONLY=false\nNANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=true\n",
        )
        .await
        .unwrap();
        fs::write(
            dir.path().join(".env.local"),
            "NANOCLAW_CORE_MODEL=from_local\nNANOCLAW_CORE_COMPACT_PRESERVE_RECENT_MESSAGES=6\n",
        )
        .await
        .unwrap();

        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        assert_eq!(config.provider.model.as_deref(), Some("from_local"));
        assert_eq!(config.runtime.compact_preserve_recent_messages, Some(6));
        assert!(!config.runtime.workspace_only);
        assert!(config.runtime.sandbox_fail_if_unavailable);
    }

    #[tokio::test]
    async fn reference_tui_command_prefix_can_be_overridden_from_env() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/apps"))
            .await
            .unwrap();
        fs::write(
            AgentCoreConfig::app_config_path(dir.path()),
            r#"
                [tui]
                command_prefix = ":"
            "#,
        )
        .await
        .unwrap();

        unsafe {
            std::env::set_var(REFERENCE_TUI_COMMAND_PREFIX.key, "!");
        }
        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        unsafe {
            std::env::remove_var(REFERENCE_TUI_COMMAND_PREFIX.key);
        }

        assert_eq!(config.tui.command_prefix, "!");
    }
}
