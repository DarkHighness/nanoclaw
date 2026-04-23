use agent::{Skill, SkillCatalog, ToolSpec};

#[derive(Clone, Debug, Default)]
pub struct StartupCatalog {
    tool_specs: Vec<ToolSpec>,
    skills: Vec<Skill>,
}

impl StartupCatalog {
    #[must_use]
    pub fn new(mut tool_specs: Vec<ToolSpec>, mut skills: Vec<Skill>) -> Self {
        tool_specs.sort_by(|left, right| left.name.cmp(&right.name));
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        Self { tool_specs, skills }
    }

    #[must_use]
    pub fn from_parts(tool_specs: Vec<ToolSpec>, skill_catalog: &SkillCatalog) -> Self {
        Self::new(tool_specs, skill_catalog.all())
    }

    #[must_use]
    pub fn tool_specs(&self) -> &[ToolSpec] {
        &self.tool_specs
    }

    #[must_use]
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_specs
            .iter()
            .map(|spec| spec.name.to_string())
            .collect()
    }

    #[must_use]
    pub fn resolve_tool(&self, query: &str) -> Option<&ToolSpec> {
        let normalized = query.trim();
        if normalized.is_empty() {
            return None;
        }
        self.tool_specs.iter().find(|spec| {
            spec.name.as_str() == normalized
                || spec
                    .aliases
                    .iter()
                    .any(|alias| alias.as_str() == normalized)
        })
    }

    #[must_use]
    pub fn resolve_skill(&self, query: &str) -> Option<&Skill> {
        let normalized = query.trim();
        if normalized.is_empty() {
            return None;
        }
        self.skills.iter().find(|skill| {
            skill.name == normalized
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.as_str() == normalized)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::StartupCatalog;
    use agent::types::{ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
    use agent::{Skill, SkillCatalog, SkillProvenance, SkillRoot, SkillRootKind};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn fixture_skill(name: &str, alias: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: "fixture".to_string(),
            aliases: vec![alias.to_string()],
            body: "body".to_string(),
            root_dir: PathBuf::from("/tmp/skills"),
            tags: vec!["linux".to_string()],
            hooks: Vec::new(),
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            metadata: BTreeMap::new(),
            extension_metadata: BTreeMap::new(),
            activation: Default::default(),
            provenance: SkillProvenance {
                root: SkillRoot {
                    path: PathBuf::from("/tmp/skills"),
                    kind: SkillRootKind::External,
                },
                skill_dir: PathBuf::from(format!("/tmp/skills/{name}")),
                hub: None,
                shadowed_copies: Vec::new(),
            },
        }
    }

    fn fixture_tool(name: &str, alias: &str) -> ToolSpec {
        ToolSpec::function(
            name,
            "fixture",
            json!({"type":"object","properties":{}}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_aliases(vec![alias.into()])
        .with_defer_loading(false)
        .with_parallel_support(false)
    }

    #[test]
    fn resolves_tools_and_skills_by_alias() {
        let catalog = StartupCatalog::from_parts(
            vec![fixture_tool("sched_claw_daemon", "daemon")],
            &SkillCatalog::new(vec![fixture_skill("sched-ext-design-loop", "sched-loop")]),
        );

        assert_eq!(
            catalog
                .resolve_tool("daemon")
                .map(|spec| spec.name.as_str()),
            Some("sched_claw_daemon")
        );
        assert_eq!(
            catalog
                .resolve_skill("sched-loop")
                .map(|skill| skill.name.as_str()),
            Some("sched-ext-design-loop")
        );
    }

    #[test]
    fn returns_sorted_tool_names() {
        let catalog = StartupCatalog::from_parts(
            vec![fixture_tool("write", "w"), fixture_tool("edit", "e")],
            &SkillCatalog::new(Vec::new()),
        );

        assert_eq!(catalog.tool_names(), vec!["edit", "write"]);
    }
}
