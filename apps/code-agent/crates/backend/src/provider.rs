use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent::runtime::{ModelBackend, ModelBackendCapabilities, Result as RuntimeResult};
use agent::types::{ModelEvent, ModelRequest, ToolVisibilityContext};
use agent_env::{EnvMap, vars};
use anyhow::{Result, bail};
use async_trait::async_trait;
use futures::stream::BoxStream;
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile, ResolvedModel};
use serde_json::{Map, Value};
use std::sync::{Arc, RwLock};

const OPENAI_GPT5_REASONING_LEVELS: &[&str] = &["minimal", "low", "medium", "high"];
const OPENAI_GPT51_REASONING_LEVELS: &[&str] = &["none", "low", "medium", "high"];
const OPENAI_GPT52_PLUS_REASONING_LEVELS: &[&str] = &["none", "low", "medium", "high", "xhigh"];
const OPENAI_GPT52_PLUS_PRO_REASONING_LEVELS: &[&str] = &["medium", "high", "xhigh"];
const OPENAI_HIGH_ONLY_REASONING_LEVELS: &[&str] = &["high"];
const ANTHROPIC_REASONING_LEVELS: &[&str] = &["low", "medium", "high"];
const ANTHROPIC_MAX_REASONING_LEVELS: &[&str] = &["low", "medium", "high", "max"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasoningEffortUpdate {
    pub previous: Option<String>,
    pub current: Option<String>,
    pub supported: Vec<String>,
}

#[derive(Clone)]
pub struct MutableAgentBackend {
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
    pub fn from_profile(profile: &ResolvedAgentProfile, env_map: &EnvMap) -> Result<Self> {
        let config = MutableAgentBackendConfig::from_profile(profile, env_map);
        let backend = Arc::new(config.build_backend()?);
        Ok(Self {
            state: Arc::new(RwLock::new(MutableAgentBackendState { config, backend })),
        })
    }

    pub fn supported_reasoning_efforts(&self) -> Vec<String> {
        let state = self.state.read().unwrap();
        supported_reasoning_efforts(&state.config.model)
    }

    pub fn reasoning_effort(&self) -> Option<String> {
        self.state.read().unwrap().config.reasoning_effort.clone()
    }

    pub fn cycle_reasoning_effort(&self) -> Result<ReasoningEffortUpdate> {
        let supported = self.supported_reasoning_efforts();
        let Some(next) = next_reasoning_effort(self.reasoning_effort().as_deref(), &supported)
        else {
            let model = self.state.read().unwrap().config.model.model.clone();
            bail!("thinking effort controls are unavailable for `{model}`");
        };
        self.set_reasoning_effort(&next)
    }

    pub fn set_reasoning_effort(&self, effort: &str) -> Result<ReasoningEffortUpdate> {
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
    fn provider_name(&self) -> &'static str {
        let state = self.state.read().unwrap();
        provider_name(&state.config.model.provider)
    }

    fn tool_visibility_context(&self) -> ToolVisibilityContext {
        let state = self.state.read().unwrap();
        ToolVisibilityContext::default()
            .with_provider(provider_name(&state.config.model.provider))
            .with_model(state.config.model.model.clone())
    }

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

pub fn build_agent_backend(
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

pub fn build_mutable_agent_backend(
    profile: &ResolvedAgentProfile,
    env_map: &EnvMap,
) -> Result<MutableAgentBackend> {
    MutableAgentBackend::from_profile(profile, env_map)
}

pub fn build_internal_backend(
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

pub fn agent_backend_capabilities(profile: &ResolvedAgentProfile) -> ModelBackendCapabilities {
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

pub fn provider_label(profile: &ResolvedAgentProfile) -> String {
    let provider = provider_name(&profile.model.provider);
    format!("{} -> {provider}", profile.model.alias)
}

pub fn provider_name(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    }
}

pub fn ensure_api_key_available(model: &ResolvedModel, env_map: &EnvMap) -> Result<()> {
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
                // The host defaults to explicit transcript replay. Only a
                // narrow subset of provider integrations can safely own
                // append-only history semantics across resume, rollback, and
                // visibility surfaces, so provider-managed chaining stays
                // opt-in instead of silently dropping local context.
                chain_previous_response: false,
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

fn supported_reasoning_efforts(model: &ResolvedModel) -> Vec<String> {
    if !model.supported_reasoning_efforts.is_empty() {
        return model.supported_reasoning_efforts.clone();
    }

    // The TUI cycles only through the levels we can describe consistently
    // across providers today. Model-specific extremes (for example OpenAI's
    // newer `xhigh` or provider-only budget knobs) can still be added later
    // once the host grows a richer picker than a single cycle shortcut.
    default_supported_reasoning_efforts(&model.provider, &model.model)
        .iter()
        .map(|level| (*level).to_string())
        .collect()
}

fn default_supported_reasoning_efforts(
    provider: &ProviderKind,
    model: &str,
) -> &'static [&'static str] {
    match provider {
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5-pro") => {
            OPENAI_HIGH_ONLY_REASONING_LEVELS
        }
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5.4-pro") => {
            OPENAI_GPT52_PLUS_PRO_REASONING_LEVELS
        }
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5.2-pro") => {
            OPENAI_GPT52_PLUS_PRO_REASONING_LEVELS
        }
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5.4") => {
            OPENAI_GPT52_PLUS_REASONING_LEVELS
        }
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5.2") => {
            OPENAI_GPT52_PLUS_REASONING_LEVELS
        }
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5.1") => {
            OPENAI_GPT51_REASONING_LEVELS
        }
        ProviderKind::OpenAi if matches_model_family_or_snapshot(model, "gpt-5") => {
            OPENAI_GPT5_REASONING_LEVELS
        }
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

fn matches_model_family_or_snapshot(model: &str, family: &str) -> bool {
    // OpenAI family aliases and dated snapshots share a stable prefix, while
    // sibling variants such as `gpt-5.4-mini` or `gpt-5.4-pro` must not inherit
    // the parent family's picker defaults by accident.
    model == family
        || model
            .strip_prefix(family)
            .is_some_and(|suffix| suffix.starts_with("-20"))
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
    use super::{
        MutableAgentBackend, agent_backend_capabilities, build_backend_settings,
        default_supported_reasoning_efforts, with_reasoning_effort,
    };
    use agent::runtime::ModelBackend;
    use agent_env::EnvMap;
    use nanoclaw_config::{
        AgentSandboxMode, ModelCapabilitiesConfig, ProviderKind, ResolvedAgentProfile,
        ResolvedModel,
    };
    use serde_json::json;
    use tempfile::tempdir;

    fn sample_agent_profile(provider: ProviderKind, model: &str) -> ResolvedAgentProfile {
        ResolvedAgentProfile {
            profile_name: "primary".to_string(),
            model: ResolvedModel {
                alias: "default".to_string(),
                provider,
                model: model.to_string(),
                base_url: Some("https://example.invalid/v1".to_string()),
                context_window_tokens: 400_000,
                max_output_tokens: 32_000,
                compact_trigger_tokens: 320_000,
                compact_preserve_recent_messages: 8,
                temperature: None,
                reasoning_effort: Some("medium".to_string()),
                supported_reasoning_efforts: Vec::new(),
                additional_params: None,
                capabilities: ModelCapabilitiesConfig::default(),
            },
            global_system_prompt: None,
            system_prompt: None,
            reasoning_effort: Some("medium".to_string()),
            allowed_tools: None,
            allowed_mcp_servers: None,
            allowed_skills: None,
            temperature: None,
            max_output_tokens: 32_000,
            additional_params: None,
            sandbox: AgentSandboxMode::WorkspaceWrite,
            context_window_tokens: 400_000,
            compact_trigger_tokens: 320_000,
            compact_preserve_recent_messages: 8,
            auto_compact: true,
        }
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
        let profile = sample_agent_profile(ProviderKind::OpenAi, "gpt-5.4");

        let backend = MutableAgentBackend::from_profile(&profile, &env_map).unwrap();
        let update = backend.cycle_reasoning_effort().unwrap();

        assert_eq!(update.previous.as_deref(), Some("medium"));
        assert_eq!(update.current.as_deref(), Some("high"));
        assert_eq!(backend.reasoning_effort().as_deref(), Some("high"));
    }

    #[test]
    fn mutable_backend_surfaces_provider_and_model_for_tool_visibility() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let profile = sample_agent_profile(ProviderKind::OpenAi, "gpt-5.4");

        let backend = MutableAgentBackend::from_profile(&profile, &env_map).unwrap();
        let visibility = backend.tool_visibility_context();

        assert_eq!(backend.provider_name(), "openai");
        assert_eq!(visibility.provider.as_deref(), Some("openai"));
        assert_eq!(visibility.model.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn host_openai_defaults_to_transcript_replay_over_provider_managed_history() {
        let profile = sample_agent_profile(ProviderKind::OpenAi, "gpt-5.4");

        let capabilities = agent_backend_capabilities(&profile);

        assert!(!capabilities.provider_managed_history);
        assert!(capabilities.provider_native_compaction);
    }

    #[test]
    fn host_openai_request_options_disable_previous_response_chaining_by_default() {
        let profile = sample_agent_profile(ProviderKind::OpenAi, "gpt-5.4");

        let (_, request_options) = build_backend_settings(
            &profile.model,
            profile.temperature,
            Some(profile.max_output_tokens),
            profile.additional_params.clone(),
            Some(profile.compact_trigger_tokens),
            profile.reasoning_effort.clone(),
        );

        let openai_options = request_options
            .openai_responses
            .expect("OpenAI host should still configure Responses options");
        assert!(!openai_options.chain_previous_response);
        assert_eq!(openai_options.store, Some(true));
        assert_eq!(
            openai_options.server_compaction,
            Some(super::OpenAiServerCompaction {
                compact_threshold: profile.compact_trigger_tokens,
            })
        );
    }

    #[test]
    fn openai_reasoning_effort_defaults_follow_model_pages() {
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5"),
            &["minimal", "low", "medium", "high"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.1"),
            &["none", "low", "medium", "high"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.2"),
            &["none", "low", "medium", "high", "xhigh"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.4"),
            &["none", "low", "medium", "high", "xhigh"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5-pro"),
            &["high"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.2-pro"),
            &["medium", "high", "xhigh"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.4-pro"),
            &["medium", "high", "xhigh"]
        );
    }

    #[test]
    fn openai_reasoning_effort_mapping_does_not_confuse_subfamilies_with_snapshots() {
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.4-2026-03-05"),
            &["none", "low", "medium", "high", "xhigh"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.4-pro-2026-03-05"),
            &["medium", "high", "xhigh"]
        );
        assert_eq!(
            default_supported_reasoning_efforts(&ProviderKind::OpenAi, "gpt-5.4-mini"),
            &[] as &[&str]
        );
    }
}
