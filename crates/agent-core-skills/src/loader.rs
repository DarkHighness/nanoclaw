use crate::frontmatter::SkillFrontmatter;
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub body: String,
    pub root_dir: PathBuf,
    pub tags: Vec<String>,
    pub hooks: Vec<agent_core_types::HookRegistration>,
    pub references: Vec<PathBuf>,
    pub scripts: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
    pub metadata: BTreeMap<String, serde_yaml::Value>,
    pub extension_metadata: BTreeMap<String, serde_yaml::Value>,
}

impl Skill {
    #[must_use]
    pub fn system_instruction(&self) -> String {
        self.body.trim().to_string()
    }
}

pub async fn load_skill_from_dir(dir: impl AsRef<Path>) -> Result<Skill> {
    let dir = dir.as_ref();
    let skill_path = dir.join("SKILL.md");
    let raw = fs::read_to_string(&skill_path)
        .await
        .with_context(|| format!("failed to read {}", skill_path.display()))?;
    let (frontmatter, body) = parse_frontmatter(&raw)?;
    Ok(Skill {
        name: frontmatter.name,
        description: frontmatter.description,
        aliases: frontmatter.aliases,
        body,
        root_dir: dir.to_path_buf(),
        tags: frontmatter.tags,
        hooks: frontmatter.agent_core.hooks,
        references: collect_child_paths(dir.join("references")).await?,
        scripts: collect_child_paths(dir.join("scripts")).await?,
        assets: collect_child_paths(dir.join("assets")).await?,
        metadata: frontmatter.extra,
        extension_metadata: frontmatter.agent_core.extra,
    })
}

pub async fn load_skill_roots(roots: &[PathBuf]) -> Result<crate::SkillCatalog> {
    let mut skills = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        let mut entries = fs::read_dir(root)
            .await
            .with_context(|| format!("failed to read skill root {}", root.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() && path.join("SKILL.md").exists() {
                skills.push(load_skill_from_dir(&path).await?);
            }
        }
    }
    Ok(crate::SkillCatalog::new(skills))
}

fn parse_frontmatter(raw: &str) -> Result<(SkillFrontmatter, String)> {
    let re = Regex::new(r"(?s)\A---\n(.*?)\n---\n?(.*)\z").expect("frontmatter regex");
    let captures = re
        .captures(raw)
        .ok_or_else(|| anyhow::anyhow!("skill file is missing YAML frontmatter"))?;
    let frontmatter = captures
        .get(1)
        .map(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing frontmatter body"))?;
    let body = captures
        .get(2)
        .map(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing skill body"))?;
    let parsed: SkillFrontmatter = serde_yaml::from_str(frontmatter)?;
    if parsed.name.trim().is_empty() || parsed.description.trim().is_empty() {
        bail!("skill frontmatter requires non-empty name and description");
    }
    Ok((parsed, body.to_string()))
}

async fn collect_child_paths(dir: PathBuf) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    let mut entries = fs::read_dir(&dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::{load_skill_from_dir, load_skill_roots};
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn loads_standard_skill_layout() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("pdf");
        fs::create_dir_all(skill_dir.join("references"))
            .await
            .unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: pdf
description: Use for PDF tasks
aliases: [acrobat]
tags: [document]
x-agent-core:
  hooks: []
---

# PDF

Do PDF things.
"#,
        )
        .await
        .unwrap();
        fs::write(skill_dir.join("references").join("guide.md"), "guide")
            .await
            .unwrap();

        let skill = load_skill_from_dir(&skill_dir).await.unwrap();
        assert_eq!(skill.name, "pdf");
        assert_eq!(skill.aliases, vec!["acrobat".to_string()]);
        assert_eq!(skill.references.len(), 1);
        assert!(skill.system_instruction().contains("Do PDF things."));
    }

    #[tokio::test]
    async fn registry_builds_stable_prompt_manifest() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("pdf");
        fs::create_dir_all(&skill_dir).await.unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: pdf
description: Use for PDF tasks
aliases: [acrobat]
---

Use for PDF work.
"#,
        )
        .await
        .unwrap();
        let registry = load_skill_roots(&[dir.path().join("skills")])
            .await
            .unwrap();
        let manifest = registry.prompt_manifest().unwrap();
        assert!(manifest.contains("Available workspace skills are listed below."));
        assert!(manifest.contains("- pdf: Use for PDF tasks"));
        assert!(manifest.contains("aliases=acrobat"));
        assert!(manifest.contains("path="));
    }
}
