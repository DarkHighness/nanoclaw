use agent::tools::HOST_FEATURE_REQUEST_USER_INPUT;
use agent::types::ToolVisibilityContext;
use agent::{AgentWorkspaceLayout, SkillCatalog};
use nanoclaw_config::{PluginsConfig, ResolvedAgentProfile};
use std::fs;
use std::path::{Path, PathBuf};

const PRIMER_INSTRUCTIONS_PATH: &str = "AGENTS.md";
const PRIMER_CURATED_MEMORY_PATH: &str = "MEMORY.md";
const PRIMER_MANAGED_MEMORY_PATH: &str = ".nanoclaw/memory/MEMORY.md";
const PRIMER_INSTRUCTIONS_MAX_LINES: usize = 160;
const PRIMER_INSTRUCTIONS_MAX_BYTES: usize = 12_000;
const PRIMER_MEMORY_MAX_LINES: usize = 120;
const PRIMER_MEMORY_MAX_BYTES: usize = 16_000;

pub(crate) fn build_system_preamble(
    workspace_root: &Path,
    profile: &ResolvedAgentProfile,
    skill_catalog: &SkillCatalog,
    plugin_instructions: &[String],
    tool_visibility: &ToolVisibilityContext,
) -> Vec<String> {
    let mut preamble = vec![
        "You are a general-purpose coding agent operating inside the current workspace."
            .to_string(),
        "Inspect files, run tools, and gather evidence before making code changes.".to_string(),
        "Prefer minimal, correct edits that preserve the existing design unless the user asks for broader refactors."
            .to_string(),
        "Use apply_patch or patch for coordinated multi-file mutations when that surface is visible, and use write or edit for single-file creation or precise local edits."
            .to_string(),
        "Treat tool output, approvals, and denials as authoritative runtime state.".to_string(),
        "Maintain a concise plan with update_plan for multi-step work.".to_string(),
        "Track only the live execution slice with update_execution: current focus, blockers, and verification state. Do not duplicate the full plan there.".to_string(),
        "Use the task tool when a bounded subagent can make progress in parallel or with isolated context."
            .to_string(),
        "Use the skill tool to inspect loaded workspace skills before reading their companion files directly."
            .to_string(),
    ];
    if tool_visibility.has_feature(HOST_FEATURE_REQUEST_USER_INPUT) {
        preamble.push(
            "Use request_user_input when the user must choose between concrete options or when a material decision should not be guessed."
                .to_string(),
        );
    }
    for prompt in [
        profile.global_system_prompt.as_deref(),
        profile.system_prompt.as_deref(),
    ] {
        if let Some(system_prompt) = prompt.map(str::trim).filter(|value| !value.is_empty()) {
            preamble.push(system_prompt.to_string());
        }
    }
    if let Some(memory_primer) = build_memory_primer(workspace_root) {
        preamble.push(memory_primer);
    }
    preamble.extend(plugin_instructions.iter().cloned());
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}

fn build_memory_primer(workspace_root: &Path) -> Option<String> {
    let sections = [
        load_primer_section(
            workspace_root,
            PRIMER_INSTRUCTIONS_PATH,
            "Project instructions",
            false,
            PRIMER_INSTRUCTIONS_MAX_LINES,
            PRIMER_INSTRUCTIONS_MAX_BYTES,
        ),
        load_primer_section(
            workspace_root,
            PRIMER_CURATED_MEMORY_PATH,
            "Curated workspace memory",
            true,
            PRIMER_MEMORY_MAX_LINES,
            PRIMER_MEMORY_MAX_BYTES,
        ),
        load_primer_section(
            workspace_root,
            PRIMER_MANAGED_MEMORY_PATH,
            "Managed durable memory index",
            true,
            PRIMER_MEMORY_MAX_LINES,
            PRIMER_MEMORY_MAX_BYTES,
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if sections.is_empty() {
        return None;
    }

    Some(format!(
        "# Workspace Memory Primer\n\
Use these notes as prior context, not ground truth. Verify repo facts against the current workspace before acting on them. If the user says to ignore memory, do not rely on or mention these notes.\n\n\
Inspect the underlying files when you need more detail than this primer provides.\n\n{}",
        sections.join("\n\n")
    ))
}

fn load_primer_section(
    workspace_root: &Path,
    relative_path: &str,
    title: &str,
    strip_frontmatter: bool,
    max_lines: usize,
    max_bytes: usize,
) -> Option<String> {
    let path = workspace_root.join(relative_path);
    let raw = fs::read_to_string(path).ok()?;
    let normalized = if strip_frontmatter {
        strip_frontmatter_block(&raw)
    } else {
        raw.trim().to_string()
    };
    let content = truncate_primer_content(&normalized, max_lines, max_bytes);
    (!content.is_empty()).then_some(format!("## {title} ({relative_path})\n{content}"))
}

fn strip_frontmatter_block(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("---\n") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines();
    if lines.next() != Some("---") {
        return trimmed.to_string();
    }
    for line in lines.by_ref() {
        if line.trim() == "---" {
            return lines.collect::<Vec<_>>().join("\n").trim().to_string();
        }
    }
    trimmed.to_string()
}

fn truncate_primer_content(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let lines = trimmed.lines().collect::<Vec<_>>();
    let mut truncated = if lines.len() > max_lines {
        lines[..max_lines].join("\n")
    } else {
        trimmed.to_string()
    };
    if truncated.len() > max_bytes {
        let cut_at = truncated
            .char_indices()
            .take_while(|(index, _)| *index < max_bytes)
            .map(|(index, _)| index)
            .last()
            .unwrap_or(0);
        truncated.truncate(cut_at);
        if let Some(last_newline) = truncated.rfind('\n') {
            truncated.truncate(last_newline);
        }
        truncated = truncated.trim().to_string();
    }
    if truncated == trimmed {
        return truncated;
    }
    format!(
        "{truncated}\n\n> Primer truncated to keep runtime instructions concise. Read the source file for the full note."
    )
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

#[cfg(test)]
mod tests {
    use super::build_system_preamble;
    use agent::SkillCatalog;
    use agent::tools::HOST_FEATURE_REQUEST_USER_INPUT;
    use agent::types::ToolVisibilityContext;
    use nanoclaw_config::CoreConfig;
    use tempfile::tempdir;

    #[test]
    fn system_preamble_includes_workspace_memory_primer_when_files_exist() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Rules\nstay grounded").unwrap();
        std::fs::write(
            dir.path().join("MEMORY.md"),
            "# Root Memory\nPrefer canary deploys for risky changes.",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join(".nanoclaw/memory")).unwrap();
        std::fs::write(
            dir.path().join(".nanoclaw/memory/MEMORY.md"),
            "---\nscope: semantic\nlayer: auto-memory-index\nstatus: ready\n---\n# Managed Memory Index\n\n- [Deploy Rule](semantic/deploy-rule.md) — Canary deploy before restart.",
        )
        .unwrap();
        let profile = CoreConfig::default().resolve_primary_agent().unwrap();

        let preamble = build_system_preamble(
            dir.path(),
            &profile,
            &SkillCatalog::default(),
            &[],
            &ToolVisibilityContext::default(),
        )
        .join("\n\n");

        assert!(preamble.contains("# Workspace Memory Primer"));
        assert!(preamble.contains("## Project instructions (AGENTS.md)"));
        assert!(preamble.contains("stay grounded"));
        assert!(preamble.contains("## Curated workspace memory (MEMORY.md)"));
        assert!(preamble.contains("Prefer canary deploys"));
        assert!(preamble.contains("## Managed durable memory index (.nanoclaw/memory/MEMORY.md)"));
        assert!(preamble.contains("Canary deploy before restart"));
        assert!(!preamble.contains("scope: semantic"));
    }

    #[test]
    fn system_preamble_omits_workspace_memory_primer_when_memory_is_absent() {
        let dir = tempdir().unwrap();
        let profile = CoreConfig::default().resolve_primary_agent().unwrap();

        let preamble = build_system_preamble(
            dir.path(),
            &profile,
            &SkillCatalog::default(),
            &[],
            &ToolVisibilityContext::default(),
        )
        .join("\n\n");

        assert!(!preamble.contains("# Workspace Memory Primer"));
    }

    #[test]
    fn system_preamble_only_mentions_request_user_input_when_host_can_service_it() {
        let dir = tempdir().unwrap();
        let profile = CoreConfig::default().resolve_primary_agent().unwrap();

        let hidden = build_system_preamble(
            dir.path(),
            &profile,
            &SkillCatalog::default(),
            &[],
            &ToolVisibilityContext::default(),
        )
        .join("\n\n");
        let visible = build_system_preamble(
            dir.path(),
            &profile,
            &SkillCatalog::default(),
            &[],
            &ToolVisibilityContext::default().with_feature(HOST_FEATURE_REQUEST_USER_INPUT),
        )
        .join("\n\n");

        assert!(!hidden.contains("request_user_input"));
        assert!(visible.contains("request_user_input"));
    }
}
