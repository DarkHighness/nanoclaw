use crate::config::AgentCoreConfig;
use agent::inference::LlmServiceConfig;
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent::runtime::ModelBackendCapabilities;
use agent_env::EnvMap;
use anyhow::Result;
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile, ResolvedModel};
use serde_json::Value;

const DEFAULT_INTERNAL_MEMORY_TIMEOUT_MS: u64 = 30_000;

pub(super) fn build_backend(config: &AgentCoreConfig, env_map: &EnvMap) -> Result<ProviderBackend> {
    build_agent_backend(&config.primary_profile, env_map)
}

pub(super) fn build_summary_backend(
    config: &AgentCoreConfig,
    env_map: &EnvMap,
) -> Result<ProviderBackend> {
    build_internal_backend(&config.summary_profile, env_map)
}

pub(super) fn build_memory_reasoning_service(
    config: &AgentCoreConfig,
    env_map: &EnvMap,
) -> LlmServiceConfig {
    let profile = &config.memory_profile;
    LlmServiceConfig {
        provider: provider_name(&profile.model.provider).to_string(),
        model: profile.model.model.clone(),
        base_url: profile.model.base_url.clone(),
        api_key: provider_api_key(&profile.model, env_map),
        headers: Default::default(),
        timeout_ms: DEFAULT_INTERNAL_MEMORY_TIMEOUT_MS,
    }
}

pub(super) fn agent_backend_capabilities(
    profile: &ResolvedAgentProfile,
) -> ModelBackendCapabilities {
    build_backend_settings(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        matches!(profile.model.provider, ProviderKind::OpenAi)
            .then_some(profile.compact_trigger_tokens),
    )
    .0
    .capabilities
}

pub(super) fn provider_summary(config: &AgentCoreConfig) -> String {
    provider_model_summary(&config.primary_profile.model)
}

pub(super) fn provider_model_summary(model: &ResolvedModel) -> String {
    let provider = provider_name(&model.provider);
    format!("{provider} / {}", model.model)
}

fn build_agent_backend(
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
        provider_api_key(&profile.model, env_map),
    )
}

fn build_internal_backend(
    profile: &ResolvedInternalProfile,
    env_map: &EnvMap,
) -> Result<ProviderBackend> {
    build_backend_from_model(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        None,
        provider_api_key(&profile.model, env_map),
    )
}

fn build_backend_from_model(
    model: &ResolvedModel,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<Value>,
    compact_trigger_tokens: Option<usize>,
    api_key: Option<String>,
) -> Result<ProviderBackend> {
    let (descriptor, request_options) = build_backend_settings(
        model,
        temperature,
        max_tokens,
        additional_params,
        compact_trigger_tokens,
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
) -> (BackendDescriptor, RequestOptions) {
    // The reference shell opts into Responses-native state chaining for OpenAI
    // so the substrate path is exercised by default instead of only in tests.
    let request_options = RequestOptions {
        temperature,
        max_tokens,
        additional_params,
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

fn provider_api_key(model: &ResolvedModel, env_map: &EnvMap) -> Option<String> {
    let env_key = match model.provider {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    env_map.get_non_empty(env_key)
}

fn provider_name(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    }
}

#[cfg(test)]
mod tests {
    use super::build_memory_reasoning_service;
    use crate::config::{AgentCoreConfig, TuiConfig};
    use agent_env::EnvMap;
    use nanoclaw_config::{
        ModelCapabilitiesConfig, ProviderKind, ResolvedInternalProfile, ResolvedModel,
    };
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
        let config = AgentCoreConfig {
            core: nanoclaw_config::CoreConfig::default(),
            primary_profile: nanoclaw_config::CoreConfig::default()
                .resolve_primary_agent()
                .unwrap(),
            summary_profile: nanoclaw_config::CoreConfig::default()
                .resolve_summary_profile()
                .unwrap(),
            memory_profile: profile,
            tui: TuiConfig::default(),
        };

        let service = build_memory_reasoning_service(&config, &env_map);

        assert_eq!(service.api_key.as_deref(), Some("env-memory-key"));
    }
}
