use agent::inference::LlmServiceConfig;
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent::runtime::{ModelBackend, ModelBackendCapabilities, Result as RuntimeResult};
use agent::types::{ModelEvent, ModelRequest};
use agent_env::{EnvMap, vars};
use anyhow::{Result, bail};
use async_trait::async_trait;
use futures::stream::BoxStream;
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile, ResolvedModel};
use serde_json::{Map, Value};
use std::sync::{Arc, RwLock};

const DEFAULT_INTERNAL_MEMORY_TIMEOUT_MS: u64 = 30_000;
const OPENAI_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];
const OPENAI_EXTENDED_REASONING_LEVELS: &[&str] = &["none", "low", "medium", "high"];
const OPENAI_HIGH_ONLY_REASONING_LEVELS: &[&str] = &["high"];
const ANTHROPIC_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];
const ANTHROPIC_MAX_REASONING_LEVELS: &[&str] = &["low", "medium", "high", "max"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReasoningEffortUpdate {
    pub(crate) previous: Option<String>,
    pub(crate) current: Option<String>,
    pub(crate) supported: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct MutableAgentBackend {
    state: Arc<RwLock<MutableAgentBackendState>>,
}

struct MutableAgentBackendState {
    config: MutableAgentBackendConfig,
    backend: Arc<ProviderBackend>,
}

#[derive(Clone)]
struct MutableAgentBackendConfig {
    model: ResolvedModel,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<Value>,
    compact_trigger_tokens: Option<usize>,
    api_key: Option<String>,
    reasoning_effort: Option<String>,
}

impl MutableAgentBackendConfig {
    fn from_profile(profile: &ResolvedAgentProfile, env_map: &EnvMap) -> Self {
        Self {
            model: profile.model.clone(),
            temperature: profile.temperature,
            max_tokens: Some(profile.max_output_tokens),
            additional_params: profile.additional_params.clone(),
            compact_trigger_tokens: matches!(profile.model.provider, ProviderKind::OpenAi)
                .then_some(profile.compact_trigger_tokens),
            api_key: provider_api_key(&profile.model, env_map),
            reasoning_effort: normalized_reasoning_effort(profile.reasoning_effort.as_deref()),
        }
    }

    fn build_backend(&self) -> Result<ProviderBackend> {
        build_backend_from_model(
            &self.model,
            self.temperature,
            self.max_tokens,
            self.additional_params.clone(),
            self.compact_trigger_tokens,
            self.reasoning_effort.clone(),
            self.api_key.clone(),
        )
    }
}

impl MutableAgentBackend {
    pub(crate) fn from_profile(profile: &ResolvedAgentProfile, env_map: &EnvMap) -> Result<Self> {
        let config = MutableAgentBackendConfig::from_profile(profile, env_map);
        let backend = Arc::new(config.build_backend()?);
        Ok(Self {
            state: Arc::new(RwLock::new(MutableAgentBackendState { config, backend })),
        })
    }

    pub(crate) fn supported_reasoning_efforts(&self) -> Vec<String> {
        let state = self.state.read().unwrap();
        supported_reasoning_efforts(&state.config.model.provider, &state.config.model.model)
            .iter()
            .map(|level| (*level).to_string())
            .collect()
    }

    pub(crate) fn reasoning_effort(&self) -> Option<String> {
        self.state.read().unwrap().config.reasoning_effort.clone()
    }

    pub(crate) fn cycle_reasoning_effort(&self) -> Result<ReasoningEffortUpdate> {
        let supported = self.supported_reasoning_efforts();
        let Some(next) = next_reasoning_effort(self.reasoning_effort().as_deref(), &supported)
        else {
            let model = self.state.read().unwrap().config.model.model.clone();
            bail!("thinking effort controls are unavailable for `{model}`");
        };
        self.set_reasoning_effort(&next)
    }

    pub(crate) fn set_reasoning_effort(&self, effort: &str) -> Result<ReasoningEffortUpdate> {
        let supported = self.supported_reasoning_efforts();
        let normalized = normalized_reasoning_effort(Some(effort))
            .ok_or_else(|| anyhow::anyhow!("thinking effort must be non-empty"))?;
        if !supported.iter().any(|level| level == &normalized) {
            let model = self.state.read().unwrap().config.model.model.clone();
            let supported_list = if supported.is_empty() {
                "none".to_string()
            } else {
                supported.join(", ")
            };
            bail!(
                "thinking effort `{normalized}` is not supported for `{model}` (supported: {supported_list})"
            );
        }

        let mut state = self.state.write().unwrap();
        let previous = state.config.reasoning_effort.clone();
        if previous.as_deref() == Some(normalized.as_str()) {
            return Ok(ReasoningEffortUpdate {
                previous,
                current: Some(normalized),
                supported,
            });
        }

        state.config.reasoning_effort = Some(normalized.clone());
        state.backend = Arc::new(state.config.build_backend()?);
        Ok(ReasoningEffortUpdate {
            previous,
            current: Some(normalized),
            supported,
        })
    }
}

#[async_trait]
impl ModelBackend for MutableAgentBackend {
    fn capabilities(&self) -> ModelBackendCapabilities {
        self.state.read().unwrap().backend.capabilities()
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
        let backend = self.state.read().unwrap().backend.clone();
        backend.stream_turn(request).await
    }
}

pub(crate) fn build_agent_backend(
    profile: &ResolvedAgentProfile,
    env_map: &EnvMap,
) -> Result<ProviderBackend> {
    build_backend_from_model(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        matches!(profile.model.provider, ProviderKind::OpenAi)
            .then_some(profile.compact_trigger_tokens),
        profile.reasoning_effort.clone(),
        provider_api_key(&profile.model, env_map),
    )
}

pub(crate) fn build_mutable_agent_backend(
    profile: &ResolvedAgentProfile,
    env_map: &EnvMap,
) -> Result<MutableAgentBackend> {
    MutableAgentBackend::from_profile(profile, env_map)
}

pub(crate) fn build_internal_backend(
    profile: &ResolvedInternalProfile,
    env_map: &EnvMap,
) -> Result<ProviderBackend> {
    build_backend_from_model(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        None,
        profile.reasoning_effort.clone(),
        provider_api_key(&profile.model, env_map),
    )
}

pub(crate) fn build_memory_reasoning_service(
    profile: &ResolvedInternalProfile,
    env_map: &EnvMap,
) -> LlmServiceConfig {
    LlmServiceConfig {
        provider: provider_name(&profile.model.provider).to_string(),
        model: profile.model.model.clone(),
        base_url: profile.model.base_url.clone(),
        api_key: provider_api_key(&profile.model, env_map),
        headers: Default::default(),
        timeout_ms: DEFAULT_INTERNAL_MEMORY_TIMEOUT_MS,
    }
}

pub(crate) fn agent_backend_capabilities(
    profile: &ResolvedAgentProfile,
) -> ModelBackendCapabilities {
    build_backend_settings(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        matches!(profile.model.provider, ProviderKind::OpenAi)
            .then_some(profile.compact_trigger_tokens),
        profile.reasoning_effort.clone(),
    )
    .0
    .capabilities
}

pub(crate) fn provider_label(profile: &ResolvedAgentProfile) -> String {
    let provider = provider_name(&profile.model.provider);
    format!("{} -> {provider}", profile.model.alias)
}

fn provider_name(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    }
}

pub(crate) fn ensure_api_key_available(model: &ResolvedModel, env_map: &EnvMap) -> Result<()> {
    let has_openai = env_map.get_non_empty(vars::OPENAI_API_KEY.key).is_some();
    let has_anthropic = env_map.get_non_empty(vars::ANTHROPIC_API_KEY.key).is_some();
    match model.provider {
        ProviderKind::OpenAi if !has_openai => {
            bail!("missing OPENAI_API_KEY for provider openai")
        }
        ProviderKind::Anthropic if !has_anthropic => {
            bail!("missing ANTHROPIC_API_KEY for provider anthropic")
        }
        _ => Ok(()),
    }
}

fn build_backend_from_model(
    model: &ResolvedModel,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<Value>,
    compact_trigger_tokens: Option<usize>,
    reasoning_effort: Option<String>,
    api_key: Option<String>,
) -> Result<ProviderBackend> {
    let (descriptor, request_options) = build_backend_settings(
        model,
        temperature,
        max_tokens,
        additional_params,
        compact_trigger_tokens,
        reasoning_effort,
    );
    Ok(ProviderBackend::from_settings_with_api_key(
        descriptor,
        request_options,
        model.base_url.clone(),
        api_key,
    )?)
}

fn build_backend_settings(
    model: &ResolvedModel,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<Value>,
    compact_trigger_tokens: Option<usize>,
    reasoning_effort: Option<String>,
) -> (BackendDescriptor, RequestOptions) {
    let request_options = RequestOptions {
        temperature,
        max_tokens,
        additional_params: with_reasoning_effort(
            &model.provider,
            additional_params,
            reasoning_effort,
        ),
        openai_responses: matches!(model.provider, ProviderKind::OpenAi).then(|| {
            OpenAiResponsesOptions {
                chain_previous_response: true,
                store: Some(true),
                server_compaction: compact_trigger_tokens
                    .map(|compact_threshold| OpenAiServerCompaction { compact_threshold }),
            }
        }),
        ..RequestOptions::default()
    };
    let descriptor = BackendDescriptor::new(match model.provider {
        ProviderKind::OpenAi => ProviderDescriptor::openai(model.model.clone()),
        ProviderKind::Anthropic => ProviderDescriptor::anthropic(model.model.clone()),
    })
    .with_capabilities(ModelBackendCapabilities::from_model_surface(
        model.capabilities.tool_calls,
        model.capabilities.vision,
        model.capabilities.image_generation,
        model.capabilities.audio_input,
        model.capabilities.tts,
    ))
    .resolved_for_request(&request_options);

    (descriptor, request_options)
}

fn with_reasoning_effort(
    provider: &ProviderKind,
    additional_params: Option<Value>,
    reasoning_effort: Option<String>,
) -> Option<Value> {
    let Some(effort) = normalized_reasoning_effort(reasoning_effort.as_deref()) else {
        return additional_params;
    };

    let mut object = match additional_params {
        Some(Value::Object(object)) => object,
        Some(other) => return Some(other),
        None => Map::new(),
    };

    // Provider-specific reasoning controls live in different top-level request
    // objects. Centralizing the mapping here keeps the host-facing "thinking
    // effort" control stable while the actual payload shape stays provider-native.
    match provider {
        ProviderKind::OpenAi => {
            nested_object(&mut object, "reasoning")
                .insert("effort".to_string(), Value::String(effort));
        }
        ProviderKind::Anthropic => {
            nested_object(&mut object, "output_config")
                .insert("effort".to_string(), Value::String(effort));
        }
    }

    Some(Value::Object(object))
}

fn nested_object<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !matches!(object.get(key), Some(Value::Object(_))) {
        object.insert(key.to_string(), Value::Object(Map::new()));
    }
    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("nested reasoning config must be an object")
}

fn supported_reasoning_efforts(provider: &ProviderKind, model: &str) -> &'static [&'static str] {
    // The TUI cycles only through the levels we can describe consistently
    // across providers today. Model-specific extremes (for example OpenAI's
    // newer `xhigh` or provider-only budget knobs) can still be added later
    // once the host grows a richer picker than a single cycle shortcut.
    match provider {
        ProviderKind::OpenAi if model.starts_with("gpt-5-pro") => OPENAI_HIGH_ONLY_REASONING_LEVELS,
        ProviderKind::OpenAi if model.starts_with("gpt-5.1") || model.starts_with("gpt-5.2") => {
            OPENAI_EXTENDED_REASONING_LEVELS
        }
        ProviderKind::OpenAi if model.starts_with("gpt-5") => OPENAI_REASONING_LEVELS,
        ProviderKind::Anthropic if model.starts_with("claude-opus-4-6") => {
            ANTHROPIC_MAX_REASONING_LEVELS
        }
        ProviderKind::Anthropic
            if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-opus-4-5") =>
        {
            ANTHROPIC_REASONING_LEVELS
        }
        _ => &[],
    }
}

fn normalized_reasoning_effort(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn next_reasoning_effort(current: Option<&str>, supported: &[String]) -> Option<String> {
    if supported.is_empty() {
        return None;
    }
    let current = normalized_reasoning_effort(current);
    let current_index = current
        .as_ref()
        .and_then(|value| supported.iter().position(|candidate| candidate == value));
    Some(match current_index {
        Some(index) => supported[(index + 1) % supported.len()].clone(),
        None => supported[0].clone(),
    })
}

fn provider_api_key(model: &ResolvedModel, env_map: &EnvMap) -> Option<String> {
    let env_key = match model.provider {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    env_map.get_non_empty(env_key)
}

#[cfg(test)]
mod tests {
    use super::{MutableAgentBackend, build_memory_reasoning_service, with_reasoning_effort};
    use agent_env::EnvMap;
    use nanoclaw_config::{
        AgentSandboxMode, ModelCapabilitiesConfig, ProviderKind, ResolvedAgentProfile,
        ResolvedInternalProfile, ResolvedModel,
    };
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn memory_reasoning_service_falls_back_to_workspace_env_key() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=env-memory-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let profile = ResolvedInternalProfile {
            profile_name: "memory".to_string(),
            model: ResolvedModel {
                alias: "memory-lane".to_string(),
                provider: ProviderKind::OpenAi,
                model: "gpt-5.4-mini".to_string(),
                base_url: None,
                context_window_tokens: 400_000,
                max_output_tokens: 32_000,
                compact_trigger_tokens: 320_000,
                compact_preserve_recent_messages: 8,
                temperature: None,
                reasoning_effort: Some("medium".to_string()),
                additional_params: None,
                capabilities: ModelCapabilitiesConfig::default(),
            },
            global_system_prompt: None,
            system_prompt: None,
            reasoning_effort: Some("medium".to_string()),
            temperature: None,
            max_output_tokens: 32_000,
            additional_params: None,
        };

        let service = build_memory_reasoning_service(&profile, &env_map);

        assert_eq!(service.api_key.as_deref(), Some("env-memory-key"));
    }

    #[test]
    fn openai_reasoning_effort_is_merged_into_additional_params() {
        let params = with_reasoning_effort(
            &ProviderKind::OpenAi,
            Some(json!({ "metadata": { "tier": "standard" } })),
            Some("high".to_string()),
        )
        .unwrap();

        assert_eq!(params["reasoning"]["effort"], "high");
        assert_eq!(params["metadata"]["tier"], "standard");
    }

    #[test]
    fn anthropic_reasoning_effort_uses_output_config() {
        let params = with_reasoning_effort(
            &ProviderKind::Anthropic,
            Some(json!({ "cache_control": { "type": "ephemeral" } })),
            Some("medium".to_string()),
        )
        .unwrap();

        assert_eq!(params["output_config"]["effort"], "medium");
        assert_eq!(params["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn mutable_backend_cycles_reasoning_effort() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let profile = ResolvedAgentProfile {
            profile_name: "primary".to_string(),
            model: ResolvedModel {
                alias: "default".to_string(),
                provider: ProviderKind::OpenAi,
                model: "gpt-5.4".to_string(),
                base_url: Some("https://example.invalid/v1".to_string()),
                context_window_tokens: 400_000,
                max_output_tokens: 32_000,
                compact_trigger_tokens: 320_000,
                compact_preserve_recent_messages: 8,
                temperature: None,
                reasoning_effort: Some("medium".to_string()),
                additional_params: None,
                capabilities: ModelCapabilitiesConfig::default(),
            },
            global_system_prompt: None,
            system_prompt: None,
            reasoning_effort: Some("medium".to_string()),
            temperature: None,
            max_output_tokens: 32_000,
            additional_params: None,
            sandbox: AgentSandboxMode::WorkspaceWrite,
            context_window_tokens: 400_000,
            compact_trigger_tokens: 320_000,
            compact_preserve_recent_messages: 8,
            auto_compact: true,
        };

        let backend = MutableAgentBackend::from_profile(&profile, &env_map).unwrap();
        let update = backend.cycle_reasoning_effort().unwrap();

        assert_eq!(update.previous.as_deref(), Some("medium"));
        assert_eq!(update.current.as_deref(), Some("high"));
        assert_eq!(backend.reasoning_effort().as_deref(), Some("high"));
    }
}
