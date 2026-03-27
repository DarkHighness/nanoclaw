use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent_env::{EnvMap, vars};
use anyhow::{Result, bail};
use nanoclaw_config::ProviderKind;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_TRIGGER_TOKENS: usize = 96_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectedProvider {
    OpenAi,
    Anthropic,
}

impl SelectedProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

impl fmt::Display for SelectedProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn build_backend(
    provider: SelectedProvider,
    model: String,
    base_url: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    additional_params: Option<Value>,
    provider_env: &BTreeMap<String, String>,
) -> Result<ProviderBackend> {
    let descriptor = BackendDescriptor::new(match provider {
        SelectedProvider::OpenAi => ProviderDescriptor::openai(model),
        SelectedProvider::Anthropic => ProviderDescriptor::anthropic(model),
    });
    let request_options = RequestOptions {
        temperature,
        max_tokens,
        additional_params,
        openai_responses: matches!(provider, SelectedProvider::OpenAi).then(|| {
            OpenAiResponsesOptions {
                chain_previous_response: true,
                store: Some(true),
                server_compaction: Some(OpenAiServerCompaction {
                    compact_threshold: DEFAULT_TRIGGER_TOKENS,
                }),
            }
        }),
        ..RequestOptions::default()
    };
    Ok(ProviderBackend::from_settings_with_api_key(
        descriptor,
        request_options,
        base_url,
        configured_provider_api_key(provider, provider_env),
    )?)
}

pub(crate) fn parse_provider(value: &str) -> Result<SelectedProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai" => Ok(SelectedProvider::OpenAi),
        "anthropic" => Ok(SelectedProvider::Anthropic),
        other => bail!("unsupported provider `{other}`"),
    }
}

pub(crate) fn selected_provider_from_kind(kind: ProviderKind) -> SelectedProvider {
    match kind {
        ProviderKind::OpenAi => SelectedProvider::OpenAi,
        ProviderKind::Anthropic => SelectedProvider::Anthropic,
    }
}

pub(crate) fn ensure_api_key_available(
    provider: SelectedProvider,
    provider_env: &BTreeMap<String, String>,
    env_map: &EnvMap,
) -> Result<()> {
    let has_openai = provider_env.contains_key("OPENAI_API_KEY")
        || env_map.get_non_empty(vars::OPENAI_API_KEY.key).is_some();
    let has_anthropic = provider_env.contains_key("ANTHROPIC_API_KEY")
        || env_map.get_non_empty(vars::ANTHROPIC_API_KEY.key).is_some();
    match provider {
        SelectedProvider::OpenAi if !has_openai => {
            bail!("missing OPENAI_API_KEY for provider openai")
        }
        SelectedProvider::Anthropic if !has_anthropic => {
            bail!("missing ANTHROPIC_API_KEY for provider anthropic")
        }
        _ => Ok(()),
    }
}

pub(crate) fn default_model(provider: SelectedProvider) -> &'static str {
    match provider {
        SelectedProvider::OpenAi => DEFAULT_OPENAI_MODEL,
        SelectedProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
    }
}

fn configured_provider_api_key(
    provider: SelectedProvider,
    provider_env: &BTreeMap<String, String>,
) -> Option<String> {
    let env_key = match provider {
        SelectedProvider::OpenAi => "OPENAI_API_KEY",
        SelectedProvider::Anthropic => "ANTHROPIC_API_KEY",
    };
    provider_env.get(env_key).cloned()
}
