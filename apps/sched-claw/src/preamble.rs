use agent::SkillCatalog;
use nanoclaw_config::ResolvedAgentProfile;
use std::fs;
use std::path::Path;

const PROJECT_INSTRUCTIONS_PATH: &str = "AGENTS.md";
const MAX_PRIMER_LINES: usize = 120;
const MAX_PRIMER_BYTES: usize = 12_000;
const MAX_SKILLS_IN_INDEX: usize = 32;
const MAX_DESCRIPTION_CHARS: usize = 140;

pub fn build_system_preamble(
    workspace_root: &Path,
    profile: &ResolvedAgentProfile,
    skill_catalog: &SkillCatalog,
) -> Vec<String> {
    let mut preamble = vec![
        "You are sched-claw, a Linux scheduling agent operating inside the current workspace."
            .to_string(),
        "Your job is to collect evidence, explain scheduler behavior, implement sched-ext policy code, and use the privileged daemon only for scheduler lifecycle actions."
            .to_string(),
        "Keep the host surface minimal. Use existing file, shell, and web tools before asking for new runtime capabilities."
            .to_string(),
        "Performance data collection is not a dedicated tool. Use normal shell commands, save the exact evidence you rely on, and load the relevant skill before inventing a measurement workflow."
            .to_string(),
        "Before collecting or interpreting Linux performance data, inspect the skill index below and load the relevant skill with skill_view. Err on the side of loading."
            .to_string(),
        "When the task is host setup, dependency validation, or operator readiness, use the product-readiness skill and the local `sched-claw doctor` surface instead of guessing prerequisites."
            .to_string(),
        "Keep workflows skill-driven, not host-driven. Use the active skill SOP to decide which experiment, template, build, and rollout steps matter for the current workload."
            .to_string(),
        "When a task is workload-driven, use the sched-claw experiment substrate whenever it helps keep workload contracts, candidates, baseline runs, deployments, and scores structured instead of living only in transcript text."
            .to_string(),
        "When defining a workload contract, keep the target selector explicit: script, pid, uid, gid, or cgroup."
            .to_string(),
        "Prefer direct throughput or latency metrics when they exist. If they do not, record the proxy basis explicitly, for example IPC or CPI, instead of pretending the metric is direct."
            .to_string(),
        "Use the local template catalog and materialization commands when you need concrete sched-ext source scaffolding, but do not treat template selection as a fixed workflow."
            .to_string(),
        "Use sched_ext_daemon only for status, activate, stop, and logs. Do not use it as a generic privileged execution escape hatch."
            .to_string(),
        "When you generate a new sched-ext scheduler, keep the rollout loop explicit: baseline evidence, code change, privileged activation, verification, and rollback criteria."
            .to_string(),
        "The host provides generic local commands such as experiment materialize, experiment build, experiment run, experiment deploy, template list/show, and experiment score. Choose among them based on the current evidence and skill guidance."
            .to_string(),
    ];
    if let Some(system_prompt) = profile
        .global_system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        preamble.push(system_prompt.to_string());
    }
    if let Some(system_prompt) = profile
        .system_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        preamble.push(system_prompt.to_string());
    }
    if let Some(skill_index) = build_skill_index(skill_catalog) {
        preamble.push(skill_index);
    }
    if let Some(project_primer) = load_project_primer(workspace_root) {
        preamble.push(project_primer);
    }
    preamble
}

fn build_skill_index(skill_catalog: &SkillCatalog) -> Option<String> {
    let skills = skill_catalog.all();
    if skills.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for skill in skills.into_iter().take(MAX_SKILLS_IN_INDEX) {
        let mut line = format!(
            "- {}: {}",
            skill.name,
            truncate_inline(&skill.description, MAX_DESCRIPTION_CHARS)
        );
        if !skill.aliases.is_empty() {
            line.push_str(&format!(" | aliases: {}", skill.aliases.join(", ")));
        }
        if !skill.tags.is_empty() {
            line.push_str(&format!(" | tags: {}", skill.tags.join(", ")));
        }
        lines.push(line);
    }
    Some(format!(
        "## Skills (mandatory)\nBefore replying, scan this index. If a skill matches the task or any part of the task, load it with skill_view before acting.\n\n<available_skills>\n{}\n</available_skills>",
        lines.join("\n")
    ))
}

fn load_project_primer(workspace_root: &Path) -> Option<String> {
    let path = workspace_root.join(PROJECT_INSTRUCTIONS_PATH);
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = truncate_content(raw.trim(), MAX_PRIMER_LINES, MAX_PRIMER_BYTES);
    (!trimmed.is_empty()).then(|| {
        format!(
            "## Project Instructions ({PROJECT_INSTRUCTIONS_PATH})\nUse these repository instructions as host policy. Verify code facts against the live workspace before acting.\n\n{}",
            trimmed
        )
    })
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.trim().to_string();
    }
    let prefix = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    format!("{}...", prefix.trim_end())
}

fn truncate_content(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let mut lines = text
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if lines.len() > max_bytes {
        lines.truncate(max_bytes.saturating_sub(3));
        lines.push_str("...");
    }
    lines
}
