use super::super::{
    RuntimeTui, TuiState, format_skill_line, format_skill_sidebar, format_tool_line,
};
use crate::TuiCommand;
use agent::skills::Skill;

impl RuntimeTui {
    pub(in crate::app) async fn apply_catalog_command(
        &mut self,
        command: TuiCommand,
        state: &mut TuiState,
    ) -> anyhow::Result<bool> {
        match command {
            TuiCommand::Skills { query } => {
                let skills = filter_skills(&self.skills, query.as_deref());
                state.sidebar = if skills.is_empty() {
                    vec!["no skills matched".to_string()]
                } else {
                    skills
                        .iter()
                        .take(16)
                        .map(|skill| format_skill_line(skill))
                        .collect()
                };
                state.sidebar_title = "Skills".to_string();
                state.status = if let Some(query) = query {
                    if skills.is_empty() {
                        format!("No skills matched `{query}`")
                    } else {
                        format!(
                            "Listed {} matching skills. Use {}skill <name> for details.",
                            skills.len(),
                            self.command_prefix
                        )
                    }
                } else if skills.is_empty() {
                    "No skills loaded".to_string()
                } else {
                    format!(
                        "Listed {} skills. Use {}skill <name> for details.",
                        skills.len(),
                        self.command_prefix
                    )
                };
                Ok(false)
            }
            TuiCommand::Skill { skill_name } => {
                let skill = resolve_skill_reference(&self.skills, &skill_name)?;
                state.sidebar = format_skill_sidebar(skill);
                state.sidebar_title = "Skill".to_string();
                state.status = format!("Loaded skill {}", skill.name);
                Ok(false)
            }
            TuiCommand::Tools => {
                state.sidebar = self.runtime_tools().iter().map(format_tool_line).collect();
                state.sidebar_title = "Tools".to_string();
                state.status = "Listed tools".to_string();
                Ok(false)
            }
            TuiCommand::Hooks => {
                state.sidebar = vec![
                    "Claude-style hooks enabled".to_string(),
                    "SessionStart".to_string(),
                    "UserPromptSubmit".to_string(),
                    "PreToolUse/PostToolUse".to_string(),
                    "Stop/SessionEnd".to_string(),
                ];
                state.sidebar_title = "Hooks".to_string();
                state.status = "Listed hooks".to_string();
                Ok(false)
            }
            _ => unreachable!("catalog handler received non-catalog command"),
        }
    }
}

fn filter_skills<'a>(skills: &'a [Skill], query: Option<&str>) -> Vec<&'a Skill> {
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return skills.iter().collect();
    };
    let query = query.to_lowercase();
    skills
        .iter()
        .filter(|skill| {
            skill.name.to_lowercase().contains(&query)
                || skill.description.to_lowercase().contains(&query)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.to_lowercase().contains(&query))
                || skill
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&query))
        })
        .collect()
}

fn resolve_skill_reference<'a>(skills: &'a [Skill], skill_ref: &str) -> anyhow::Result<&'a Skill> {
    if let Some(skill) = skills.iter().find(|skill| skill.name == skill_ref) {
        return Ok(skill);
    }
    if let Some(skill) = skills
        .iter()
        .find(|skill| skill.aliases.iter().any(|alias| alias == skill_ref))
    {
        return Ok(skill);
    }

    let matches = skills
        .iter()
        .filter(|skill| {
            skill.name.starts_with(skill_ref)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.starts_with(skill_ref))
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!("unknown skill: {skill_ref}")),
        [skill] => Ok(skill),
        _ => Err(anyhow::anyhow!(
            "ambiguous skill reference {skill_ref}: {}",
            matches
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_skill_reference;
    use agent::skills::Skill;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn resolves_skill_by_alias() {
        let skills = vec![Skill {
            name: "pdf".to_string(),
            description: "Use for PDF tasks".to_string(),
            aliases: vec!["acrobat".to_string()],
            body: "Do PDF things.".to_string(),
            root_dir: PathBuf::from("/tmp/pdf"),
            tags: vec!["document".to_string()],
            hooks: Vec::new(),
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            metadata: BTreeMap::new(),
            extension_metadata: BTreeMap::new(),
        }];

        let resolved = resolve_skill_reference(&skills, "acrobat").unwrap();
        assert_eq!(resolved.name, "pdf");
    }
}
