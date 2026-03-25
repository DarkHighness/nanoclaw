use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};

pub fn resolve_tool_path_against_workspace_root(
    file_path: &str,
    root: &Path,
    container_workdir: Option<&str>,
) -> Result<PathBuf> {
    let mapped = map_container_path_to_workspace_root(file_path, root, container_workdir)?;
    Ok(if mapped.is_absolute() {
        mapped
    } else {
        root.join(mapped)
    })
}

pub fn map_container_path_to_workspace_root(
    file_path: &str,
    root: &Path,
    container_workdir: Option<&str>,
) -> Result<PathBuf> {
    let mut candidate = file_path.trim();
    if let Some(stripped) = candidate.strip_prefix('@') {
        candidate = stripped;
    }
    if let Some(stripped) = candidate.strip_prefix("file://") {
        candidate = stripped;
    }

    let path = PathBuf::from(candidate);
    if let Some(workdir) = container_workdir {
        let workdir = PathBuf::from(workdir);
        if path.is_absolute() && path.starts_with(&workdir) {
            if let Ok(relative) = path.strip_prefix(&workdir) {
                return Ok(root.join(relative));
            }
        }
    }
    Ok(path)
}

pub fn assert_path_inside_root(path: &Path, root: &Path) -> Result<()> {
    let normalized_root = normalize_for_prefix(root)?;
    let normalized_path = normalize_for_prefix(path)?;
    if normalized_path.starts_with(&normalized_root) {
        Ok(())
    } else {
        bail!(
            "Path escapes workspace root: {} is outside {}",
            normalized_path.display(),
            normalized_root.display()
        )
    }
}

pub fn assert_path_inside_allowed_roots<'a, I>(path: &Path, roots: I) -> Result<()>
where
    I: IntoIterator<Item = &'a Path>,
{
    let normalized_path = normalize_for_prefix(path)?;
    let normalized_roots = roots
        .into_iter()
        .map(normalize_for_prefix)
        .collect::<Result<Vec<_>>>()?;
    if normalized_roots
        .iter()
        .any(|root| normalized_path.starts_with(root))
    {
        return Ok(());
    }

    let allowed = normalized_roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "Path escapes allowed roots: {} is outside [{}]",
        normalized_path.display(),
        allowed
    )
}

fn normalize_for_prefix(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path)
            .with_context(|| format!("failed to canonicalize {}", path.display()));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut existing_ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !existing_ancestor.exists() {
        let file_name = existing_ancestor
            .file_name()
            .ok_or_else(|| anyhow!("path has no existing ancestor: {}", absolute.display()))?;
        suffix.push(file_name.to_os_string());
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or_else(|| anyhow!("path has no parent: {}", absolute.display()))?;
    }
    let mut normalized = std::fs::canonicalize(existing_ancestor)?;
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::assert_path_inside_allowed_roots;

    #[test]
    fn allowed_roots_accept_any_configured_root() {
        let workspace = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let allowed_path = extra.path().join("nested/file.txt");

        let result =
            assert_path_inside_allowed_roots(&allowed_path, [workspace.path(), extra.path()]);
        assert!(result.is_ok());
    }

    #[test]
    fn allowed_roots_reject_paths_outside_every_root() {
        let workspace = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        let outsider = tempfile::tempdir().unwrap();
        let denied_path = outsider.path().join("nested/file.txt");

        let err = assert_path_inside_allowed_roots(&denied_path, [workspace.path(), extra.path()])
            .unwrap_err();
        assert!(err.to_string().contains("Path escapes allowed roots"));
    }
}
