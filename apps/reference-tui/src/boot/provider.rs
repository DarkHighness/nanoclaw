use crate::config::AgentCoreConfig;
use agent::inference::LlmServiceConfig;
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent_env::EnvMap;
use anyhow::Result;
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile};

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

pub(super) fn provider_summary(config: &AgentCoreConfig) -> String {
    let model = &config.primary_profile.model;
    let provider = provider_name(&model.provider);
    format!("{} -> {provider} / {}", model.alias, model.model)
}

fn build_agent_backend(
    profile: &ResolvedAgentProfile,
    env_map: &EnvMap,
) -> Result<ProviderBackend> {
    let descriptor = BackendDescriptor::new(match profile.model.provider {
        ProviderKind::OpenAi => ProviderDescriptor::openai(profile.model.model.clone()),
        ProviderKind::Anthropic => ProviderDescriptor::anthropic(profile.model.model.clone()),
    });

    // The reference shell opts into Responses-native state chaining for the
    // foreground profile so the default host path exercises provider-managed
    // history rather than only the append-only local fallback.
    let request_options = RequestOptions {
        temperature: profile.temperature,
        max_tokens: Some(profile.max_output_tokens),
        additional_params: profile.additional_params.clone(),
        openai_responses: matches!(profile.model.provider, ProviderKind::OpenAi).then(|| {
            OpenAiResponsesOptions {
                chain_previous_response: true,
                store: Some(true),
                server_compaction: Some(OpenAiServerCompaction {
                    compact_threshold: profile.compact_trigger_tokens,
                }),
            }
        }),
        ..RequestOptions::default()
    };

    Ok(ProviderBackend::from_settings_with_api_key(
        descriptor,
        request_options,
        profile.model.base_url.clone(),
        provider_api_key(&profile.model, env_map),
    )?)
}

fn build_internal_backend(
    profile: &ResolvedInternalProfile,
    env_map: &EnvMap,
) -> Result<ProviderBackend> {
    let descriptor = BackendDescriptor::new(match profile.model.provider {
        ProviderKind::OpenAi => ProviderDescriptor::openai(profile.model.model.clone()),
        ProviderKind::Anthropic => ProviderDescriptor::anthropic(profile.model.model.clone()),
    });
    let request_options = RequestOptions {
        temperature: profile.temperature,
        max_tokens: Some(profile.max_output_tokens),
        additional_params: profile.additional_params.clone(),
        ..RequestOptions::default()
    };
    Ok(ProviderBackend::from_settings_with_api_key(
        descriptor,
        request_options,
        profile.model.base_url.clone(),
        provider_api_key(&profile.model, env_map),
    )?)
}

fn provider_api_key(model: &nanoclaw_config::ResolvedModel, env_map: &EnvMap) -> Option<String> {
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
