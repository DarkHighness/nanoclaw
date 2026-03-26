//! Shell-local configuration loading for the independent reference TUI crate.
//!
//! This module is intentionally private to the reference shell. Substrate hosts
//! should define their own configuration layer, or none at all.

use agent::{AgentWorkspaceLayout, mcp::McpServerConfig};
use agent_env::{EnvMap, vars};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: Option<ProviderKind>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub additional_params: Option<Value>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub workspace_only: bool,
    pub auto_compact: bool,
    #[serde(default)]
    pub context_tokens: Option<usize>,
    #[serde(default)]
    pub compact_trigger_tokens: Option<usize>,
    #[serde(default)]
    pub compact_preserve_recent_messages: Option<usize>,
    #[serde(default)]
    pub store_dir: Option<String>,
    pub sandbox_fail_if_unavailable: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            workspace_only: true,
            auto_compact: true,
            context_tokens: Some(128_000),
            compact_trigger_tokens: Some(96_000),
            compact_preserve_recent_messages: Some(8),
            store_dir: None,
            sandbox_fail_if_unavailable: false,
        }
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginSlotsConfig {
    #[serde(default)]
    pub memory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PluginEntryConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    pub enabled: bool,
    pub roots: Vec<String>,
    pub include_builtin: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub entries: BTreeMap<String, PluginEntryConfig>,
    pub slots: PluginSlotsConfig,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: Vec::new(),
            include_builtin: true,
            allow: Vec::new(),
            deny: Vec::new(),
            entries: BTreeMap::new(),
            slots: PluginSlotsConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentCoreConfig {
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub hook_env: BTreeMap<String, String>,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub skill_roots: Vec<String>,
    #[serde(default)]
    pub plugins: PluginsConfig,
}

impl AgentCoreConfig {
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let mut config = load_config_file(dir)?;
        let env_map = EnvMap::from_workspace_dir(dir)?;

        if let Some(value) = env_map.get_non_empty_var(vars::AGENT_CORE_PROVIDER) {
            config.provider.kind = match value.trim().to_ascii_lowercase().as_str() {
                "openai" => Some(ProviderKind::OpenAi),
                "anthropic" => Some(ProviderKind::Anthropic),
                _ => config.provider.kind,
            };
        }
        if let Some(value) = env_map.get_non_empty_var(vars::AGENT_CORE_MODEL) {
            config.provider.model = Some(value);
        }
        if let Some(value) = env_map.get_non_empty_var(vars::AGENT_CORE_BASE_URL) {
            config.provider.base_url = Some(value);
        }
        if let Some(parsed) = env_map.get_parsed_var::<f64>(vars::AGENT_CORE_TEMPERATURE) {
            config.provider.temperature = Some(parsed);
        }
        if let Some(parsed) = env_map.get_parsed_var::<u64>(vars::AGENT_CORE_MAX_TOKENS) {
            config.provider.max_tokens = Some(parsed);
        }
        if let Some(value) = env_map.get_raw_var(vars::AGENT_CORE_PROVIDER_ADDITIONAL_PARAMS_JSON)
            && let Ok(parsed) = serde_json::from_str::<Value>(value)
        {
            config.provider.additional_params = Some(parsed);
        }
        if let Some(parsed) = env_map.get_bool_var(vars::AGENT_CORE_WORKSPACE_ONLY) {
            config.runtime.workspace_only = parsed;
        }
        if let Some(parsed) = env_map.get_bool_var(vars::AGENT_CORE_AUTO_COMPACT) {
            config.runtime.auto_compact = parsed;
        }
        if let Some(parsed) = env_map.get_parsed_var::<usize>(vars::AGENT_CORE_CONTEXT_TOKENS) {
            config.runtime.context_tokens = Some(parsed);
        }
        if let Some(parsed) =
            env_map.get_parsed_var::<usize>(vars::AGENT_CORE_COMPACT_TRIGGER_TOKENS)
        {
            config.runtime.compact_trigger_tokens = Some(parsed);
        }
        if let Some(parsed) =
            env_map.get_parsed_var::<usize>(vars::AGENT_CORE_COMPACT_PRESERVE_RECENT_MESSAGES)
        {
            config.runtime.compact_preserve_recent_messages = Some(parsed);
        }
        if let Some(value) = env_map.get_non_empty_var(vars::AGENT_CORE_STORE_DIR) {
            config.runtime.store_dir = Some(value);
        }
        if let Some(parsed) = env_map.get_bool_var(vars::AGENT_CORE_SANDBOX_FAIL_IF_UNAVAILABLE) {
            config.runtime.sandbox_fail_if_unavailable = parsed;
        }
        if let Some(value) = env_map.get_non_empty_var(vars::AGENT_CORE_COMMAND_PREFIX) {
            config.tui.command_prefix = value;
        }
        if let Some(value) = env_map.get_non_empty_var(vars::AGENT_CORE_SYSTEM_PROMPT) {
            config.system_prompt = Some(value);
        }
        if let Some(value) = env_map.get_raw_var(vars::AGENT_CORE_SKILL_ROOTS) {
            config.skill_roots = split_env_paths(value);
        }
        for (key, value) in env_map.iter() {
            if key.starts_with("AGENT_CORE_HOOK_ENV_") {
                config.hook_env.insert(
                    key.trim_start_matches("AGENT_CORE_HOOK_ENV_").to_string(),
                    value.clone(),
                );
            }
        }
        dedup_skill_roots(&mut config.skill_roots);
        dedup_paths(&mut config.plugins.roots);
        Ok(config)
    }

    pub fn with_override(mut self, update: impl FnOnce(&mut Self)) -> Self {
        update(&mut self);
        self
    }

    #[must_use]
    pub fn config_path(dir: impl AsRef<Path>) -> Option<PathBuf> {
        AgentWorkspaceLayout::new(dir).config_path()
    }

    #[must_use]
    pub fn resolved_skill_roots(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        self.skill_roots
            .iter()
            .map(|entry| resolve_relative_path(dir.as_ref(), entry))
            .collect()
    }

    #[must_use]
    pub fn resolved_store_dir(&self, dir: impl AsRef<Path>) -> PathBuf {
        self.runtime
            .store_dir
            .as_deref()
            .map(|entry| resolve_relative_path(dir.as_ref(), entry))
            .unwrap_or_else(|| AgentWorkspaceLayout::new(dir).store_dir())
    }

    #[must_use]
    pub fn resolved_plugin_roots(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        self.plugins
            .roots
            .iter()
            .map(|entry| resolve_relative_path(dir.as_ref(), entry))
            .collect()
    }
}

fn load_config_file(dir: &Path) -> Result<AgentCoreConfig> {
    let Some(path) = AgentCoreConfig::config_path(dir) else {
        return Ok(AgentCoreConfig::default());
    };
    let raw = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&raw)?)
}

fn split_env_paths(value: &str) -> Vec<String> {
    agent_env::split_path_list(value)
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

fn dedup_skill_roots(values: &mut Vec<String>) {
    dedup_paths(values);
}

fn dedup_paths(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|entry| seen.insert(entry.to_string()));
}

fn resolve_relative_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentCoreConfig, ProviderKind};
    use agent::AgentWorkspaceLayout;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;
    use tokio::fs;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn loads_dotenv_precedence() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "AGENT_CORE_MODEL=from_env\nAGENT_CORE_WORKSPACE_ONLY=false\nAGENT_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=true\n",
        )
        .await
        .unwrap();
        fs::write(
            dir.path().join(".env.local"),
            "AGENT_CORE_MODEL=from_local\nAGENT_CORE_COMPACT_PRESERVE_RECENT_MESSAGES=6\n",
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
    async fn loads_toml_config_and_resolves_skill_roots() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::create_dir_all(AgentWorkspaceLayout::new(dir.path()).state_dir())
            .await
            .unwrap();
        fs::write(
            dir.path().join("agent-core.toml"),
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

                [tui]
                command_prefix = ":"

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
    async fn accepts_openai_provider_alias_in_toml() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("agent-core.toml"),
            r#"
                [provider]
                kind = "openai"
                model = "gpt-4.1-mini"
            "#,
        )
        .await
        .unwrap();

        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        assert_eq!(config.provider.kind, Some(ProviderKind::OpenAi));
    }

    #[tokio::test]
    async fn runtime_and_tui_tables_can_override_partial_fields() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("agent-core.toml"),
            r#"
                [runtime]
                store_dir = ".nanoclaw/store"

                [tui]
                command_prefix = ":"
            "#,
        )
        .await
        .unwrap();

        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        assert!(config.runtime.workspace_only);
        assert!(!config.runtime.sandbox_fail_if_unavailable);
        assert_eq!(config.runtime.store_dir.as_deref(), Some(".nanoclaw/store"));
        assert_eq!(config.tui.command_prefix, ":");
    }

    #[tokio::test]
    async fn provider_additional_params_can_be_overridden_from_env_json() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("agent-core.toml"),
            r#"
                [provider]
                model = "gpt-4.1-mini"
                additional_params = { metadata = { tier = "standard" } }
            "#,
        )
        .await
        .unwrap();

        unsafe {
            std::env::set_var(
                "AGENT_CORE_PROVIDER_ADDITIONAL_PARAMS_JSON",
                r#"{"metadata":{"tier":"priority"},"response_format":{"type":"json_object"}}"#,
            );
        }
        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        unsafe {
            std::env::remove_var("AGENT_CORE_PROVIDER_ADDITIONAL_PARAMS_JSON");
        }

        assert_eq!(
            config.provider.additional_params,
            Some(serde_json::json!({
                "metadata": { "tier": "priority" },
                "response_format": { "type": "json_object" }
            }))
        );
    }

    #[tokio::test]
    async fn system_prompt_can_be_overridden_from_env() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("agent-core.toml"),
            r#"
                system_prompt = "from config"
            "#,
        )
        .await
        .unwrap();

        unsafe {
            std::env::set_var("AGENT_CORE_SYSTEM_PROMPT", "from env");
        }
        let config = AgentCoreConfig::load_from_dir(dir.path()).unwrap();
        unsafe {
            std::env::remove_var("AGENT_CORE_SYSTEM_PROMPT");
        }

        assert_eq!(config.system_prompt.as_deref(), Some("from env"));
    }
}
