use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent_env::{EnvMap, vars};
use anyhow::{Result, bail};
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile, ResolvedModel};

pub(crate) fn build_agent_backend(profile: &ResolvedAgentProfile) -> Result<ProviderBackend> {
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
        configured_provider_api_key(&profile.model),
    )?)
}

pub(crate) fn build_internal_backend(profile: &ResolvedInternalProfile) -> Result<ProviderBackend> {
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

pub(crate) fn ensure_api_key_available(model: &ResolvedModel, env_map: &EnvMap) -> Result<()> {
    let present = match model.provider {
        ProviderKind::OpenAi => {
            model.env.contains_key("OPENAI_API_KEY")
                || env_map.get_non_empty(vars::OPENAI_API_KEY.key).is_some()
        }
        ProviderKind::Anthropic => {
            model.env.contains_key("ANTHROPIC_API_KEY")
                || env_map.get_non_empty(vars::ANTHROPIC_API_KEY.key).is_some()
        }
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
    let provider = match profile.model.provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    };
    format!("{} -> {provider}", profile.model.alias)
}

fn configured_provider_api_key(model: &ResolvedModel) -> Option<String> {
    let env_key = match model.provider {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    model.env.get(env_key).cloned()
}
