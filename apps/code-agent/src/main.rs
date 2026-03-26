mod tui;

use agent::provider::{
    BackendDescriptor, OpenAiResponsesOptions, OpenAiServerCompaction, ProviderBackend,
    ProviderDescriptor, RequestOptions,
};
use agent::runtime::{
    CompactionConfig, LoopDetectionConfig, ModelConversationCompactor, NoopToolApprovalPolicy,
    RuntimeSubagentExecutor, ToolApprovalHandler,
};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, BashTool, EditTool, GlobTool, GrepTool, HookRunner,
    InMemoryRunStore, ListTool, PatchTool, ReadTool, Skill, SkillCatalog, TaskTool, TodoListState,
    TodoReadTool, TodoWriteTool, ToolExecutionContext, ToolRegistry, WriteTool,
};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tui::{CodeAgentTui, make_tui_support};

const DEFAULT_OPENAI_MODEL: &str = "gpt-5.4";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_CONTEXT_TOKENS: usize = 128_000;
const DEFAULT_TRIGGER_TOKENS: usize = 96_000;
const DEFAULT_PRESERVE_RECENT_MESSAGES: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectedProvider {
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

#[derive(Clone, Debug)]
struct AppOptions {
    provider: SelectedProvider,
    model: String,
    system_prompt: Option<String>,
    skill_roots: Vec<PathBuf>,
    one_shot_prompt: Option<String>,
}

impl AppOptions {
    fn from_env_and_args(env_map: &BTreeMap<String, String>) -> Result<Self> {
        let mut provider = env_lookup(&env_map, "CODE_AGENT_PROVIDER")
            .as_deref()
            .map(parse_provider)
            .transpose()?;
        let mut system_prompt = env_lookup(&env_map, "CODE_AGENT_SYSTEM_PROMPT");
        let mut skill_roots = env_lookup(&env_map, "CODE_AGENT_SKILL_ROOTS")
            .map(split_path_list)
            .unwrap_or_default();
        let mut prompt_parts = Vec::new();

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--provider" => {
                    provider = Some(parse_provider(&next_arg(&mut args, "--provider")?)?)
                }
                "--system-prompt" => system_prompt = Some(next_arg(&mut args, "--system-prompt")?),
                "--skill-root" => {
                    skill_roots.push(PathBuf::from(next_arg(&mut args, "--skill-root")?))
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

        let has_openai = env_lookup(&env_map, "OPENAI_API_KEY").is_some();
        let has_anthropic = env_lookup(&env_map, "ANTHROPIC_API_KEY").is_some();
        let provider = provider.unwrap_or(SelectedProvider::OpenAi);
        ensure_api_key_available(provider, has_openai, has_anthropic)?;
        let model = default_model(provider).to_string();
        let one_shot_prompt = (!prompt_parts.is_empty()).then(|| prompt_parts.join(" "));

        Ok(Self {
            provider,
            model,
            system_prompt,
            skill_roots,
            one_shot_prompt,
        })
    }
}

fn main() -> Result<()> {
    let workspace_root = env::current_dir().context("failed to resolve current workspace")?;
    let env_map = load_env_map(&workspace_root)?;
    inject_process_env(&env_map);
    let options = AppOptions::from_env_and_args(&env_map)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async_main(workspace_root, options)))
}

async fn async_main(workspace_root: PathBuf, options: AppOptions) -> Result<()> {
    let (ui_state, approval_bridge, approval_handler) = make_tui_support();
    let (runtime, skills) = build_runtime(&options, &workspace_root, approval_handler).await?;
    let provider_label = options.provider.as_str().to_string();
    let model = options.model.clone();
    let initial_prompt = options.one_shot_prompt.clone();
    CodeAgentTui::new(
        runtime,
        workspace_root,
        provider_label,
        model,
        skills,
        initial_prompt,
        ui_state,
        approval_bridge,
    )
    .run()
    .await
}

async fn build_runtime(
    options: &AppOptions,
    workspace_root: &Path,
    approval_handler: Arc<dyn ToolApprovalHandler>,
) -> Result<(AgentRuntime, Vec<Skill>)> {
    let backend = Arc::new(build_backend(options)?);
    let store = Arc::new(InMemoryRunStore::new());
    let hook_runner = Arc::new(HookRunner::default());
    let skill_roots = resolve_skill_roots(&options.skill_roots, workspace_root);
    let skill_catalog = agent::skills::load_skill_roots(&skill_roots)
        .await
        .context("failed to load skill roots")?;
    let skills = skill_catalog.all().to_vec();
    let skill_hooks = skills
        .iter()
        .flat_map(|skill| skill.hooks.clone())
        .collect::<Vec<_>>();
    let instructions = build_system_preamble(options.system_prompt.as_deref(), &skill_catalog);
    let tool_context = ToolExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        worktree_root: Some(workspace_root.to_path_buf()),
        workspace_only: true,
        model_context_window_tokens: Some(DEFAULT_CONTEXT_TOKENS),
        ..Default::default()
    };
    let compactor = Arc::new(ModelConversationCompactor::new(backend.clone()));
    let loop_detection_config = LoopDetectionConfig {
        enabled: true,
        ..LoopDetectionConfig::default()
    };
    let todo_state = TodoListState::default();

    let mut tools = ToolRegistry::new();
    tools.register(ReadTool::new());
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(PatchTool::new());
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(BashTool::new());
    tools.register(TodoReadTool::new(todo_state.clone()));
    tools.register(TodoWriteTool::new(todo_state));
    let subagent_executor = RuntimeSubagentExecutor::new(
        backend.clone(),
        hook_runner.clone(),
        store.clone(),
        tools.clone(),
        tool_context.clone(),
        approval_handler.clone(),
        Arc::new(NoopToolApprovalPolicy),
        compactor.clone(),
        CompactionConfig {
            enabled: true,
            context_window_tokens: DEFAULT_CONTEXT_TOKENS,
            trigger_tokens: DEFAULT_TRIGGER_TOKENS,
            preserve_recent_messages: DEFAULT_PRESERVE_RECENT_MESSAGES,
        },
        loop_detection_config.clone(),
        instructions.clone(),
        skill_hooks.clone(),
        skill_catalog.clone(),
    );
    tools.register(TaskTool::new(Arc::new(subagent_executor)));

    let runtime = AgentRuntimeBuilder::new(backend.clone(), store)
        .hook_runner(hook_runner)
        .tool_registry(tools)
        .tool_context(tool_context)
        .tool_approval_handler(approval_handler)
        .conversation_compactor(compactor)
        .compaction_config(CompactionConfig {
            enabled: true,
            context_window_tokens: DEFAULT_CONTEXT_TOKENS,
            trigger_tokens: DEFAULT_TRIGGER_TOKENS,
            preserve_recent_messages: DEFAULT_PRESERVE_RECENT_MESSAGES,
        })
        .loop_detection_config(loop_detection_config)
        .instructions(instructions)
        .hooks(skill_hooks)
        .skill_catalog(skill_catalog)
        .build();

    Ok((runtime, skills))
}

fn build_backend(options: &AppOptions) -> Result<ProviderBackend> {
    let descriptor = BackendDescriptor::new(match options.provider {
        SelectedProvider::OpenAi => ProviderDescriptor::openai(options.model.clone()),
        SelectedProvider::Anthropic => ProviderDescriptor::anthropic(options.model.clone()),
    });
    let request_options = RequestOptions {
        openai_responses: matches!(options.provider, SelectedProvider::OpenAi).then(|| {
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
    Ok(ProviderBackend::from_settings(
        descriptor,
        request_options,
        None,
    )?)
}

fn build_system_preamble(system_prompt: Option<&str>, skill_catalog: &SkillCatalog) -> Vec<String> {
    let mut preamble = vec![
        "You are a general-purpose coding agent operating inside the current workspace."
            .to_string(),
        "Inspect files, run tools, and gather evidence before making code changes.".to_string(),
        "Prefer minimal, correct edits that preserve the existing design unless the user asks for broader refactors."
            .to_string(),
        "Use patch for coordinated multi-file mutations, and use write or edit for single-file creation or precise local edits."
            .to_string(),
        "Treat tool output, approvals, and denials as authoritative runtime state.".to_string(),
        "Maintain a concise plan with todo_read and todo_write for multi-step work.".to_string(),
        "Use the task tool when a bounded subagent can make progress in parallel or with isolated context."
            .to_string(),
    ];
    if let Some(system_prompt) = system_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        preamble.push(system_prompt.to_string());
    }
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}

fn resolve_skill_roots(configured_roots: &[PathBuf], workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots = if configured_roots.is_empty() {
        default_skill_roots(workspace_root)
    } else {
        configured_roots.to_vec()
    };
    roots.retain(|path| path.exists());
    roots.sort();
    roots.dedup();
    roots
}

fn default_skill_roots(workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_if_exists(&mut roots, workspace_root.join(".codex/skills"));
    push_if_exists(&mut roots, workspace_root.join(".agent-core/skills"));
    if let Some(home) = env::var_os("HOME") {
        push_if_exists(&mut roots, PathBuf::from(home).join(".codex/skills"));
    }
    roots
}

fn push_if_exists(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if path.exists() && !roots.iter().any(|candidate| candidate == &path) {
        roots.push(path);
    }
}

fn split_path_list(value: String) -> Vec<PathBuf> {
    env::split_paths(&value).collect()
}

fn load_env_map(workspace_root: &Path) -> Result<BTreeMap<String, String>> {
    let mut env_map = BTreeMap::new();
    load_dotenv_file(workspace_root.join(".env"), &mut env_map)?;
    load_dotenv_file(workspace_root.join(".env.local"), &mut env_map)?;
    env_map.extend(env::vars());
    Ok(env_map)
}

fn load_dotenv_file(path: PathBuf, target: &mut BTreeMap<String, String>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in dotenvy::from_path_iter(path)? {
        let (key, value) = entry?;
        target.insert(key, value);
    }
    Ok(())
}

fn inject_process_env(env_map: &BTreeMap<String, String>) {
    for (key, value) in env_map {
        // This runs before the Tokio runtime starts, so mutating process env is safe here.
        unsafe {
            env::set_var(key, value);
        }
    }
}

fn env_lookup(env_map: &BTreeMap<String, String>, name: &str) -> Option<String> {
    env_map
        .get(name)
        .cloned()
        .filter(|value| !value.trim().is_empty())
}

fn next_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .with_context(|| format!("missing value for `{flag}`"))
}

fn parse_provider(value: &str) -> Result<SelectedProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai" => Ok(SelectedProvider::OpenAi),
        "anthropic" => Ok(SelectedProvider::Anthropic),
        other => bail!("unsupported provider `{other}`"),
    }
}

fn ensure_api_key_available(
    provider: SelectedProvider,
    has_openai: bool,
    has_anthropic: bool,
) -> Result<()> {
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

fn default_model(provider: SelectedProvider) -> &'static str {
    match provider {
        SelectedProvider::OpenAi => DEFAULT_OPENAI_MODEL,
        SelectedProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
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
    println!("  -h, --help");
    println!();
    println!("environment:");
    println!("  .env and .env.local in the current workspace are loaded automatically");
    println!("  OPENAI_API_KEY / ANTHROPIC_API_KEY");
    println!("  OPENAI_BASE_URL / ANTHROPIC_BASE_URL");
    println!("  CODE_AGENT_PROVIDER / CODE_AGENT_SYSTEM_PROMPT / CODE_AGENT_SKILL_ROOTS");
}

#[cfg(test)]
mod tests {
    use super::{SelectedProvider, default_model};

    #[test]
    fn default_model_matches_provider() {
        assert_eq!(default_model(SelectedProvider::OpenAi), "gpt-5.4");
        assert_eq!(
            default_model(SelectedProvider::Anthropic),
            "claude-sonnet-4-6"
        );
    }
}
