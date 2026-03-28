use crate::config::CodeAgentConfig;
use crate::provider::ensure_api_key_available;
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use nanoclaw_config::{CoreConfig, PluginsConfig, ResolvedAgentProfile, ResolvedInternalProfile};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct AppOptions {
    pub(crate) core: CoreConfig,
    pub(crate) primary_profile: ResolvedAgentProfile,
    pub(crate) summary_profile: ResolvedInternalProfile,
    pub(crate) memory_profile: ResolvedInternalProfile,
    pub(crate) skill_roots: Vec<PathBuf>,
    pub(crate) plugins: PluginsConfig,
    pub(crate) workspace_only: bool,
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
        let core = workspace_config.core.clone();
        let mut primary_profile = core.resolve_primary_agent()?;
        let summary_profile = core.resolve_summary_profile()?;
        let memory_profile = core.resolve_memory_profile()?;
        let mut skill_roots = core.resolved_skill_roots(workspace_root);
        let mut plugins = core.plugins.clone();
        let mut sandbox_fail_if_unavailable = core.host.sandbox_fail_if_unavailable;
        let lsp_enabled = workspace_config.lsp_enabled;
        let lsp_auto_install = workspace_config.lsp_auto_install;
        let lsp_install_root = workspace_config.lsp_install_root.clone();
        let mut prompt_parts = Vec::new();

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--system-prompt" => {
                    primary_profile.system_prompt = Some(next_arg(&mut args, "--system-prompt")?)
                }
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
                "--provider" => {
                    bail!(
                        "`--provider` was removed; configure `models.*` and `agents.primary.model` in `.nanoclaw/config/core.toml`"
                    )
                }
                _ if arg.starts_with("--") => bail!("unknown option `{arg}`"),
                _ => {
                    prompt_parts.push(arg);
                    prompt_parts.extend(args);
                    break;
                }
            }
        }

        ensure_api_key_available(&primary_profile.model, env_map)?;
        ensure_api_key_available(&summary_profile.model, env_map)?;
        ensure_api_key_available(&memory_profile.model, env_map)?;
        let one_shot_prompt = (!prompt_parts.is_empty()).then(|| prompt_parts.join(" "));

        Ok(Self {
            core,
            primary_profile,
            summary_profile,
            memory_profile,
            skill_roots,
            plugins,
            workspace_only: workspace_config.core.host.workspace_only,
            sandbox_fail_if_unavailable,
            tokio_worker_threads: workspace_config.core.host.tokio_worker_threads,
            tokio_max_blocking_threads: workspace_config.core.host.tokio_max_blocking_threads,
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
    println!();
    println!("options:");
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
    println!("  NANOCLAW_CORE_* for shared host/plugin settings");
    println!(
        "  CODE_AGENT_LSP_ENABLED / CODE_AGENT_LSP_AUTO_INSTALL / CODE_AGENT_LSP_INSTALL_ROOT"
    );
}
