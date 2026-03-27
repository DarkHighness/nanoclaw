use crate::config::{AgentCoreConfig, ProviderKind};
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent_env::vars;
use anyhow::{Result, anyhow};

pub(super) fn build_backend(config: &AgentCoreConfig) -> Result<ProviderBackend> {
    let model = config.provider.model.clone().ok_or_else(|| {
        anyhow!(
            "missing provider model; set `provider.model` in `.nanoclaw/config/core.toml` or `NANOCLAW_CORE_MODEL`"
        )
    })?;
    let provider_kind = resolved_provider_kind(config, &model);
    let descriptor = BackendDescriptor::new(match provider_kind {
        ProviderKind::OpenAi => ProviderDescriptor::openai(model),
        ProviderKind::Anthropic => ProviderDescriptor::anthropic(model),
    });

    // The reference shell opts into Responses-native state chaining for OpenAI
    // so the substrate path is exercised by default instead of only in tests.
    let request_options = RequestOptions {
        temperature: config.provider.temperature,
        max_tokens: config.provider.max_tokens,
        additional_params: config.provider.additional_params.clone(),
        openai_responses: matches!(provider_kind, ProviderKind::OpenAi).then(|| {
            OpenAiResponsesOptions {
                chain_previous_response: true,
                store: Some(true),
                server_compaction: config
                    .runtime
                    .compact_trigger_tokens
                    .map(|compact_threshold| OpenAiServerCompaction { compact_threshold }),
            }
        }),
        ..RequestOptions::default()
    };

    Ok(ProviderBackend::from_settings_with_api_key(
        descriptor,
        request_options,
        config.provider.base_url.clone(),
        configured_provider_api_key(config, &provider_kind),
    )?)
}

pub(super) fn provider_summary(config: &AgentCoreConfig, backend: &ProviderBackend) -> String {
    let provider = match resolved_provider_kind(config, &backend.descriptor().provider.model) {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    };
    format!("{provider} / {}", backend.descriptor().provider.model)
}

fn configured_provider_api_key(
    config: &AgentCoreConfig,
    provider_kind: &ProviderKind,
) -> Option<String> {
    let env_key = match provider_kind {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    config.provider.env.get(env_key).cloned()
}

fn resolved_provider_kind(config: &AgentCoreConfig, model: &str) -> ProviderKind {
    if let Some(kind) = &config.provider.kind {
        return kind.clone();
    }
    if model.trim().starts_with("claude") {
        return ProviderKind::Anthropic;
    }
    let has_openai = config.provider.env.contains_key("OPENAI_API_KEY")
        || agent_env::has_non_empty(vars::OPENAI_API_KEY);
    let has_anthropic = config.provider.env.contains_key("ANTHROPIC_API_KEY")
        || agent_env::has_non_empty(vars::ANTHROPIC_API_KEY);
    match (has_openai, has_anthropic) {
        (false, true) => ProviderKind::Anthropic,
        _ => ProviderKind::OpenAi,
    }
}
