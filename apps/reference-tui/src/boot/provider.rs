use crate::config::AgentCoreConfig;
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use anyhow::Result;
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile};

pub(super) fn build_backend(config: &AgentCoreConfig) -> Result<ProviderBackend> {
    build_agent_backend(&config.primary_profile)
}

pub(super) fn build_summary_backend(config: &AgentCoreConfig) -> Result<ProviderBackend> {
    build_internal_backend(&config.summary_profile)
}

pub(super) fn provider_summary(config: &AgentCoreConfig) -> String {
    let model = &config.primary_profile.model;
    let provider = match model.provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    };
    format!("{} -> {provider} / {}", model.alias, model.model)
}

fn build_agent_backend(profile: &ResolvedAgentProfile) -> Result<ProviderBackend> {
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
        configured_provider_api_key(&profile.model),
    )?)
}

fn build_internal_backend(profile: &ResolvedInternalProfile) -> Result<ProviderBackend> {
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
        configured_provider_api_key(&profile.model),
    )?)
}

fn configured_provider_api_key(model: &nanoclaw_config::ResolvedModel) -> Option<String> {
    let env_key = match model.provider {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    model.env.get(env_key).cloned()
}
