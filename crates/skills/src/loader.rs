use crate::frontmatter::SkillFrontmatter;
use crate::{Result, Skill, SkillActivation, SkillCatalog, SkillError, SkillProvenance, SkillRoot};
use futures::{StreamExt, TryStreamExt, stream};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tokio::fs;

const SKILL_LOAD_CONCURRENCY_LIMIT: usize = 8;

pub async fn load_skill_from_dir(dir: impl AsRef<Path>, root: &SkillRoot) -> Result<Skill> {
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
        activation: SkillActivation {
            platforms: frontmatter.platforms,
            requires_tools: frontmatter.requires_tools,
            fallback_for_tools: frontmatter.fallback_for_tools,
        },
        provenance: SkillProvenance {
            root: root.clone(),
            skill_dir: dir.to_path_buf(),
        },
    })
}

pub async fn load_skill_roots(roots: &[SkillRoot]) -> Result<SkillCatalog> {
    let mut skill_dirs = Vec::new();
    let mut seen_dirs = BTreeSet::new();
    for (root_index, root) in roots.iter().enumerate() {
        if !root.path.exists() {
            continue;
        }
        let mut entries = fs::read_dir(&root.path)
            .await
            .map_err(|source| SkillError::read_path(root.path.display().to_string(), source))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir()
                && path.join("SKILL.md").exists()
                && seen_dirs.insert(path.clone())
            {
                skill_dirs.push((root_index, root.clone(), path));
            }
        }
    }
    // Root order carries precedence semantics: managed/local roots should win over
    // readonly external roots, and configured/plugin roots should keep the order
    // chosen by the host instead of drifting into lexical path order.
    skill_dirs.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.2.cmp(&right.2)));

    let tasks = skill_dirs
        .into_iter()
        .enumerate()
        .map(|(index, (_, root, path))| async move {
            let skill = load_skill_from_dir(&path, &root).await?;
            Ok::<_, SkillError>((index, skill))
        })
        .collect::<Vec<_>>();
    let loaded = run_indexed_tasks_ordered(tasks, SKILL_LOAD_CONCURRENCY_LIMIT).await?;
    let mut seen_names = BTreeMap::new();
    let mut skills = Vec::new();
    for skill in loaded {
        if seen_names.contains_key(&skill.name) {
            continue;
        }
        seen_names.insert(skill.name.clone(), ());
        skills.push(skill);
    }
    Ok(SkillCatalog::from_parts(roots.to_vec(), skills))
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
    use crate::SkillRoot;
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

        let skill = load_skill_from_dir(&skill_dir, &SkillRoot::managed(dir.path().join("skills")))
            .await
            .unwrap();
        assert_eq!(skill.name, "pdf");
        assert_eq!(skill.aliases, vec!["acrobat".to_string()]);
        assert_eq!(skill.references.len(), 1);
        assert!(skill.system_instruction().contains("Do PDF things."));
    }

    #[tokio::test]
    async fn registry_resolves_aliases_from_managed_root() {
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
        let registry = load_skill_roots(&[SkillRoot::managed(dir.path().join("skills"))])
            .await
            .unwrap();
        let skill = registry.resolve("acrobat").unwrap();
        assert_eq!(skill.name, "pdf");
        assert!(skill.provenance.root.writable());
    }

    #[tokio::test]
    async fn managed_root_wins_when_skill_names_overlap_external_roots() {
        let dir = tempdir().unwrap();
        let managed_root = dir.path().join("managed");
        let external_root = dir.path().join("external");
        let managed_skill = managed_root.join("review");
        let external_skill = external_root.join("review");
        fs::create_dir_all(&managed_skill).await.unwrap();
        fs::create_dir_all(&external_skill).await.unwrap();
        fs::write(
            managed_skill.join("SKILL.md"),
            r#"---
name: review
description: Managed review skill
---

Use the managed root version.
"#,
        )
        .await
        .unwrap();
        fs::write(
            external_skill.join("SKILL.md"),
            r#"---
name: review
description: External review skill
---

Use the external root version.
"#,
        )
        .await
        .unwrap();

        let registry = load_skill_roots(&[
            SkillRoot::managed(managed_root),
            SkillRoot::external(external_root),
        ])
        .await
        .unwrap();

        let skill = registry.resolve("review").unwrap();
        assert_eq!(skill.description, "Managed review skill");
        assert!(skill.body.contains("managed root version"));
        assert!(skill.provenance.root.writable());
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

        let skill = load_skill_from_dir(&skill_dir, &SkillRoot::managed(dir.path().join("skills")))
            .await
            .unwrap();
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

        let skill = load_skill_from_dir(&skill_dir, &SkillRoot::managed(dir.path().join("skills")))
            .await
            .unwrap();
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
