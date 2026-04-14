//! Shared environment variable access for substrate crates and core config.
//!
//! This crate keeps env-key knowledge in one place so behavior changes are
//! coordinated across runtimes, tools, and shells.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnvVar {
    pub key: &'static str,
    pub description: &'static str,
}

impl EnvVar {
    pub const fn new(key: &'static str, description: &'static str) -> Self {
        Self { key, description }
    }
}

pub mod vars {
    use super::EnvVar;

    pub const SHELL: EnvVar = EnvVar::new(
        "SHELL",
        "Shell executable used by process tools when no explicit shell is configured.",
    );
    pub const HOME: EnvVar = EnvVar::new(
        "HOME",
        "User home directory used when resolving default skill roots.",
    );
    pub const OPENAI_API_KEY: EnvVar = EnvVar::new(
        "OPENAI_API_KEY",
        "OpenAI API key for OpenAI provider requests.",
    );
    pub const OPENAI_BASE_URL: EnvVar = EnvVar::new(
        "OPENAI_BASE_URL",
        "OpenAI API base URL override. Useful for proxies, gateways, or local-compatible endpoints.",
    );
    pub const ANTHROPIC_API_KEY: EnvVar = EnvVar::new(
        "ANTHROPIC_API_KEY",
        "Anthropic API key for Anthropic provider requests.",
    );
    pub const ANTHROPIC_BASE_URL: EnvVar = EnvVar::new(
        "ANTHROPIC_BASE_URL",
        "Anthropic API base URL override. Useful for proxies, gateways, or local-compatible endpoints.",
    );
    pub const RUST_LOG: EnvVar = EnvVar::new(
        "RUST_LOG",
        "Tracing filter directive used by host apps when initializing structured logs.",
    );
    pub const NANOCLAW_CORE_PROVIDER: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_PROVIDER",
        "Provider override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_MODEL: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_MODEL",
        "Model override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_BASE_URL: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_BASE_URL",
        "Provider base URL override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_TEMPERATURE: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_TEMPERATURE",
        "Sampling temperature override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_MAX_TOKENS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_MAX_TOKENS",
        "Max output token override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_PROVIDER_ADDITIONAL_PARAMS_JSON: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_PROVIDER_ADDITIONAL_PARAMS_JSON",
        "JSON object merged into provider request parameters.",
    );
    pub const NANOCLAW_CORE_WORKSPACE_ONLY: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WORKSPACE_ONLY",
        "Whether tools are restricted to workspace paths.",
    );
    pub const NANOCLAW_CORE_AUTO_COMPACT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_AUTO_COMPACT",
        "Whether runtime auto-compaction is enabled.",
    );
    pub const NANOCLAW_CORE_CONTEXT_TOKENS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_CONTEXT_TOKENS",
        "Context window token budget for runtime compaction.",
    );
    pub const NANOCLAW_CORE_COMPACT_TRIGGER_TOKENS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_COMPACT_TRIGGER_TOKENS",
        "Token threshold to trigger compaction.",
    );
    pub const NANOCLAW_CORE_COMPACT_PRESERVE_RECENT_MESSAGES: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_COMPACT_PRESERVE_RECENT_MESSAGES",
        "Recent message count preserved by compaction.",
    );
    pub const NANOCLAW_CORE_STORE_DIR: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_STORE_DIR",
        "Run-store directory override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE",
        "Whether Nanoclaw core should fail closed when no enforcing sandbox backend is available.",
    );
    pub const NANOCLAW_CORE_TOKIO_WORKER_THREADS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_TOKIO_WORKER_THREADS",
        "Optional Tokio worker-thread count override for host apps that build their own runtimes.",
    );
    pub const NANOCLAW_CORE_TOKIO_MAX_BLOCKING_THREADS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_TOKIO_MAX_BLOCKING_THREADS",
        "Optional Tokio max-blocking-thread override for host apps that build their own runtimes.",
    );
    pub const NANOCLAW_CORE_SYSTEM_PROMPT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_SYSTEM_PROMPT",
        "Additional system prompt override for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_SKILL_ROOTS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_SKILL_ROOTS",
        "OS-path-list of skill roots for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_PLUGIN_ROOTS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_PLUGIN_ROOTS",
        "OS-path-list of plugin roots for Nanoclaw core config loading.",
    );
    pub const NANOCLAW_CORE_PLUGIN_MEMORY_SLOT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_PLUGIN_MEMORY_SLOT",
        "Plugin memory slot override for the Nanoclaw core plugin graph.",
    );
    pub const NANOCLAW_CORE_DISABLED_TOOLS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_DISABLED_TOOLS",
        "Comma-separated tool names removed from the runtime tool surface after startup registration.",
    );

    pub const NANOCLAW_CORE_WEB_ALLOW_PRIVATE_HOSTS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_ALLOW_PRIVATE_HOSTS",
        "Allow web tools to access local/private network hosts.",
    );
    pub const NANOCLAW_CORE_WEB_ALLOWED_DOMAINS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_ALLOWED_DOMAINS",
        "Comma-separated web-tool domain allowlist.",
    );
    pub const NANOCLAW_CORE_WEB_BLOCKED_DOMAINS: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_BLOCKED_DOMAINS",
        "Comma-separated web-tool domain blocklist.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_ENDPOINT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_ENDPOINT",
        "HTTP/HTTPS search endpoint override for the lightweight web_search tool.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_BACKEND: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_BACKEND",
        "Search backend selection for the local web_search tool (`auto`, `bing_rss`, `brave_api`, `exa_api`, or `duckduckgo_html`). Defaults to `auto` when unset.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_API_ENDPOINT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_API_ENDPOINT",
        "Legacy HTTP/HTTPS API endpoint override for hosted web_search backends.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_API_KEY: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_API_KEY",
        "Legacy API key for hosted web_search backends.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_ENDPOINT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_ENDPOINT",
        "HTTP/HTTPS API endpoint override for the Brave hosted web_search backend.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_KEY: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_KEY",
        "API key for the Brave hosted web_search backend.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_EXA_API_ENDPOINT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_EXA_API_ENDPOINT",
        "HTTP/HTTPS API endpoint override for the Exa hosted web_search backend.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_EXA_API_KEY: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_EXA_API_KEY",
        "API key for the Exa hosted web_search backend.",
    );
    pub const NANOCLAW_CORE_WEB_SEARCH_DUCKDUCKGO_ENDPOINT: EnvVar = EnvVar::new(
        "NANOCLAW_CORE_WEB_SEARCH_DUCKDUCKGO_ENDPOINT",
        "HTTP/HTTPS HTML endpoint override for the DuckDuckGo fallback web_search backend.",
    );

    pub const ALL: &[EnvVar] = &[
        SHELL,
        HOME,
        OPENAI_API_KEY,
        OPENAI_BASE_URL,
        ANTHROPIC_API_KEY,
        ANTHROPIC_BASE_URL,
        RUST_LOG,
        NANOCLAW_CORE_PROVIDER,
        NANOCLAW_CORE_MODEL,
        NANOCLAW_CORE_BASE_URL,
        NANOCLAW_CORE_TEMPERATURE,
        NANOCLAW_CORE_MAX_TOKENS,
        NANOCLAW_CORE_PROVIDER_ADDITIONAL_PARAMS_JSON,
        NANOCLAW_CORE_WORKSPACE_ONLY,
        NANOCLAW_CORE_AUTO_COMPACT,
        NANOCLAW_CORE_CONTEXT_TOKENS,
        NANOCLAW_CORE_COMPACT_TRIGGER_TOKENS,
        NANOCLAW_CORE_COMPACT_PRESERVE_RECENT_MESSAGES,
        NANOCLAW_CORE_STORE_DIR,
        NANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE,
        NANOCLAW_CORE_TOKIO_WORKER_THREADS,
        NANOCLAW_CORE_TOKIO_MAX_BLOCKING_THREADS,
        NANOCLAW_CORE_SYSTEM_PROMPT,
        NANOCLAW_CORE_SKILL_ROOTS,
        NANOCLAW_CORE_PLUGIN_ROOTS,
        NANOCLAW_CORE_PLUGIN_MEMORY_SLOT,
        NANOCLAW_CORE_DISABLED_TOOLS,
        NANOCLAW_CORE_WEB_ALLOW_PRIVATE_HOSTS,
        NANOCLAW_CORE_WEB_ALLOWED_DOMAINS,
        NANOCLAW_CORE_WEB_BLOCKED_DOMAINS,
        NANOCLAW_CORE_WEB_SEARCH_ENDPOINT,
        NANOCLAW_CORE_WEB_SEARCH_BACKEND,
        NANOCLAW_CORE_WEB_SEARCH_API_ENDPOINT,
        NANOCLAW_CORE_WEB_SEARCH_API_KEY,
        NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_ENDPOINT,
        NANOCLAW_CORE_WEB_SEARCH_BRAVE_API_KEY,
        NANOCLAW_CORE_WEB_SEARCH_EXA_API_ENDPOINT,
        NANOCLAW_CORE_WEB_SEARCH_EXA_API_KEY,
        NANOCLAW_CORE_WEB_SEARCH_DUCKDUCKGO_ENDPOINT,
    ];
}

#[derive(Debug, Error)]
pub enum EnvError {
    #[error("failed to read dotenv file `{path}`: {source}")]
    DotenvLoad {
        path: String,
        #[source]
        source: dotenvy::Error,
    },
}

pub type Result<T> = std::result::Result<T, EnvError>;

#[derive(Clone, Debug, Default)]
pub struct EnvMap {
    values: BTreeMap<String, String>,
}

impl EnvMap {
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            values: std::env::vars().collect(),
        }
    }

    pub fn from_workspace_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let mut values = BTreeMap::new();
        load_dotenv_file(dir.as_ref().join(".env"), &mut values)?;
        load_dotenv_file(dir.as_ref().join(".env.local"), &mut values)?;
        // Process env wins over dotenv so host-level overrides remain authoritative.
        values.extend(std::env::vars());
        Ok(Self { values })
    }

    #[must_use]
    pub fn get_raw(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn get_raw_var(&self, variable: EnvVar) -> Option<&str> {
        self.get_raw(variable.key)
    }

    #[must_use]
    pub fn get_non_empty(&self, key: &str) -> Option<String> {
        self.get_raw(key)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    #[must_use]
    pub fn get_non_empty_var(&self, variable: EnvVar) -> Option<String> {
        self.get_non_empty(variable.key)
    }

    #[must_use]
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get_raw(key).and_then(parse_bool_value)
    }

    #[must_use]
    pub fn get_bool_var(&self, variable: EnvVar) -> Option<bool> {
        self.get_bool(variable.key)
    }

    #[must_use]
    pub fn get_parsed<T>(&self, key: &str) -> Option<T>
    where
        T: std::str::FromStr,
    {
        self.get_raw(key).and_then(|value| value.parse::<T>().ok())
    }

    #[must_use]
    pub fn get_parsed_var<T>(&self, variable: EnvVar) -> Option<T>
    where
        T: std::str::FromStr,
    {
        self.get_parsed(variable.key)
    }

    #[must_use]
    pub fn split_paths_var(&self, variable: EnvVar) -> Vec<PathBuf> {
        self.get_raw_var(variable)
            .map(split_path_list)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.values.iter()
    }

    #[must_use]
    pub fn into_inner(self) -> BTreeMap<String, String> {
        self.values
    }

    pub fn apply_to_process(&self) {
        for (key, value) in &self.values {
            // Callers use this during startup before worker threads are spawned.
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
}

#[must_use]
pub fn get_non_empty(variable: EnvVar) -> Option<String> {
    std::env::var(variable.key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[must_use]
pub fn describe(variable: EnvVar) -> (&'static str, &'static str) {
    (variable.key, variable.description)
}

#[must_use]
pub fn log_filter_or_default(default: &str) -> String {
    get_non_empty(vars::RUST_LOG).unwrap_or_else(|| default.to_string())
}

#[must_use]
pub fn has_non_empty(variable: EnvVar) -> bool {
    get_non_empty(variable).is_some()
}

#[must_use]
pub fn read_bool_flag(variable: EnvVar) -> bool {
    get_non_empty(variable)
        .as_deref()
        .and_then(parse_bool_value)
        .unwrap_or(false)
}

#[must_use]
pub fn split_path_list(value: &str) -> Vec<PathBuf> {
    std::env::split_paths(value).collect()
}

#[must_use]
pub fn shell_or_default(default_shell: &str) -> String {
    get_non_empty(vars::SHELL).unwrap_or_else(|| default_shell.to_string())
}

#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os(vars::HOME.key).map(PathBuf::from)
}

#[must_use]
pub fn parse_bool_value(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn load_dotenv_file(path: PathBuf, target: &mut BTreeMap<String, String>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in dotenvy::from_path_iter(&path).map_err(|source| EnvError::DotenvLoad {
        path: path.display().to_string(),
        source,
    })? {
        let (key, value) = entry.map_err(|source| EnvError::DotenvLoad {
            path: path.display().to_string(),
            source,
        })?;
        target.insert(key, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EnvMap, parse_bool_value};
    use tempfile::tempdir;

    #[test]
    fn parse_bool_variants() {
        assert_eq!(parse_bool_value("true"), Some(true));
        assert_eq!(parse_bool_value(" YES "), Some(true));
        assert_eq!(parse_bool_value("0"), Some(false));
        assert_eq!(parse_bool_value("off"), Some(false));
        assert_eq!(parse_bool_value("maybe"), None);
    }

    #[test]
    fn workspace_env_overlay_keeps_local_file_precedence() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "AGENT_ENV_UNIT_TEST_OVERLAY=from_env\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".env.local"),
            "AGENT_ENV_UNIT_TEST_OVERLAY=from_local\n",
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        assert_eq!(
            env_map.get_non_empty("AGENT_ENV_UNIT_TEST_OVERLAY"),
            Some("from_local".to_string())
        );
    }
}
