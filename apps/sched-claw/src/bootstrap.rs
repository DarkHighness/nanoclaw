use crate::app_config::{CliOverrides, SchedClawConfig};
use crate::builtin_skills::{builtin_skill_root, materialize_builtin_skills};
use crate::daemon_client::SchedExtDaemonClient;
use crate::daemon_tool::SchedClawDaemonTool;
use crate::preamble::build_system_preamble;
use crate::startup_catalog::StartupCatalog;
use agent::tools::{
    HostEscapePolicy, ManagedPolicyProcessExecutor, NetworkPolicy, SandboxMode, WebFetchTool,
    WebSearchTool,
};
use agent::{
    AgentRuntime, AgentRuntimeBuilder, AgentWorkspaceLayout, EditTool, ExecCommandTool, GlobTool,
    GrepTool, HookRunner, ListTool, PRIMARY_WORKTREE_ID, PatchFilesTool, ReadTool, SandboxPolicy,
    SkillCatalog, SkillRoot, SkillViewTool, SkillsListTool, ToolDiscoverTool, ToolExecutionContext,
    ToolRegistry, WorktreeId, WriteStdinTool, WriteTool,
};
use anyhow::{Context, Result};
use code_agent_backend::{build_mutable_agent_backend, ensure_api_key_available};
use nanoclaw_config::AgentSandboxMode;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

pub struct RuntimeBootstrap {
    config: SchedClawConfig,
    skill_catalog: SkillCatalog,
    startup_catalog: StartupCatalog,
    tool_context: ToolExecutionContext,
    instructions: Vec<String>,
    tool_registry: ToolRegistry,
    daemon_client: SchedExtDaemonClient,
}

pub struct BuiltRuntime {
    pub config: SchedClawConfig,
    pub runtime: AgentRuntime,
    pub startup_catalog: StartupCatalog,
    pub daemon_client: SchedExtDaemonClient,
    pub workspace_root: PathBuf,
}

pub async fn load_bootstrap(
    workspace_root: &Path,
    overrides: &CliOverrides,
) -> Result<RuntimeBootstrap> {
    AgentWorkspaceLayout::new(workspace_root).ensure_standard_layout()?;
    std::fs::create_dir_all(crate::app_config::app_state_dir(workspace_root))?;
    let config = SchedClawConfig::load_from_dir(workspace_root, overrides)?;
    materialize_builtin_skills(workspace_root)?;
    let skill_roots = resolve_skill_roots(workspace_root, &config.skill_roots);
    let skill_catalog = filter_disabled_builtin_skills(
        workspace_root,
        &config.disabled_builtin_skills,
        agent::load_skill_roots(&skill_roots)
            .await
            .context("failed to load sched-claw skill roots")?,
    );
    let tool_context = build_tool_context(workspace_root, &config);
    let daemon_client = SchedExtDaemonClient::new(config.daemon.clone());
    let tool_registry = build_tool_registry(&tool_context, &skill_catalog, daemon_client.clone());
    apply_disabled_tools(&tool_registry, &config.disabled_tools);
    let startup_catalog = StartupCatalog::from_parts(tool_registry.specs(), &skill_catalog);
    let instructions =
        build_system_preamble(workspace_root, &config.primary_profile, &skill_catalog);

    Ok(RuntimeBootstrap {
        config,
        skill_catalog,
        startup_catalog,
        tool_context,
        instructions,
        tool_registry,
        daemon_client,
    })
}

impl RuntimeBootstrap {
    pub fn startup_catalog(&self) -> &StartupCatalog {
        &self.startup_catalog
    }

    pub async fn build_runtime(self) -> Result<BuiltRuntime> {
        self.config.env_map.apply_to_process();
        ensure_api_key_available(&self.config.primary_profile.model, &self.config.env_map)?;
        let backend = Arc::new(build_mutable_agent_backend(
            &self.config.primary_profile,
            &self.config.env_map,
        )?);
        let store = build_store(&self.config.core, &self.tool_context.workspace_root).await?;
        let workspace_root = self.tool_context.workspace_root.clone();
        let runtime = AgentRuntimeBuilder::new(backend, store)
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_registry(self.tool_registry)
            .tool_context(self.tool_context)
            .instructions(self.instructions)
            .skill_catalog(self.skill_catalog)
            .build();
        Ok(BuiltRuntime {
            config: self.config,
            runtime,
            startup_catalog: self.startup_catalog,
            daemon_client: self.daemon_client,
            workspace_root,
        })
    }
}

fn build_tool_registry(
    tool_context: &ToolExecutionContext,
    skill_catalog: &SkillCatalog,
    daemon_client: SchedExtDaemonClient,
) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    let discovery_registry = tools.clone();
    tools.register(ReadTool::new());
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(PatchFilesTool::new());
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(ExecCommandTool::with_process_executor_and_policy(
        Arc::new(ManagedPolicyProcessExecutor::new()),
        tool_context.sandbox_policy(),
    ));
    tools.register(WriteStdinTool::new());
    tools.register(WebFetchTool::new());
    tools.register(WebSearchTool::new());
    tools.register(ToolDiscoverTool::new(discovery_registry));
    tools.register(SkillsListTool::new(skill_catalog.clone()));
    tools.register(SkillViewTool::new(skill_catalog.clone()));
    tools.register(SchedClawDaemonTool::new(daemon_client));
    tools
}

fn apply_disabled_tools(tools: &ToolRegistry, disabled: &BTreeSet<String>) {
    for tool_name in disabled {
        tools.remove(tool_name);
    }
}

fn build_tool_context(workspace_root: &Path, config: &SchedClawConfig) -> ToolExecutionContext {
    let mut context = ToolExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        worktree_root: Some(workspace_root.to_path_buf()),
        active_worktree_id: Some(WorktreeId::from(PRIMARY_WORKTREE_ID)),
        workspace_only: config.workspace_only,
        model_context_window_tokens: Some(config.primary_profile.context_window_tokens),
        ..Default::default()
    };
    context.effective_sandbox_policy = Some(match config.primary_profile.sandbox {
        AgentSandboxMode::DangerFullAccess => {
            SandboxPolicy::permissive().with_fail_if_unavailable(config.sandbox_fail_if_unavailable)
        }
        AgentSandboxMode::WorkspaceWrite => context
            .sandbox_scope()
            .recommended_policy()
            .with_fail_if_unavailable(config.sandbox_fail_if_unavailable),
        AgentSandboxMode::ReadOnly => {
            context.workspace_only = true;
            let derived = context
                .sandbox_scope()
                .recommended_policy()
                .with_fail_if_unavailable(config.sandbox_fail_if_unavailable);
            SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                filesystem: agent::tools::FilesystemPolicy {
                    readable_roots: derived.filesystem.readable_roots,
                    writable_roots: Vec::new(),
                    executable_roots: derived.filesystem.executable_roots,
                    protected_paths: derived.filesystem.protected_paths,
                },
                network: NetworkPolicy::Off,
                host_escape: HostEscapePolicy::Deny,
                fail_if_unavailable: derived.fail_if_unavailable,
            }
        }
    });
    context
}

async fn build_store(
    core: &nanoclaw_config::CoreConfig,
    workspace_root: &Path,
) -> Result<Arc<dyn agent::SessionStore>> {
    let store_dir = core.resolved_store_dir(workspace_root);
    match agent::FileSessionStore::open(&store_dir).await {
        Ok(store) => Ok(Arc::new(store)),
        Err(error) => {
            warn!(
                "failed to initialize file session store at {}: {error}; falling back to in-memory store",
                store_dir.display()
            );
            Ok(Arc::new(agent::InMemorySessionStore::new()))
        }
    }
}

fn resolve_skill_roots(workspace_root: &Path, configured_roots: &[PathBuf]) -> Vec<SkillRoot> {
    let mut roots = vec![SkillRoot::managed(
        AgentWorkspaceLayout::new(workspace_root).skills_dir(),
    )];
    if configured_roots.is_empty() {
        push_if_exists(
            &mut roots,
            SkillRoot::external(workspace_root.join(".codex/skills")),
        );
        push_if_exists(
            &mut roots,
            SkillRoot::external(workspace_root.join("apps/code-agent/skills")),
        );
        if let Some(home) = agent_env::home_dir() {
            push_if_exists(&mut roots, SkillRoot::external(home.join(".codex/skills")));
        }
    } else {
        roots.extend(configured_roots.iter().cloned().map(SkillRoot::external));
    }
    push_if_exists(
        &mut roots,
        SkillRoot::external(builtin_skill_root(workspace_root)),
    );
    roots.retain(|root| root.path.exists() || root.kind == agent::SkillRootKind::Managed);
    let mut seen = BTreeSet::new();
    roots.retain(|root| seen.insert(root.path.clone()));
    roots
}

fn push_if_exists(roots: &mut Vec<SkillRoot>, root: SkillRoot) {
    if root.path.exists() {
        roots.push(root);
    }
}

fn filter_disabled_builtin_skills(
    workspace_root: &Path,
    disabled_builtin_skills: &BTreeSet<String>,
    skill_catalog: SkillCatalog,
) -> SkillCatalog {
    if disabled_builtin_skills.is_empty() {
        return skill_catalog;
    }
    let builtin_root = builtin_skill_root(workspace_root);
    let filtered = skill_catalog
        .all()
        .into_iter()
        .filter(|skill| {
            !(skill.provenance.root.path == builtin_root
                && disabled_builtin_skills.contains(&skill.name))
        })
        .collect();
    SkillCatalog::from_parts(skill_catalog.roots(), filtered)
}
