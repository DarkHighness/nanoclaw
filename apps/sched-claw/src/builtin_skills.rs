use crate::app_config::app_state_dir;
use anyhow::{Context, Result, anyhow};
use include_dir::{Dir, DirEntry, include_dir};
use std::path::{Path, PathBuf};

static BUILTIN_SKILLS_SOURCE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills");

pub fn builtin_skill_root(workspace_root: &Path) -> PathBuf {
    app_state_dir(workspace_root).join("builtin-skills")
}

pub fn materialize_builtin_skills(workspace_root: &Path) -> Result<PathBuf> {
    let root = builtin_skill_root(workspace_root);
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("failed to reset {}", root.display()))?;
    }
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create {}", root.display()))?;
    write_embedded_skill_dir(&BUILTIN_SKILLS_SOURCE, &root)?;
    Ok(root)
}

fn write_embedded_skill_dir(dir: &Dir<'_>, destination: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(child) => {
                let child_destination = destination.join(
                    child
                        .path()
                        .file_name()
                        .ok_or_else(|| anyhow!("missing embedded skill directory name"))?,
                );
                std::fs::create_dir_all(&child_destination)
                    .with_context(|| format!("failed to create {}", child_destination.display()))?;
                write_embedded_skill_dir(child, &child_destination)?;
            }
            DirEntry::File(file) => {
                let file_destination = destination.join(
                    file.path()
                        .file_name()
                        .ok_or_else(|| anyhow!("missing embedded skill file name"))?,
                );
                std::fs::write(&file_destination, file.contents())
                    .with_context(|| format!("failed to write {}", file_destination.display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{builtin_skill_root, materialize_builtin_skills};
    use tempfile::tempdir;

    #[test]
    fn materializes_embedded_skill_bundle() {
        let dir = tempdir().unwrap();
        let root = materialize_builtin_skills(dir.path()).unwrap();

        assert_eq!(root, builtin_skill_root(dir.path()));
        assert!(root.join("linux-scheduler-triage/SKILL.md").is_file());
        assert!(root.join("sched-ext-design-loop/SKILL.md").is_file());
    }
}
