use crate::frontmatter::SkillFrontmatter;
use crate::{Result, SkillError};
use futures::{StreamExt, TryStreamExt, stream};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs;

const SKILL_LOAD_CONCURRENCY_LIMIT: usize = 8;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub body: String,
    pub root_dir: PathBuf,
    pub tags: Vec<String>,
    pub hooks: Vec<types::HookRegistration>,
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
    let skill_toml_path = dir.join("skill.toml");
    let raw = fs::read_to_string(&skill_path)
        .await
        .map_err(|source| SkillError::read_path(skill_path.display().to_string(), source))?;
    let (frontmatter, body) = if skill_toml_path.exists() {
        let raw_toml = fs::read_to_string(&skill_toml_path)
            .await
            .map_err(|source| {
                SkillError::read_path(skill_toml_path.display().to_string(), source)
            })?;
        // During migration, a skill may provide `skill.toml` while still carrying
        // legacy YAML frontmatter in `SKILL.md`. Keep TOML as the metadata source
        // of truth but strip optional frontmatter from the instruction body.
        (
            parse_skill_toml(&raw_toml)?,
            strip_optional_frontmatter(&raw).to_string(),
        )
    } else {
        parse_frontmatter(&raw)?
    };
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
    let mut skill_dirs = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        let mut entries = fs::read_dir(root)
            .await
            .map_err(|source| SkillError::read_path(root.display().to_string(), source))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() && path.join("SKILL.md").exists() {
                skill_dirs.push(path);
            }
        }
    }
    skill_dirs.sort();
    skill_dirs.dedup();

    let tasks = skill_dirs
        .into_iter()
        .enumerate()
        .map(|(index, path)| async move {
            let skill = load_skill_from_dir(&path).await?;
            Ok::<_, SkillError>((index, skill))
        })
        .collect::<Vec<_>>();
    let skills = run_indexed_tasks_ordered(tasks, SKILL_LOAD_CONCURRENCY_LIMIT).await?;
    Ok(crate::SkillCatalog::new(skills))
}

async fn run_indexed_tasks_ordered<T, E, Fut>(
    tasks: Vec<Fut>,
    concurrency_limit: usize,
) -> std::result::Result<Vec<T>, E>
where
    Fut: std::future::Future<Output = std::result::Result<(usize, T), E>>,
{
    // Skill packages often touch many files (frontmatter, references, scripts).
    // Bounded parallel loading removes obvious serialization while avoiding a
    // large number of simultaneous filesystem operations.
    let mut indexed = stream::iter(tasks)
        .buffer_unordered(concurrency_limit.max(1))
        .try_collect::<Vec<_>>()
        .await?;

    // Skill selection and manifest generation should remain deterministic
    // regardless of filesystem traversal order or task completion timing.
    indexed.sort_by_key(|(index, _)| *index);
    Ok(indexed.into_iter().map(|(_, value)| value).collect())
}

fn parse_frontmatter(raw: &str) -> Result<(SkillFrontmatter, String)> {
    let re = Regex::new(r"(?s)\A---\n(.*?)\n---\n?(.*)\z").expect("frontmatter regex");
    let captures = re
        .captures(raw)
        .ok_or_else(|| SkillError::invalid_format("skill file is missing YAML frontmatter"))?;
    let frontmatter = captures
        .get(1)
        .map(|value| value.as_str())
        .ok_or_else(|| SkillError::invalid_format("missing frontmatter body"))?;
    let body = captures
        .get(2)
        .map(|value| value.as_str())
        .ok_or_else(|| SkillError::invalid_format("missing skill body"))?;
    let parsed: SkillFrontmatter = serde_yaml::from_str(frontmatter)?;
    if parsed.name.trim().is_empty() || parsed.description.trim().is_empty() {
        return Err(SkillError::invalid_format(
            "skill frontmatter requires non-empty name and description",
        ));
    }
    Ok((parsed, body.to_string()))
}

fn parse_skill_toml(raw: &str) -> Result<SkillFrontmatter> {
    let parsed: SkillFrontmatter = toml::from_str(raw)?;
    validate_required_skill_fields(&parsed)?;
    Ok(parsed)
}

fn strip_optional_frontmatter(raw: &str) -> &str {
    let re = Regex::new(r"(?s)\A---\n(.*?)\n---\n?(.*)\z").expect("frontmatter regex");
    re.captures(raw)
        .and_then(|captures| captures.get(2).map(|value| value.as_str()))
        .unwrap_or(raw)
}

fn validate_required_skill_fields(frontmatter: &SkillFrontmatter) -> Result<()> {
    if frontmatter.name.trim().is_empty() || frontmatter.description.trim().is_empty() {
        return Err(SkillError::invalid_format(
            "skill metadata requires non-empty name and description",
        ));
    }
    Ok(())
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
    use super::{load_skill_from_dir, load_skill_roots, run_indexed_tasks_ordered};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::{Duration, sleep};

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

    #[tokio::test]
    async fn indexed_runner_is_ordered_and_bounded() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks = (0usize..10)
            .map(|index| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    update_peak(&peak, now);
                    sleep(Duration::from_millis((10 - index) as u64)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, ()>((index, index))
                }
            })
            .collect::<Vec<_>>();

        let output = run_indexed_tasks_ordered(tasks, 2).await.unwrap();
        assert_eq!(output, (0usize..10).collect::<Vec<_>>());
        assert!(peak.load(Ordering::SeqCst) <= 2);
    }

    #[tokio::test]
    async fn skill_toml_takes_precedence_over_yaml_frontmatter() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("review");
        fs::create_dir_all(&skill_dir).await.unwrap();

        fs::write(
            skill_dir.join("skill.toml"),
            r#"
                name = "review"
                description = "Use for review tasks"
                aliases = ["rvw"]
            "#,
        )
        .await
        .unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: wrong-name
description: wrong description
---

Use for high-signal reviews.
"#,
        )
        .await
        .unwrap();

        let skill = load_skill_from_dir(&skill_dir).await.unwrap();
        assert_eq!(skill.name, "review");
        assert_eq!(skill.description, "Use for review tasks");
        assert_eq!(skill.aliases, vec!["rvw".to_string()]);
        assert!(skill.body.contains("Use for high-signal reviews."));
        assert!(!skill.body.contains("wrong-name"));
    }

    #[tokio::test]
    async fn falls_back_to_yaml_frontmatter_when_skill_toml_is_missing() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("fallback");
        fs::create_dir_all(&skill_dir).await.unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: fallback
description: legacy metadata source
aliases: [old]
---

Legacy body.
"#,
        )
        .await
        .unwrap();

        let skill = load_skill_from_dir(&skill_dir).await.unwrap();
        assert_eq!(skill.name, "fallback");
        assert_eq!(skill.description, "legacy metadata source");
        assert_eq!(skill.aliases, vec!["old".to_string()]);
        assert!(skill.body.contains("Legacy body."));
    }

    fn update_peak(peak: &AtomicUsize, candidate: usize) {
        let mut current = peak.load(Ordering::SeqCst);
        while candidate > current {
            match peak.compare_exchange(current, candidate, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }
}
