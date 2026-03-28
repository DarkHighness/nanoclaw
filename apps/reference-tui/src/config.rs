//! App-specific config merging for the reference TUI shell.
//!
//! Core substrate behavior is loaded from `nanoclaw-config`, while this module
//! owns only the reference shell's private TOML/env surface.

use agent_env::{EnvMap, EnvVar};
use anyhow::Result;
use nanoclaw_config::{
    CoreConfig, PluginsConfig, ResolvedAgentProfile, ResolvedInternalProfile, app_config_path,
    load_optional_app_config,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const REFERENCE_TUI_APP_NAME: &str = "reference-tui";
const REFERENCE_TUI_COMMAND_PREFIX: EnvVar = EnvVar::new(
    "NANOCLAW_REFERENCE_TUI_COMMAND_PREFIX",
    "Slash-command prefix override for the reference-tui app.",
);

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

#[derive(Clone, Debug)]
pub struct AgentCoreConfig {
    pub core: CoreConfig,
    pub primary_profile: ResolvedAgentProfile,
    pub summary_profile: ResolvedInternalProfile,
    pub memory_profile: ResolvedInternalProfile,
    pub tui: TuiConfig,
}

impl Default for AgentCoreConfig {
    fn default() -> Self {
        let core = CoreConfig::default();
        Self::from_core_and_tui(core, TuiConfig::default()).expect("default core config is valid")
    }
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
        Self::from_core_and_tui(core, app.tui)
    }

    pub fn with_override(mut self, update: impl FnOnce(&mut Self)) -> Self {
        update(&mut self);
        self.refresh_resolved_profiles()
            .expect("AgentCoreConfig overrides must remain valid");
        self
    }

    #[must_use]
    pub fn app_config_path(dir: impl AsRef<Path>) -> PathBuf {
        app_config_path(dir, REFERENCE_TUI_APP_NAME)
    }

    #[must_use]
    pub fn resolved_skill_roots(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        self.core.resolved_skill_roots(dir)
    }

    #[must_use]
    pub fn resolved_store_dir(&self, dir: impl AsRef<Path>) -> PathBuf {
        self.core.resolved_store_dir(dir)
    }

    #[must_use]
    pub fn resolved_plugin_roots(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        self.core.resolved_plugin_roots(dir)
    }

    #[must_use]
    pub fn plugins(&self) -> &PluginsConfig {
        &self.core.plugins
    }

    fn from_core_and_tui(core: CoreConfig, tui: TuiConfig) -> Result<Self> {
        let primary_profile = core.resolve_primary_agent()?;
        let summary_profile = core.resolve_summary_profile()?;
        let memory_profile = core.resolve_memory_profile()?;
        Ok(Self {
            core,
            primary_profile,
            summary_profile,
            memory_profile,
            tui,
        })
    }

    fn refresh_resolved_profiles(&mut self) -> Result<()> {
        self.primary_profile = self.core.resolve_primary_agent()?;
        self.summary_profile = self.core.resolve_summary_profile()?;
        self.memory_profile = self.core.resolve_memory_profile()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentCoreConfig, REFERENCE_TUI_COMMAND_PREFIX};
    use crate::test_support::lock_env_test;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn merges_core_and_reference_tui_configs() {
        let _guard = lock_env_test();
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
                global_system_prompt = "Work carefully and be concise."
                skill_roots = ["skills", "/tmp/global-skills"]

                [host]
                workspace_only = false
                store_dir = ".nanoclaw/custom-store"
                sandbox_fail_if_unavailable = true

                [models.gpt_5_4_default]
                provider = "openai"
                model = "gpt-5.4"
                context_window_tokens = 400000
                max_output_tokens = 128000
                compact_trigger_tokens = 320000
                additional_params = { metadata = { tier = "standard" } }

                [models.fast_review]
                provider = "anthropic"
                model = "claude-sonnet-4-6"
                context_window_tokens = 200000
                max_output_tokens = 4096
                compact_trigger_tokens = 160000
                temperature = 0.2

                [agents.primary]
                model = "fast_review"
                sandbox = "workspace_write"
                system_prompt = "Primary prompt."
                compact_preserve_recent_messages = 5

                [agents.subagent_defaults]
                model = "gpt_5_4_default"
                sandbox = "read_only"

                [internal.summary]
                model = "gpt_5_4_default"
                max_output_tokens = 32000

                [internal.memory]
                model = "gpt_5_4_default"
                max_output_tokens = 24000

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
        assert_eq!(config.primary_profile.model.model, "claude-sonnet-4-6");
        assert_eq!(
            config.primary_profile.global_system_prompt.as_deref(),
            Some("Work carefully and be concise.")
        );
        assert_eq!(config.primary_profile.temperature, Some(0.2));
        assert_eq!(config.primary_profile.max_output_tokens, 4096);
        assert_eq!(config.primary_profile.compact_preserve_recent_messages, 5);
        assert!(!config.core.host.workspace_only);
        assert_eq!(
            config.core.host.store_dir.as_deref(),
            Some(".nanoclaw/custom-store")
        );
        assert!(config.core.host.sandbox_fail_if_unavailable);
        assert_eq!(config.tui.command_prefix, ":");

        let skill_roots = config.resolved_skill_roots(dir.path());
        assert_eq!(skill_roots[0], dir.path().join("skills"));
        assert_eq!(skill_roots[1], PathBuf::from("/tmp/global-skills"));
        let plugin_roots = config.resolved_plugin_roots(dir.path());
        assert_eq!(plugin_roots[0], dir.path().join("plugins"));
        assert_eq!(plugin_roots[1], PathBuf::from("/tmp/global-plugins"));
        assert_eq!(config.plugins().allow, vec!["memory-core".to_string()]);
        assert_eq!(
            config.plugins().slots.memory.as_deref(),
            Some("memory-core")
        );
        assert_eq!(
            config.resolved_store_dir(dir.path()),
            dir.path().join(".nanoclaw/custom-store")
        );
    }

    #[tokio::test]
    async fn core_env_overrides_flow_through_merged_config() {
        let _guard = lock_env_test();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "NANOCLAW_CORE_WORKSPACE_ONLY=false\nNANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=true\n",
        )
        .await
        .unwrap();
        fs::write(
            dir.path().join(".env.local"),
            "NANOCLAW_CORE_STORE_DIR=.nanoclaw/env-store\n",
        )
        .await
        .unwrap();

        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        assert!(!config.core.host.workspace_only);
        assert!(config.core.host.sandbox_fail_if_unavailable);
        assert_eq!(
            config.core.host.store_dir.as_deref(),
            Some(".nanoclaw/env-store")
        );
    }

    #[tokio::test]
    async fn reference_tui_command_prefix_can_be_overridden_from_env() {
        let _guard = lock_env_test();
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
