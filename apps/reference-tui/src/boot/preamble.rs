use crate::config::AgentCoreConfig;

pub(super) const DEFAULT_AGENT_PREAMBLE: &[&str] = &[
    "You are a general-purpose software agent operating inside the current workspace.",
    "Inspect available state and use tools before guessing. Treat tool results, approvals, and denials as authoritative runtime feedback.",
];

pub(super) fn build_runtime_preamble(
    config: &AgentCoreConfig,
    skill_catalog: &agent::skills::SkillCatalog,
    plugin_instructions: &[String],
) -> Vec<String> {
    let mut preamble = DEFAULT_AGENT_PREAMBLE
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    if let Some(system_prompt) = config.system_prompt.as_deref().map(str::trim) {
        if !system_prompt.is_empty() {
            preamble.push(system_prompt.to_string());
        }
    }
    preamble.extend(plugin_instructions.iter().cloned());
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}
