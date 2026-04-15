use agent::AgentWorkspaceLayout;
use agent::tools::HOST_FEATURE_REQUEST_USER_INPUT;
use agent::types::ToolVisibilityContext;
use code_agent_config::builtin_skill_root;
use nanoclaw_config::{PluginsConfig, ResolvedAgentProfile};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const PRIMER_INSTRUCTIONS_PATH: &str = "AGENTS.md";
const PRIMER_CURATED_MEMORY_PATH: &str = "MEMORY.md";
const PRIMER_MANAGED_MEMORY_PATH: &str = ".nanoclaw/memory/MEMORY.md";
const SKILL_INDEX_MAX_SKILLS: usize = 64;
const SKILL_INDEX_MAX_DESCRIPTION_CHARS: usize = 160;
const SKILL_INDEX_MAX_METADATA_ITEMS: usize = 4;
const PRIMER_INSTRUCTIONS_MAX_LINES: usize = 160;
const PRIMER_INSTRUCTIONS_MAX_BYTES: usize = 12_000;
const PRIMER_MEMORY_MAX_LINES: usize = 120;
const PRIMER_MEMORY_MAX_BYTES: usize = 16_000;

pub fn build_system_preamble(
    workspace_root: &Path,
    profile: &ResolvedAgentProfile,
    skill_catalog: &agent::SkillCatalog,
    plugin_instructions: &[String],
    tool_visibility: &ToolVisibilityContext,
) -> Vec<String> {
    let mut preamble = vec![
        "You are a general-purpose coding agent operating inside the current workspace."
            .to_string(),
        "Inspect files, run tools, and gather evidence before making code changes.".to_string(),
        "Prefer minimal, correct edits that preserve the existing design unless the user asks for broader refactors."
            .to_string(),
        "Use patch_files for coordinated multi-file mutations, and use write or edit for single-file creation or precise local edits."
            .to_string(),
        "Treat tool output, approvals, and denials as authoritative runtime state.".to_string(),
        "Use task_create, task_update, task_stop, task_get, and task_list for typed execution objects and TODO tracking. Keep high-level coordination visible by maintaining task summaries, dependencies, and child-agent status instead of mirroring a separate plan surface."
            .to_string(),
        "Use checkpoint_list, checkpoint_summarize, checkpoint_restore, and review_start to inspect durable restore points, compact verbose history without changing files, restore code or conversation state, and generate a structured review of recent tool activity plus current diagnostics."
            .to_string(),
        "Use spawn_agent for bounded child work, then send_input, wait_agent, resume_agent, and close_agent to manage that child explicitly."
            .to_string(),
        "Before replying, scan the available skill index below. If a skill matches or is even partially relevant, you MUST inspect it with skill_view before acting on the task. Err on the side of loading."
            .to_string(),
        "Do not treat the injected skill index as loaded instructions. skill_view loads the actual skill body or companion files; skills_list is for broader browsing and for refreshing the catalog after skill changes."
            .to_string(),
        "Treat leading `$skill_name` directives in the user prompt as explicit requests to load that skill with skill_view before continuing with the rest of the prompt."
            .to_string(),
        "Use tool_discover when you need a typed runtime capability, host-managed lifecycle, or other integration a skill cannot supply on its own."
            .to_string(),
        "After completing a complex or iterative task, fixing a tricky error, or discovering a reusable non-trivial workflow, save or update the approach with skill_manage unless an existing skill already captures it."
            .to_string(),
        "When a loaded skill is outdated, incomplete, or wrong, patch it with skill_manage(action='patch') before finishing. Skills that are not maintained become liabilities."
            .to_string(),
        "Do not assume the host will inject memory automatically on each turn. Decide yourself when prior workspace memory is relevant."
            .to_string(),
        "Use memory_search when the task may depend on prior decisions, user preferences, previous sessions, incidents, or other project context that is not derivable from the current files alone."
            .to_string(),
        "Use memory_get to verify a specific memory hit before relying on it, and use memory_list when you need to browse the memory inventory before choosing what to read."
            .to_string(),
        "Use memory_record, memory_promote, and memory_forget intentionally when the user asks to remember something or when you are preserving a verified handoff-worthy fact outside the live session note. Compact-triggered working-memory snapshots are host-maintained separately."
            .to_string(),
        "Do not wait for host compaction before preserving important session state. When the current task gains handoff-worthy state that would matter after resume, interruption, or later follow-up, update the working session note with memory_update_session_note."
            .to_string(),
        "memory_update_session_note preserves omitted sections, so use it to refresh only the parts of the session note that actually changed."
            .to_string(),
        "Only update the session note after a material continuity change such as a plan pivot, a user correction or preference, a blocker or failed approach with reason, or a resume-critical next-step handoff."
            .to_string(),
        "Even after a material continuity change, write only when the existing session note is stale enough that resuming from it would mislead the next agent about the current plan, blocker, owner, or next step."
            .to_string(),
        "Do not update the session note for routine tool output, small incremental progress, or code edits that are already obvious from the current repository state and recent transcript."
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
    if let Some(skill_index) = build_skill_index(skill_catalog) {
        preamble.push(skill_index);
    }
    if let Some(memory_primer) = build_memory_primer(workspace_root) {
        preamble.push(memory_primer);
    }
    preamble.extend(plugin_instructions.iter().cloned());
    preamble
}

fn build_skill_index(skill_catalog: &agent::SkillCatalog) -> Option<String> {
    let skills = skill_catalog.all();
    if skills.is_empty() {
        return None;
    }
    let total_skill_count = skills.len();

    let mut rendered = skills
        .into_iter()
        .take(SKILL_INDEX_MAX_SKILLS)
        .map(|skill| render_skill_index_entry(&skill))
        .collect::<Vec<_>>();
    if total_skill_count > rendered.len() {
        rendered.push(format!(
            "- ... {remaining} more skill(s) omitted from the prompt index; use skills_list to browse the full catalog.",
            remaining = total_skill_count.saturating_sub(rendered.len())
        ));
    }

    Some(format!(
        "## Skills (mandatory)\n\
Before replying, scan this index. If a skill matches or is even partially relevant, load it with skill_view before continuing. Only proceed without loading a skill if genuinely none are relevant.\n\
After creating, patching, archiving, or restoring a skill, refresh your view with skills_list or skill_view before relying on the updated catalog.\n\n\
<available_skills>\n{}\n</available_skills>",
        rendered.join("\n")
    ))
}

fn render_skill_index_entry(skill: &agent::Skill) -> String {
    let mut parts = vec![format!(
        "- {}: {}",
        skill.name,
        truncate_inline(&skill.description, SKILL_INDEX_MAX_DESCRIPTION_CHARS)
    )];
    if !skill.aliases.is_empty() {
        parts.push(format!(
            "aliases: {}",
            truncate_list(&skill.aliases, SKILL_INDEX_MAX_METADATA_ITEMS)
        ));
    }
    if !skill.tags.is_empty() {
        parts.push(format!(
            "tags: {}",
            truncate_list(&skill.tags, SKILL_INDEX_MAX_METADATA_ITEMS)
        ));
    }
    parts.join(" | ")
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let truncated = trimmed
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    format!("{}...", truncated.trim_end())
}

fn truncate_list(values: &[String], max_items: usize) -> String {
    let visible = values
        .iter()
        .take(max_items)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let mut rendered = visible.join(", ");
    if values.len() > visible.len() {
        rendered.push_str(&format!(", +{}", values.len() - visible.len()));
    }
    rendered
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

pub fn resolve_skill_roots(
    configured_roots: &[PathBuf],
    workspace_root: &Path,
    plugin_plan: &agent::plugins::PluginActivationPlan,
) -> Vec<agent::SkillRoot> {
    let mut roots = vec![agent::SkillRoot::managed(
        AgentWorkspaceLayout::new(workspace_root).skills_dir(),
    )];
    roots.extend(user_skill_roots(configured_roots, workspace_root));
    push_if_exists(
        &mut roots,
        agent::SkillRoot::external(builtin_skill_root(workspace_root)),
    );
    roots.extend(
        plugin_plan
            .skill_roots
            .iter()
            .cloned()
            .map(agent::SkillRoot::external),
    );
    roots.retain(|root| root.kind == agent::SkillRootKind::Managed || root.path.exists());
    // Skill root order is policy, not presentation. Preserve the configured/default
    // precedence so the managed root wins over readonly external roots and plugin
    // roots extend that stack instead of reordering it lexicographically.
    let mut seen = BTreeSet::new();
    roots.retain(|root| seen.insert(root.path.clone()));
    roots
}

pub fn build_plugin_activation_plan(
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

fn user_skill_roots(configured_roots: &[PathBuf], workspace_root: &Path) -> Vec<agent::SkillRoot> {
    if configured_roots.is_empty() {
        default_external_skill_roots(workspace_root)
    } else {
        configured_roots
            .iter()
            .cloned()
            .map(agent::SkillRoot::external)
            .collect()
    }
}

fn default_external_skill_roots(workspace_root: &Path) -> Vec<agent::SkillRoot> {
    let mut roots = Vec::new();
    push_if_exists(
        &mut roots,
        agent::SkillRoot::external(workspace_root.join(".codex/skills")),
    );
    if let Some(home) = agent_env::home_dir() {
        push_if_exists(
            &mut roots,
            agent::SkillRoot::external(home.join(".codex/skills")),
        );
    }
    roots
}

fn push_if_exists(roots: &mut Vec<agent::SkillRoot>, root: agent::SkillRoot) {
    if root.path.exists() && !roots.iter().any(|candidate| candidate.path == root.path) {
        roots.push(root);
    }
}

#[cfg(test)]
mod tests {
    use super::{build_system_preamble, resolve_skill_roots};
    use agent::tools::HOST_FEATURE_REQUEST_USER_INPUT;
    use agent::types::ToolVisibilityContext;
    use agent::{AgentWorkspaceLayout, Skill, SkillCatalog, SkillProvenance, SkillRoot};
    use code_agent_config::{builtin_skill_root, materialize_builtin_skills};
    use nanoclaw_config::CoreConfig;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn sample_skill_catalog(workspace_root: &std::path::Path) -> SkillCatalog {
        SkillCatalog::new(vec![Skill {
            name: "release-smoke".to_string(),
            description:
                "Run the release smoke-test sequence after risky deployment or packaging changes."
                    .to_string(),
            aliases: vec!["smoke-release".to_string()],
            body: "Run smoke tests before rollout.".to_string(),
            root_dir: workspace_root.join(".nanoclaw/skills/release-smoke"),
            tags: vec!["release".to_string(), "smoke".to_string()],
            hooks: Vec::new(),
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            metadata: BTreeMap::new(),
            extension_metadata: BTreeMap::new(),
            activation: Default::default(),
            provenance: SkillProvenance {
                root: SkillRoot::managed(AgentWorkspaceLayout::new(workspace_root).skills_dir()),
                skill_dir: workspace_root.join(".nanoclaw/skills/release-smoke"),
                hub: None,
                shadowed_copies: Vec::new(),
            },
        }])
    }

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
        let skill_catalog = sample_skill_catalog(dir.path());

        let preamble = build_system_preamble(
            dir.path(),
            &profile,
            &skill_catalog,
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
        let skill_catalog = sample_skill_catalog(dir.path());

        let preamble = build_system_preamble(
            dir.path(),
            &profile,
            &skill_catalog,
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
        let skill_catalog = sample_skill_catalog(dir.path());

        let hidden = build_system_preamble(
            dir.path(),
            &profile,
            &skill_catalog,
            &[],
            &ToolVisibilityContext::default(),
        )
        .join("\n\n");
        let visible = build_system_preamble(
            dir.path(),
            &profile,
            &skill_catalog,
            &[],
            &ToolVisibilityContext::default().with_feature(HOST_FEATURE_REQUEST_USER_INPUT),
        )
        .join("\n\n");

        assert!(!hidden.contains("request_user_input"));
        assert!(visible.contains("request_user_input"));
    }

    #[test]
    fn system_preamble_encodes_tool_vs_skill_boundary() {
        let dir = tempdir().unwrap();
        let profile = CoreConfig::default().resolve_primary_agent().unwrap();
        let skill_catalog = sample_skill_catalog(dir.path());

        let preamble = build_system_preamble(
            dir.path(),
            &profile,
            &skill_catalog,
            &[],
            &ToolVisibilityContext::default(),
        )
        .join("\n\n");

        assert!(preamble.contains("## Skills (mandatory)"));
        assert!(preamble.contains("you MUST inspect it with skill_view"));
        assert!(preamble.contains("release-smoke"));
        assert!(preamble.contains("Use tool_discover when you need a typed runtime capability"));
        assert!(preamble.contains("save or update the approach with skill_manage"));
        assert!(preamble.contains("skill_manage(action='patch')"));
        assert!(
            preamble
                .contains("Do not assume the host will inject memory automatically on each turn.")
        );
        assert!(preamble.contains("Use memory_search when the task may depend on prior decisions"));
        assert!(preamble.contains(
            "Do not wait for host compaction before preserving important session state."
        ));
        assert!(preamble.contains("memory_update_session_note"));
        assert!(
            preamble.contains("Only update the session note after a material continuity change")
        );
        assert!(preamble.contains(
            "write only when the existing session note is stale enough that resuming from it would mislead"
        ));
        assert!(preamble.contains("Do not update the session note for routine tool output"));
    }

    #[test]
    fn resolve_skill_roots_keeps_managed_root_ahead_of_external_roots() {
        let dir = tempdir().unwrap();
        materialize_builtin_skills(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join(".codex/skills")).unwrap();
        std::fs::create_dir_all(dir.path().join(".nanoclaw/skills")).unwrap();
        let plugin_root = dir.path().join("plugin-skills");
        std::fs::create_dir_all(&plugin_root).unwrap();
        let plugin_plan = agent::plugins::PluginActivationPlan {
            skill_roots: vec![plugin_root.clone()],
            ..agent::plugins::PluginActivationPlan::default()
        };

        let roots = resolve_skill_roots(&[], dir.path(), &plugin_plan);

        assert_eq!(roots[0].kind, agent::SkillRootKind::Managed);
        assert_eq!(roots[0].path, dir.path().join(".nanoclaw/skills"));
        assert_eq!(roots[1].path, dir.path().join(".codex/skills"));
        assert!(
            roots
                .iter()
                .any(|root| root.path == builtin_skill_root(dir.path()))
        );
        assert!(roots.iter().any(|root| root.path == plugin_root));
    }

    #[test]
    fn resolve_skill_roots_keeps_managed_root_ahead_of_explicit_roots() {
        let dir = tempdir().unwrap();
        materialize_builtin_skills(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join(".nanoclaw/skills")).unwrap();
        let explicit = dir.path().join("custom-skills");
        std::fs::create_dir_all(&explicit).unwrap();

        let roots = resolve_skill_roots(
            std::slice::from_ref(&explicit),
            dir.path(),
            &agent::plugins::PluginActivationPlan::default(),
        );

        assert_eq!(roots[0].kind, agent::SkillRootKind::Managed);
        assert_eq!(roots[0].path, dir.path().join(".nanoclaw/skills"));
        assert_eq!(roots[1].path, explicit);
        assert!(
            roots
                .iter()
                .any(|root| root.path == builtin_skill_root(dir.path()))
        );
    }
}
