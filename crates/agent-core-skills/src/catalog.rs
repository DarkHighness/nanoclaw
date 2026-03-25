use crate::Skill;

#[derive(Clone, Debug, Default)]
pub struct SkillCatalog {
    skills: Vec<Skill>,
}

impl SkillCatalog {
    #[must_use]
    pub fn new(mut skills: Vec<Skill>) -> Self {
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        Self { skills }
    }

    #[must_use]
    pub fn all(&self) -> &[Skill] {
        &self.skills
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|skill| skill.name == name)
    }

    #[must_use]
    pub fn prompt_manifest(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }

        let mut lines = vec![
            "Available workspace skills are listed below.".to_string(),
            "Do not rely on heuristic skill activation. If a skill is relevant, inspect its SKILL.md with the read tool and then follow it explicitly.".to_string(),
            "Loaded skills:".to_string(),
        ];
        lines.extend(self.skills.iter().map(format_skill_manifest_line));
        Some(lines.join("\n"))
    }
}

fn format_skill_manifest_line(skill: &Skill) -> String {
    let skill_path = skill.root_dir.join("SKILL.md");
    let aliases = if skill.aliases.is_empty() {
        String::new()
    } else {
        format!(" aliases={}", skill.aliases.join(","))
    };
    let tags = if skill.tags.is_empty() {
        String::new()
    } else {
        format!(" tags={}", skill.tags.join(","))
    };
    format!(
        "- {}:{}{} path={}",
        skill.name,
        if skill.description.is_empty() {
            " no description".to_string()
        } else {
            format!(" {}", skill.description)
        },
        format!("{aliases}{tags}"),
        skill_path.display()
    )
}
