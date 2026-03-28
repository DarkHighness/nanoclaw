use agent::AgentWorkspaceLayout;
use agent::mcp::McpServerConfig;
use agent::plugins::{PluginEntryConfig, PluginSlotsConfig};
use agent_env::{EnvMap, vars};
use anyhow::{Result, anyhow, bail, ensure};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const CORE_HOOK_ENV_PREFIX: &str = "NANOCLAW_CORE_HOOK_ENV_";
const DEFAULT_LANE_ALIAS: &str = "gpt_5_4_default";
const DEFAULT_LANE_MODEL: &str = "gpt-5.4";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSandboxMode {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapabilitiesConfig {
    pub tool_calls: bool,
    pub vision: bool,
    pub image_generation: bool,
    pub audio_input: bool,
    pub tts: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: ProviderKind,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub context_window_tokens: usize,
    pub max_output_tokens: u64,
    pub compact_trigger_tokens: usize,
    #[serde(default)]
    pub compact_preserve_recent_messages: Option<usize>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub additional_params: Option<Value>,
    #[serde(default)]
    pub capabilities: ModelCapabilitiesConfig,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentProfileConfig {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u64>,
    pub context_window_tokens: Option<usize>,
    pub compact_trigger_tokens: Option<usize>,
    pub compact_preserve_recent_messages: Option<usize>,
    pub additional_params: Option<Value>,
    pub auto_compact: Option<bool>,
    pub sandbox: Option<AgentSandboxMode>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InternalProfileConfig {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u64>,
    pub additional_params: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsConfig {
    pub primary: AgentProfileConfig,
    pub subagent_defaults: AgentProfileConfig,
    pub roles: BTreeMap<String, AgentProfileConfig>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            primary: AgentProfileConfig {
                model: Some(DEFAULT_LANE_ALIAS.to_string()),
                sandbox: Some(AgentSandboxMode::WorkspaceWrite),
                auto_compact: Some(true),
                ..AgentProfileConfig::default()
            },
            subagent_defaults: AgentProfileConfig {
                model: Some(DEFAULT_LANE_ALIAS.to_string()),
                sandbox: Some(AgentSandboxMode::ReadOnly),
                reasoning_effort: Some("medium".to_string()),
                auto_compact: Some(true),
                ..AgentProfileConfig::default()
            },
            roles: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InternalProfilesConfig {
    pub summary: InternalProfileConfig,
    pub memory: InternalProfileConfig,
}

impl Default for InternalProfilesConfig {
    fn default() -> Self {
        Self {
            summary: InternalProfileConfig {
                model: Some(DEFAULT_LANE_ALIAS.to_string()),
                reasoning_effort: Some("low".to_string()),
                max_output_tokens: Some(32_000),
                ..InternalProfileConfig::default()
            },
            memory: InternalProfileConfig {
                model: Some(DEFAULT_LANE_ALIAS.to_string()),
                reasoning_effort: Some("medium".to_string()),
                max_output_tokens: Some(32_000),
                ..InternalProfileConfig::default()
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostRuntimeConfig {
    pub workspace_only: bool,
    pub sandbox_fail_if_unavailable: bool,
    #[serde(default)]
    pub store_dir: Option<String>,
    #[serde(default)]
    pub tokio_worker_threads: Option<usize>,
    #[serde(default)]
    pub tokio_max_blocking_threads: Option<usize>,
}

impl Default for HostRuntimeConfig {
    fn default() -> Self {
        Self {
            workspace_only: true,
            sandbox_fail_if_unavailable: false,
            store_dir: None,
            tokio_worker_threads: None,
            tokio_max_blocking_threads: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedModel {
    pub alias: String,
    pub provider: ProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub context_window_tokens: usize,
    pub max_output_tokens: u64,
    pub compact_trigger_tokens: usize,
    pub compact_preserve_recent_messages: usize,
    pub temperature: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub additional_params: Option<Value>,
    pub capabilities: ModelCapabilitiesConfig,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAgentProfile {
    pub profile_name: String,
    pub model: ResolvedModel,
    pub global_system_prompt: Option<String>,
    pub system_prompt: Option<String>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub max_output_tokens: u64,
    pub context_window_tokens: usize,
    pub compact_trigger_tokens: usize,
    pub compact_preserve_recent_messages: usize,
    pub additional_params: Option<Value>,
    pub auto_compact: bool,
    pub sandbox: AgentSandboxMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedInternalProfile {
    pub profile_name: String,
    pub model: ResolvedModel,
    pub global_system_prompt: Option<String>,
    pub system_prompt: Option<String>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub max_output_tokens: u64,
    pub additional_params: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NanoclawCoreConfig {
    #[serde(default)]
    pub global_system_prompt: Option<String>,
    #[serde(default)]
    pub host: HostRuntimeConfig,
    #[serde(default = "default_models")]
    pub models: BTreeMap<String, ModelConfig>,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub internal: InternalProfilesConfig,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub hook_env: BTreeMap<String, String>,
    #[serde(default)]
    pub skill_roots: Vec<String>,
    #[serde(default)]
    pub plugins: PluginsConfig,
}

impl Default for NanoclawCoreConfig {
    fn default() -> Self {
        Self {
            global_system_prompt: None,
            host: HostRuntimeConfig::default(),
            models: default_models(),
            agents: AgentsConfig::default(),
            internal: InternalProfilesConfig::default(),
            mcp_servers: Vec::new(),
            hook_env: BTreeMap::new(),
            skill_roots: Vec::new(),
            plugins: PluginsConfig::default(),
        }
    }
}

pub type CoreConfig = NanoclawCoreConfig;

impl NanoclawCoreConfig {
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let mut config = load_config_file(dir)?;
        let env_map = EnvMap::from_workspace_dir(dir)?;

        if let Some(parsed) = env_map.get_bool_var(vars::NANOCLAW_CORE_WORKSPACE_ONLY) {
            config.host.workspace_only = parsed;
        }
        if let Some(value) = env_map.get_non_empty_var(vars::NANOCLAW_CORE_STORE_DIR) {
            config.host.store_dir = Some(value);
        }
        if let Some(parsed) = env_map.get_bool_var(vars::NANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE)
        {
            config.host.sandbox_fail_if_unavailable = parsed;
        }
        if let Some(parsed) =
            env_map.get_parsed_var::<usize>(vars::NANOCLAW_CORE_TOKIO_WORKER_THREADS)
        {
            config.host.tokio_worker_threads = Some(parsed);
        }
        if let Some(parsed) =
            env_map.get_parsed_var::<usize>(vars::NANOCLAW_CORE_TOKIO_MAX_BLOCKING_THREADS)
        {
            config.host.tokio_max_blocking_threads = Some(parsed);
        }
        if let Some(value) = env_map.get_raw_var(vars::NANOCLAW_CORE_SKILL_ROOTS) {
            config.skill_roots = split_env_paths(value);
        }
        if let Some(value) = env_map.get_raw_var(vars::NANOCLAW_CORE_PLUGIN_ROOTS) {
            config.plugins.roots = split_env_paths(value);
        }
        if let Some(value) = env_map.get_non_empty_var(vars::NANOCLAW_CORE_PLUGIN_MEMORY_SLOT) {
            config.plugins.slots.memory = Some(value);
        }
        for (key, value) in env_map.iter() {
            if key.starts_with(CORE_HOOK_ENV_PREFIX) {
                config.hook_env.insert(
                    key.trim_start_matches(CORE_HOOK_ENV_PREFIX).to_string(),
                    value.clone(),
                );
            }
        }

        dedup_paths(&mut config.skill_roots);
        dedup_paths(&mut config.plugins.roots);
        config.validate()?;
        Ok(config)
    }

    pub fn with_override(mut self, update: impl FnOnce(&mut Self)) -> Self {
        update(&mut self);
        self
    }

    #[must_use]
    pub fn config_path(dir: impl AsRef<Path>) -> PathBuf {
        AgentWorkspaceLayout::new(dir).core_config_path()
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
        self.host
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

    pub fn resolve_model(&self, alias: &str) -> Result<ResolvedModel> {
        self.validate()?;
        let model = self
            .models
            .get(alias)
            .ok_or_else(|| anyhow!("unknown model alias `{alias}`"))?;
        validate_model(alias, model)?;
        Ok(ResolvedModel {
            alias: alias.to_string(),
            provider: model.provider.clone(),
            model: model.model.clone(),
            base_url: model.base_url.clone(),
            context_window_tokens: model.context_window_tokens,
            max_output_tokens: model.max_output_tokens,
            compact_trigger_tokens: model.compact_trigger_tokens,
            compact_preserve_recent_messages: model.compact_preserve_recent_messages.unwrap_or(8),
            temperature: model.temperature,
            reasoning_effort: normalize_optional_string(model.reasoning_effort.clone()),
            additional_params: normalize_object(
                format!("models.{alias}.additional_params"),
                model.additional_params.clone(),
            )?,
            capabilities: model.capabilities.clone(),
        })
    }

    pub fn resolve_primary_agent(&self) -> Result<ResolvedAgentProfile> {
        self.validate()?;
        self.resolve_agent_profile("primary", &self.agents.primary, None)
    }

    pub fn resolve_subagent_profile(&self, role: Option<&str>) -> Result<ResolvedAgentProfile> {
        self.validate()?;
        let role_key = role.map(str::trim).filter(|value| !value.is_empty());
        let overlay = role_key.and_then(|name| self.agents.roles.get(name));
        let profile_name = role_key
            .map(|name| format!("roles.{name}"))
            .unwrap_or_else(|| "subagent_defaults".to_string());
        self.resolve_agent_profile(&profile_name, &self.agents.subagent_defaults, overlay)
    }

    pub fn resolve_summary_profile(&self) -> Result<ResolvedInternalProfile> {
        self.validate()?;
        self.resolve_internal_profile("summary", &self.internal.summary)
    }

    pub fn resolve_memory_profile(&self) -> Result<ResolvedInternalProfile> {
        self.validate()?;
        self.resolve_internal_profile("memory", &self.internal.memory)
    }

    fn resolve_agent_profile(
        &self,
        profile_name: &str,
        base: &AgentProfileConfig,
        overlay: Option<&AgentProfileConfig>,
    ) -> Result<ResolvedAgentProfile> {
        let model_alias = overlay
            .and_then(|profile| profile.model.as_deref())
            .or(base.model.as_deref())
            .ok_or_else(|| anyhow!("agent profile `{profile_name}` is missing `model`"))?;
        let model = self.resolve_model(model_alias)?;
        let sandbox = overlay
            .and_then(|profile| profile.sandbox)
            .or(base.sandbox)
            .ok_or_else(|| anyhow!("agent profile `{profile_name}` is missing `sandbox`"))?;

        let resolved = ResolvedAgentProfile {
            profile_name: profile_name.to_string(),
            model: model.clone(),
            global_system_prompt: normalize_optional_string(self.global_system_prompt.clone()),
            system_prompt: overlay
                .and_then(|profile| profile.system_prompt.clone())
                .or_else(|| base.system_prompt.clone())
                .and_then(|value| normalize_optional_string(Some(value))),
            reasoning_effort: overlay
                .and_then(|profile| profile.reasoning_effort.clone())
                .or_else(|| base.reasoning_effort.clone())
                .map(Some)
                .unwrap_or_else(|| model.reasoning_effort.clone())
                .and_then(|value| normalize_optional_string(Some(value))),
            temperature: overlay
                .and_then(|profile| profile.temperature)
                .or(base.temperature)
                .or(model.temperature),
            max_output_tokens: overlay
                .and_then(|profile| profile.max_output_tokens)
                .or(base.max_output_tokens)
                .unwrap_or(model.max_output_tokens),
            context_window_tokens: overlay
                .and_then(|profile| profile.context_window_tokens)
                .or(base.context_window_tokens)
                .unwrap_or(model.context_window_tokens),
            compact_trigger_tokens: overlay
                .and_then(|profile| profile.compact_trigger_tokens)
                .or(base.compact_trigger_tokens)
                .unwrap_or(model.compact_trigger_tokens),
            compact_preserve_recent_messages: overlay
                .and_then(|profile| profile.compact_preserve_recent_messages)
                .or(base.compact_preserve_recent_messages)
                .unwrap_or(model.compact_preserve_recent_messages),
            additional_params: merge_json_objects(
                model.additional_params.clone(),
                base.additional_params.clone(),
                overlay.and_then(|profile| profile.additional_params.clone()),
                format!("agents.{profile_name}.additional_params"),
            )?,
            auto_compact: overlay
                .and_then(|profile| profile.auto_compact)
                .or(base.auto_compact)
                .unwrap_or(true),
            sandbox,
        };
        validate_resolved_agent_profile(&resolved)?;
        Ok(resolved)
    }

    fn resolve_internal_profile(
        &self,
        profile_name: &str,
        profile: &InternalProfileConfig,
    ) -> Result<ResolvedInternalProfile> {
        let model_alias = profile
            .model
            .as_deref()
            .ok_or_else(|| anyhow!("internal profile `{profile_name}` is missing `model`"))?;
        let model = self.resolve_model(model_alias)?;
        let resolved = ResolvedInternalProfile {
            profile_name: profile_name.to_string(),
            model: model.clone(),
            global_system_prompt: normalize_optional_string(self.global_system_prompt.clone()),
            system_prompt: profile
                .system_prompt
                .clone()
                .and_then(|value| normalize_optional_string(Some(value))),
            reasoning_effort: profile
                .reasoning_effort
                .clone()
                .map(Some)
                .unwrap_or_else(|| model.reasoning_effort.clone())
                .and_then(|value| normalize_optional_string(Some(value))),
            temperature: profile.temperature.or(model.temperature),
            max_output_tokens: profile.max_output_tokens.unwrap_or(model.max_output_tokens),
            additional_params: merge_json_objects(
                model.additional_params.clone(),
                profile.additional_params.clone(),
                None,
                format!("internal.{profile_name}.additional_params"),
            )?,
        };
        ensure!(
            resolved.max_output_tokens > 0
                && resolved.max_output_tokens as usize <= resolved.model.context_window_tokens,
            "internal profile `{profile_name}` resolved invalid max_output_tokens"
        );
        Ok(resolved)
    }

    fn validate(&self) -> Result<()> {
        for (alias, model) in &self.models {
            validate_model_alias(alias)?;
            validate_model(alias, model)?;
        }
        let default_lane = self.models.get(DEFAULT_LANE_ALIAS).ok_or_else(|| {
            anyhow!("missing required default lane `models.{DEFAULT_LANE_ALIAS}`")
        })?;
        ensure!(
            default_lane.model == DEFAULT_LANE_MODEL,
            "default lane `models.{DEFAULT_LANE_ALIAS}` must point to `{DEFAULT_LANE_MODEL}`"
        );

        ensure!(
            self.agents.primary.model.is_some(),
            "missing `agents.primary.model`"
        );
        ensure!(
            self.agents.primary.sandbox.is_some(),
            "missing `agents.primary.sandbox`"
        );
        ensure!(
            self.agents.subagent_defaults.model.is_some(),
            "missing `agents.subagent_defaults.model`"
        );
        ensure!(
            self.agents.subagent_defaults.sandbox.is_some(),
            "missing `agents.subagent_defaults.sandbox`"
        );
        ensure!(
            self.internal.summary.model.is_some(),
            "missing `internal.summary.model`"
        );
        ensure!(
            self.internal.memory.model.is_some(),
            "missing `internal.memory.model`"
        );

        for (role, profile) in &self.agents.roles {
            ensure!(
                !role.trim().is_empty(),
                "agent role names must be non-empty"
            );
            if let Some(alias) = profile.model.as_deref() {
                ensure!(
                    self.models.contains_key(alias),
                    "agent role `{role}` references unknown model alias `{alias}`"
                );
            }
            validate_profile_additional_params(
                format!("agents.roles.{role}.additional_params"),
                profile.additional_params.clone(),
            )?;
        }

        validate_profile_additional_params(
            "agents.primary.additional_params".to_string(),
            self.agents.primary.additional_params.clone(),
        )?;
        validate_profile_additional_params(
            "agents.subagent_defaults.additional_params".to_string(),
            self.agents.subagent_defaults.additional_params.clone(),
        )?;
        validate_profile_additional_params(
            "internal.summary.additional_params".to_string(),
            self.internal.summary.additional_params.clone(),
        )?;
        validate_profile_additional_params(
            "internal.memory.additional_params".to_string(),
            self.internal.memory.additional_params.clone(),
        )?;

        Ok(())
    }
}

#[must_use]
pub fn core_config_path(dir: impl AsRef<Path>) -> PathBuf {
    CoreConfig::config_path(dir)
}

#[must_use]
pub fn app_config_path(dir: impl AsRef<Path>, app_name: &str) -> PathBuf {
    AgentWorkspaceLayout::new(dir).app_config_path(app_name)
}

pub fn load_optional_app_config<T>(dir: impl AsRef<Path>, app_name: &str) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    let path = app_config_path(dir, app_name);
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&raw)?)
}

fn default_models() -> BTreeMap<String, ModelConfig> {
    let mut models = BTreeMap::new();
    models.insert(
        DEFAULT_LANE_ALIAS.to_string(),
        ModelConfig {
            provider: ProviderKind::OpenAi,
            model: DEFAULT_LANE_MODEL.to_string(),
            base_url: None,
            context_window_tokens: 400_000,
            max_output_tokens: 128_000,
            compact_trigger_tokens: 320_000,
            compact_preserve_recent_messages: Some(8),
            temperature: Some(0.2),
            reasoning_effort: Some("medium".to_string()),
            additional_params: None,
            capabilities: ModelCapabilitiesConfig {
                tool_calls: true,
                vision: true,
                image_generation: true,
                audio_input: false,
                tts: true,
            },
        },
    );
    models
}

fn validate_model_alias(alias: &str) -> Result<()> {
    ensure!(!alias.trim().is_empty(), "model aliases must be non-empty");
    ensure!(
        alias
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-'),
        "model alias `{alias}` contains unsupported characters"
    );
    Ok(())
}

fn validate_model(alias: &str, model: &ModelConfig) -> Result<()> {
    ensure!(
        !model.model.trim().is_empty(),
        "models.{alias}.model must be non-empty"
    );
    ensure!(
        model.context_window_tokens > 0,
        "models.{alias}.context_window_tokens must be > 0"
    );
    ensure!(
        model.max_output_tokens > 0,
        "models.{alias}.max_output_tokens must be > 0"
    );
    ensure!(
        model.compact_trigger_tokens > 0,
        "models.{alias}.compact_trigger_tokens must be > 0"
    );
    ensure!(
        model.compact_trigger_tokens < model.context_window_tokens,
        "models.{alias}.compact_trigger_tokens must be < context_window_tokens"
    );
    ensure!(
        model.max_output_tokens as usize <= model.context_window_tokens,
        "models.{alias}.max_output_tokens must be <= context_window_tokens"
    );
    if let Some(preserve) = model.compact_preserve_recent_messages {
        ensure!(
            preserve > 0,
            "models.{alias}.compact_preserve_recent_messages must be > 0"
        );
    }
    normalize_object(
        format!("models.{alias}.additional_params"),
        model.additional_params.clone(),
    )?;
    Ok(())
}

fn validate_resolved_agent_profile(profile: &ResolvedAgentProfile) -> Result<()> {
    ensure!(
        profile.context_window_tokens > 0,
        "agent profile `{}` resolved invalid context window",
        profile.profile_name
    );
    ensure!(
        profile.max_output_tokens > 0
            && profile.max_output_tokens as usize <= profile.context_window_tokens,
        "agent profile `{}` resolved invalid max_output_tokens",
        profile.profile_name
    );
    ensure!(
        profile.compact_trigger_tokens > 0
            && profile.compact_trigger_tokens < profile.context_window_tokens,
        "agent profile `{}` resolved invalid compact_trigger_tokens",
        profile.profile_name
    );
    ensure!(
        profile.compact_preserve_recent_messages > 0,
        "agent profile `{}` resolved invalid compact_preserve_recent_messages",
        profile.profile_name
    );
    Ok(())
}

fn validate_profile_additional_params(label: String, value: Option<Value>) -> Result<()> {
    normalize_object(label, value)?;
    Ok(())
}

fn normalize_object(label: String, value: Option<Value>) -> Result<Option<Value>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_object() {
        bail!("{label} must be a JSON object when provided");
    }
    Ok(Some(value))
}

fn merge_json_objects(
    base: Option<Value>,
    middle: Option<Value>,
    overlay: Option<Value>,
    label: String,
) -> Result<Option<Value>> {
    let mut merged = match normalize_object(label.clone(), base)? {
        Some(Value::Object(map)) => map,
        Some(_) => unreachable!(),
        None => Map::new(),
    };
    for layer in [middle, overlay] {
        let Some(value) = normalize_object(label.clone(), layer)? else {
            continue;
        };
        let Value::Object(map) = value else {
            unreachable!();
        };
        for (key, value) in map {
            merged.insert(key, value);
        }
    }
    if merged.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(merged)))
    }
}

fn load_config_file(dir: &Path) -> Result<NanoclawCoreConfig> {
    let path = NanoclawCoreConfig::config_path(dir);
    if !path.exists() {
        return Ok(NanoclawCoreConfig::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&raw)?)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|entry| {
        let trimmed = entry.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn split_env_paths(value: &str) -> Vec<String> {
    agent_env::split_path_list(value)
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect()
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
    use super::{AgentSandboxMode, NanoclawCoreConfig, ProviderKind};
    use agent::AgentWorkspaceLayout;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn loads_dotenv_precedence_for_host_fields() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(AgentWorkspaceLayout::new(dir.path()).config_dir()).unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "NANOCLAW_CORE_WORKSPACE_ONLY=false\nNANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=true\nNANOCLAW_CORE_TOKIO_MAX_BLOCKING_THREADS=12\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".env.local"),
            "NANOCLAW_CORE_STORE_DIR=.nanoclaw/env-store\nNANOCLAW_CORE_TOKIO_WORKER_THREADS=3\n",
        )
        .unwrap();

        let config = NanoclawCoreConfig::load_from_dir(dir.path()).unwrap();
        assert!(!config.host.workspace_only);
        assert!(config.host.sandbox_fail_if_unavailable);
        assert_eq!(
            config.host.store_dir.as_deref(),
            Some(".nanoclaw/env-store")
        );
        assert_eq!(config.host.tokio_worker_threads, Some(3));
        assert_eq!(config.host.tokio_max_blocking_threads, Some(12));
    }

    #[tokio::test]
    async fn loads_new_toml_schema_and_resolves_profiles() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(dir.path());
        layout.ensure_standard_layout().unwrap();
        std::fs::write(
            layout.core_config_path(),
            r#"
                global_system_prompt = "Work carefully and be concise."
                skill_roots = ["skills", "/tmp/global-skills"]

                [host]
                workspace_only = false
                store_dir = ".nanoclaw/custom-store"
                sandbox_fail_if_unavailable = true
                tokio_worker_threads = 2
                tokio_max_blocking_threads = 10

                [models.gpt_5_4_default]
                provider = "openai"
                model = "gpt-5.4"
                context_window_tokens = 400000
                max_output_tokens = 128000
                compact_trigger_tokens = 300000
                compact_preserve_recent_messages = 6
                temperature = 0.2
                additional_params = { metadata = { tier = "standard" } }

                [models.fast_review]
                provider = "anthropic"
                model = "claude-sonnet-4-6"
                context_window_tokens = 200000
                max_output_tokens = 32000
                compact_trigger_tokens = 160000
                reasoning_effort = "low"

                [agents.primary]
                model = "fast_review"
                sandbox = "workspace_write"
                system_prompt = "Primary prompt."
                auto_compact = true

                [agents.subagent_defaults]
                model = "gpt_5_4_default"
                sandbox = "read_only"
                auto_compact = false

                [agents.roles.reviewer]
                model = "fast_review"
                reasoning_effort = "low"

                [internal.summary]
                model = "fast_review"
                max_output_tokens = 16000

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
        .unwrap();

        let config = NanoclawCoreConfig::load_from_dir(dir.path()).unwrap();
        let primary = config.resolve_primary_agent().unwrap();
        let reviewer = config.resolve_subagent_profile(Some("reviewer")).unwrap();
        let summary = config.resolve_summary_profile().unwrap();

        assert_eq!(primary.model.provider, ProviderKind::Anthropic);
        assert_eq!(primary.model.model, "claude-sonnet-4-6");
        assert_eq!(primary.system_prompt.as_deref(), Some("Primary prompt."));
        assert_eq!(
            primary.global_system_prompt.as_deref(),
            Some("Work carefully and be concise.")
        );
        assert_eq!(primary.sandbox, AgentSandboxMode::WorkspaceWrite);
        assert_eq!(primary.max_output_tokens, 32_000);
        assert_eq!(reviewer.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(summary.max_output_tokens, 16_000);
        assert_eq!(
            config.host.store_dir.as_deref(),
            Some(".nanoclaw/custom-store")
        );

        let skill_roots = config.resolved_skill_roots(dir.path());
        assert_eq!(skill_roots[0], dir.path().join("skills"));
        assert_eq!(skill_roots[1], PathBuf::from("/tmp/global-skills"));
        let plugin_roots = config.resolved_plugin_roots(dir.path());
        assert_eq!(plugin_roots[0], dir.path().join("plugins"));
        assert_eq!(plugin_roots[1], PathBuf::from("/tmp/global-plugins"));
        assert_eq!(
            config.resolved_store_dir(dir.path()),
            dir.path().join(".nanoclaw/custom-store")
        );
    }

    #[tokio::test]
    async fn rejects_legacy_single_model_schema() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(dir.path());
        layout.ensure_standard_layout().unwrap();
        std::fs::write(
            layout.core_config_path(),
            r#"
                system_prompt = "legacy"

                [provider]
                kind = "openai"
                model = "gpt-4.1-mini"
            "#,
        )
        .unwrap();

        let error = NanoclawCoreConfig::load_from_dir(dir.path()).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}
