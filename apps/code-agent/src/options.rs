use crate::config::CodeAgentConfig;
use crate::provider::{
    SelectedProvider, default_model, ensure_api_key_available, parse_provider,
    selected_provider_from_kind,
};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use nanoclaw_config::{PluginsConfig, resolved_provider_kind};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct AppOptions {
    pub(crate) provider: SelectedProvider,
    pub(crate) model: String,
    pub(crate) base_url: Option<String>,
    pub(crate) temperature: Option<f64>,
    pub(crate) max_tokens: Option<u64>,
    pub(crate) additional_params: Option<Value>,
    pub(crate) provider_env: BTreeMap<String, String>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) skill_roots: Vec<PathBuf>,
    pub(crate) plugins: PluginsConfig,
    pub(crate) sandbox_fail_if_unavailable: bool,
    pub(crate) tokio_worker_threads: Option<usize>,
    pub(crate) tokio_max_blocking_threads: Option<usize>,
    pub(crate) lsp_enabled: bool,
    pub(crate) lsp_auto_install: bool,
    pub(crate) lsp_install_root: Option<PathBuf>,
    pub(crate) one_shot_prompt: Option<String>,
}

impl AppOptions {
    pub(crate) fn from_env_and_args(workspace_root: &Path, env_map: &EnvMap) -> Result<Self> {
        Self::from_env_and_args_iter(workspace_root, env_map, env::args().skip(1))
    }

    pub(crate) fn from_env_and_args_iter(
        workspace_root: &Path,
        env_map: &EnvMap,
        args: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let workspace_config = CodeAgentConfig::load_from_dir(workspace_root, env_map)?;
        let mut provider = None;
        let provider_from_config =
            selected_provider_from_kind(resolved_provider_kind(&workspace_config.core));
        let mut system_prompt = workspace_config.core.system_prompt.clone();
        let mut skill_roots = workspace_config.core.resolved_skill_roots(workspace_root);
        let mut plugins = workspace_config.core.plugins.clone();
        let mut sandbox_fail_if_unavailable =
            workspace_config.core.runtime.sandbox_fail_if_unavailable;
        let lsp_enabled = workspace_config.lsp_enabled;
        let lsp_auto_install = workspace_config.lsp_auto_install;
        let lsp_install_root = workspace_config.lsp_install_root.clone();
        let mut prompt_parts = Vec::new();

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--provider" => {
                    provider = Some(parse_provider(&next_arg(&mut args, "--provider")?)?)
                }
                "--system-prompt" => system_prompt = Some(next_arg(&mut args, "--system-prompt")?),
                "--skill-root" => {
                    skill_roots.push(PathBuf::from(next_arg(&mut args, "--skill-root")?))
                }
                "--plugin-root" => {
                    plugins.roots.push(next_arg(&mut args, "--plugin-root")?);
                }
                "--memory-plugin" => {
                    plugins.slots.memory = Some(next_arg(&mut args, "--memory-plugin")?);
                }
                "--sandbox-fail-if-unavailable" => {
                    sandbox_fail_if_unavailable =
                        parse_bool_flag(&next_arg(&mut args, "--sandbox-fail-if-unavailable")?)?
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                _ if arg.starts_with("--") => bail!("unknown option `{arg}`"),
                _ => {
                    prompt_parts.push(arg);
                    prompt_parts.extend(args);
                    break;
                }
            }
        }

        let provider = provider.unwrap_or(provider_from_config);
        ensure_api_key_available(provider, &workspace_config.core.provider.env, env_map)?;
        let model = if provider != provider_from_config {
            default_model(provider).to_string()
        } else {
            workspace_config
                .core
                .provider
                .model
                .clone()
                .unwrap_or_else(|| default_model(provider).to_string())
        };
        let one_shot_prompt = (!prompt_parts.is_empty()).then(|| prompt_parts.join(" "));

        Ok(Self {
            provider,
            model,
            base_url: workspace_config.core.provider.base_url.clone(),
            temperature: workspace_config.core.provider.temperature,
            max_tokens: workspace_config.core.provider.max_tokens,
            additional_params: workspace_config.core.provider.additional_params.clone(),
            provider_env: workspace_config.core.provider.env.clone(),
            system_prompt,
            skill_roots,
            plugins,
            sandbox_fail_if_unavailable,
            tokio_worker_threads: workspace_config.core.runtime.tokio_worker_threads,
            tokio_max_blocking_threads: workspace_config.core.runtime.tokio_max_blocking_threads,
            lsp_enabled,
            lsp_auto_install,
            lsp_install_root,
            one_shot_prompt,
        })
    }
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .with_context(|| format!("missing value for `{flag}`"))
}

pub(crate) fn parse_bool_flag(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => bail!("unsupported boolean value `{other}`"),
    }
}

fn print_help() {
    println!("Code Agent Example");
    println!();
    println!("usage:");
    println!("  cargo run --manifest-path apps/Cargo.toml -p code-agent");
    println!(
        "  cargo run --manifest-path apps/Cargo.toml -p code-agent -- \"fix the failing test\""
    );
    println!("  cargo run --manifest-path apps/Cargo.toml -p code-agent -- --provider anthropic");
    println!();
    println!("options:");
    println!("  --provider <openai|anthropic>");
    println!("  --system-prompt <text>");
    println!("  --skill-root <path>");
    println!("  --plugin-root <path>");
    println!("  --memory-plugin <id|none>");
    println!("  --sandbox-fail-if-unavailable <true|false>");
    println!("  -h, --help");
    println!();
    println!("environment:");
    println!("  .env and .env.local in the current workspace are loaded automatically");
    println!("  OPENAI_API_KEY / ANTHROPIC_API_KEY");
    println!("  OPENAI_BASE_URL / ANTHROPIC_BASE_URL");
    println!("  NANOCLAW_CORE_* for shared core runtime settings");
    println!(
        "  CODE_AGENT_LSP_ENABLED / CODE_AGENT_LSP_AUTO_INSTALL / CODE_AGENT_LSP_INSTALL_ROOT"
    );
}
