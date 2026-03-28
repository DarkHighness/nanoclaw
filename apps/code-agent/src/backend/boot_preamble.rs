use agent::{AgentWorkspaceLayout, SkillCatalog};
use nanoclaw_config::{PluginsConfig, ResolvedAgentProfile};
use std::path::{Path, PathBuf};

pub(crate) fn build_system_preamble(
    profile: &ResolvedAgentProfile,
    skill_catalog: &SkillCatalog,
    plugin_instructions: &[String],
) -> Vec<String> {
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
    for prompt in [
        profile.global_system_prompt.as_deref(),
        profile.system_prompt.as_deref(),
    ] {
        if let Some(system_prompt) = prompt.map(str::trim).filter(|value| !value.is_empty()) {
            preamble.push(system_prompt.to_string());
        }
    }
    preamble.extend(plugin_instructions.iter().cloned());
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}

pub(crate) fn resolve_skill_roots(
    configured_roots: &[PathBuf],
    workspace_root: &Path,
    plugin_plan: &agent::plugins::PluginActivationPlan,
) -> Vec<PathBuf> {
    let mut roots = if configured_roots.is_empty() {
        default_skill_roots(workspace_root)
    } else {
        configured_roots.to_vec()
    };
    roots.extend(plugin_plan.skill_roots.clone());
    roots.retain(|path| path.exists());
    roots.sort();
    roots.dedup();
    roots
}

pub(crate) fn build_plugin_activation_plan(
    workspace_root: &Path,
    plugins: &PluginsConfig,
) -> anyhow::Result<agent::plugins::PluginActivationPlan> {
    let resolver = agent::PluginBootResolverConfig {
        enabled: plugins.enabled,
        roots: plugins
            .roots
            .iter()
            .map(|value| {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    workspace_root.join(path)
                }
            })
            .collect::<Vec<_>>(),
        include_builtin: plugins.include_builtin,
        allow: plugins.allow.clone(),
        deny: plugins.deny.clone(),
        entries: plugins.entries.clone(),
        slots: plugins.slots.clone(),
    };
    agent::build_plugin_activation_plan(workspace_root, &resolver)
}

fn default_skill_roots(workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_if_exists(&mut roots, workspace_root.join(".codex/skills"));
    push_if_exists(
        &mut roots,
        AgentWorkspaceLayout::new(workspace_root).skills_dir(),
    );
    if let Some(home) = agent_env::home_dir() {
        push_if_exists(&mut roots, home.join(".codex/skills"));
    }
    roots
}

fn push_if_exists(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if path.exists() && !roots.iter().any(|candidate| candidate == &path) {
        roots.push(path);
    }
}
