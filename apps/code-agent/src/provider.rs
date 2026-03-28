use agent::inference::LlmServiceConfig;
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent_env::{EnvMap, vars};
use anyhow::{Result, bail};
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile, ResolvedModel};

const DEFAULT_INTERNAL_MEMORY_TIMEOUT_MS: u64 = 30_000;

pub(crate) fn build_agent_backend(
    profile: &ResolvedAgentProfile,
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

pub(crate) fn build_internal_backend(
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

pub(crate) fn ensure_api_key_available(model: &ResolvedModel, env_map: &EnvMap) -> Result<()> {
    let present = match model.provider {
        ProviderKind::OpenAi => env_map.get_non_empty(vars::OPENAI_API_KEY.key).is_some(),
        ProviderKind::Anthropic => env_map.get_non_empty(vars::ANTHROPIC_API_KEY.key).is_some(),
    };
    if present {
        return Ok(());
    }
    match model.provider {
        ProviderKind::OpenAi => bail!("missing OPENAI_API_KEY for provider openai"),
        ProviderKind::Anthropic => bail!("missing ANTHROPIC_API_KEY for provider anthropic"),
    }
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

fn provider_api_key(model: &ResolvedModel, env_map: &EnvMap) -> Option<String> {
    let env_key = match model.provider {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    env_map.get_non_empty(env_key)
}

#[cfg(test)]
mod tests {
    use super::build_memory_reasoning_service;
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

        let service = build_memory_reasoning_service(&profile, &env_map);

        assert_eq!(service.api_key.as_deref(), Some("env-memory-key"));
    }
}
