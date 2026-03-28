use agent::inference::LlmServiceConfig;
use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent::runtime::ModelBackendCapabilities;
use agent_env::{EnvMap, vars};
use anyhow::{Result, bail};
use nanoclaw_config::{ProviderKind, ResolvedAgentProfile, ResolvedInternalProfile, ResolvedModel};
use std::collections::BTreeMap;
use serde_json::Value;

const DEFAULT_INTERNAL_MEMORY_TIMEOUT_MS: u64 = 30_000;

pub(crate) fn build_agent_backend(profile: &ResolvedAgentProfile) -> Result<ProviderBackend> {
    build_backend_from_model(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        matches!(profile.model.provider, ProviderKind::OpenAi)
            .then_some(profile.compact_trigger_tokens),
    )
}

pub(crate) fn build_internal_backend(profile: &ResolvedInternalProfile) -> Result<ProviderBackend> {
    build_backend_from_model(
        &profile.model,
        profile.temperature,
        Some(profile.max_output_tokens),
        profile.additional_params.clone(),
        None,
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
        api_key: resolved_provider_api_key(&profile.model, env_map),
        headers: BTreeMap::new(),
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
    )
    .0
    .capabilities
}

pub(crate) fn provider_summary(model: &ResolvedModel) -> String {
    let provider = provider_name(&model.provider);
    format!("{provider} / {}", model.model)
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
    let has_openai = model.env.contains_key("OPENAI_API_KEY")
        || env_map.get_non_empty(vars::OPENAI_API_KEY.key).is_some();
    let has_anthropic = model.env.contains_key("ANTHROPIC_API_KEY")
        || env_map.get_non_empty(vars::ANTHROPIC_API_KEY.key).is_some();
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
        configured_provider_api_key(model),
    )?)
}

fn build_backend_settings(
    model: &ResolvedModel,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<Value>,
    compact_trigger_tokens: Option<usize>,
) -> (BackendDescriptor, RequestOptions) {
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
fn configured_provider_api_key(model: &ResolvedModel) -> Option<String> {
    resolved_provider_api_key(model, &EnvMap::from_process())
}

fn resolved_provider_api_key(model: &ResolvedModel, env_map: &EnvMap) -> Option<String> {
    let env_key = match model.provider {
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    };
    model
        .env
        .get(env_key)
        .cloned()
        .or_else(|| env_map.get_non_empty(env_key))
}

#[cfg(test)]
mod tests {
    use super::build_memory_reasoning_service;
    use agent_env::EnvMap;
    use nanoclaw_config::{
        ModelCapabilitiesConfig, ProviderKind, ResolvedInternalProfile, ResolvedModel,
    };
    use std::collections::BTreeMap;
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
                env: BTreeMap::new(),
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
