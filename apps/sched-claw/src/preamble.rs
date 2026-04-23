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
        "Your job is to investigate scheduler behavior, collect evidence, revise sched-ext policy code, and use the privileged daemon only for scheduler lifecycle actions."
            .to_string(),
        "Keep the host surface minimal. Use existing file, shell, and web tools before asking for new runtime capabilities."
            .to_string(),
        "Performance data collection, analysis, plotting, and code generation are skill-driven. Prefer repository-embedded skills and their scripts over host-owned workflows."
            .to_string(),
        "Before collecting or interpreting Linux performance data, inspect the skill index below and load the relevant skill with skill_view. Err on the side of loading."
            .to_string(),
        "Use standard development tools directly. Repository scripts, uv-managed helper environments, perf collectors, pandas or polars analysis, and matplotlib plots belong in skills and local scripts, not in the host runtime."
            .to_string(),
        "When the task is host setup, dependency validation, or operator readiness, use the product-readiness skill and the local `sched-claw doctor` surface instead of guessing prerequisites."
            .to_string(),
        "Keep workflows skill-driven, not host-driven. The host should not dictate a fixed measurement, scoring, or anomaly-detection method."
            .to_string(),
        "When collection or analysis needs deterministic automation, keep it in scripts or hook-like helpers that the agent can inspect and call explicitly."
            .to_string(),
        "Keep evidence legible in workspace files. Persist raw captures, reduced tables, plots, and code review notes as normal artifacts instead of burying them in transcript prose."
            .to_string(),
        "When comparing alternatives, choose the reduction and outlier methods that fit the workload and explain them explicitly. Do not assume one host-provided scorer is always correct."
            .to_string(),
        "If a skill ships helper scripts, inspect them before use and prefer reusing them over retyping large shell pipelines."
            .to_string(),
        "Use sched_ext_daemon only for status, activate, stop, and logs. Do not use it as a generic privileged execution escape hatch."
            .to_string(),
        "Do not use the daemon as a generic privileged shell. Build, collect, analyze, and edit through normal tools; reserve privilege for rollout lifecycle control."
            .to_string(),
        "When defining a workload or rollout target, keep the selector explicit: script, pid, uid, gid, or cgroup."
            .to_string(),
        "Prefer direct throughput or latency metrics when they exist. If they do not, state the proxy basis explicitly, for example IPC or CPI, instead of pretending the metric is direct."
            .to_string(),
        "When a rollout window must be bounded, use an explicit daemon lease instead of assuming the client will always remember to stop the deployment."
            .to_string(),
        "When you generate a new sched-ext scheduler, keep the rollout loop explicit: evidence, code change, privileged activation, verification, and rollback criteria."
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
