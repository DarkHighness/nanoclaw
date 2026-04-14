use agent::AgentWorkspaceLayout;
use agent::mcp::McpServerConfig;
use agent::skills::{SkillRoot, SkillRootKind, load_skill_from_dir};
use anyhow::{Context, Result, anyhow, bail};
use nanoclaw_config::CoreConfig;
use std::path::{Path, PathBuf};
use tokio::fs;

const DISABLED_SKILL_DIR: &str = ".disabled";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSkillArtifact {
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub enabled: bool,
}

pub fn add_core_mcp_server(workspace_root: &Path, server: McpServerConfig) -> Result<PathBuf> {
    let mut config = load_raw_core_config(workspace_root)?;
    if config
        .mcp_servers
        .iter()
        .any(|candidate| candidate.name == server.name)
    {
        bail!("MCP server `{}` already exists", server.name);
    }
    config.mcp_servers.push(server);
    write_raw_core_config(workspace_root, &config)
}

pub fn delete_core_mcp_server(workspace_root: &Path, name: &str) -> Result<PathBuf> {
    let mut config = load_raw_core_config(workspace_root)?;
    let original_len = config.mcp_servers.len();
    config
        .mcp_servers
        .retain(|server| server.name.as_str() != name);
    if config.mcp_servers.len() == original_len {
        bail!("unknown MCP server `{name}`");
    }
    write_raw_core_config(workspace_root, &config)
}

pub fn set_core_mcp_server_enabled(
    workspace_root: &Path,
    name: &str,
    enabled: bool,
) -> Result<PathBuf> {
    let mut config = load_raw_core_config(workspace_root)?;
    let server = config
        .mcp_servers
        .iter_mut()
        .find(|server| server.name.as_str() == name)
        .ok_or_else(|| anyhow!("unknown MCP server `{name}`"))?;
    server.enabled = enabled;
    write_raw_core_config(workspace_root, &config)
}

pub async fn add_managed_skill(
    workspace_root: &Path,
    source: &Path,
) -> Result<ManagedSkillArtifact> {
    let managed_root = AgentWorkspaceLayout::new(workspace_root).skills_dir();
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve source skill {}", source.display()))?;
    let source_skill = load_skill_from_dir(&source, &SkillRoot::external(source.clone()))
        .await
        .with_context(|| format!("failed to load source skill {}", source.display()))?;
    if find_managed_skill(&managed_root, &source_skill.name, true)
        .await?
        .is_some()
    {
        bail!(
            "managed skill `{}` already exists; delete or disable it first",
            source_skill.name
        );
    }
    if find_managed_skill(&managed_root, &source_skill.name, false)
        .await?
        .is_some()
    {
        bail!(
            "managed skill `{}` is currently disabled; run `skill enable {}` instead",
            source_skill.name,
            source_skill.name
        );
    }
    fs::create_dir_all(&managed_root)
        .await
        .with_context(|| format!("failed to create {}", managed_root.display()))?;
    let destination = managed_root.join(&source_skill.name);
    if destination == source {
        bail!(
            "source skill is already installed at {}",
            destination.display()
        );
    }
    copy_directory_tree(&source, &destination).await?;
    Ok(ManagedSkillArtifact {
        skill_name: source_skill.name,
        skill_path: destination,
        enabled: true,
    })
}

pub async fn delete_managed_skill(
    workspace_root: &Path,
    name: &str,
) -> Result<ManagedSkillArtifact> {
    let managed_root = AgentWorkspaceLayout::new(workspace_root).skills_dir();
    let skill = resolve_managed_skill(&managed_root, name).await?;
    fs::remove_dir_all(&skill.skill_path)
        .await
        .with_context(|| format!("failed to delete {}", skill.skill_path.display()))?;
    Ok(skill)
}

pub async fn set_managed_skill_enabled(
    workspace_root: &Path,
    name: &str,
    enabled: bool,
) -> Result<ManagedSkillArtifact> {
    let managed_root = AgentWorkspaceLayout::new(workspace_root).skills_dir();
    let disabled_root = disabled_skill_root(&managed_root);
    let skill = resolve_managed_skill(&managed_root, name).await?;
    if skill.enabled == enabled {
        let verb = if enabled { "enabled" } else { "disabled" };
        bail!("managed skill `{}` is already {verb}", skill.skill_name);
    }
    if enabled {
        let destination = managed_root.join(&skill.skill_name);
        if destination.exists() {
            bail!(
                "cannot enable skill `{}` because {} already exists",
                skill.skill_name,
                destination.display()
            );
        }
        fs::rename(&skill.skill_path, &destination)
            .await
            .with_context(|| format!("failed to move {}", skill.skill_path.display()))?;
        Ok(ManagedSkillArtifact {
            skill_name: skill.skill_name,
            skill_path: destination,
            enabled: true,
        })
    } else {
        fs::create_dir_all(&disabled_root)
            .await
            .with_context(|| format!("failed to create {}", disabled_root.display()))?;
        let destination = disabled_root.join(&skill.skill_name);
        if destination.exists() {
            bail!(
                "cannot disable skill `{}` because {} already exists",
                skill.skill_name,
                destination.display()
            );
        }
        // Disabled managed skills live under `.disabled/` so the normal skill
        // loader ignores them: it only scans immediate children of the managed
        // root that themselves contain `SKILL.md`.
        fs::rename(&skill.skill_path, &destination)
            .await
            .with_context(|| format!("failed to move {}", skill.skill_path.display()))?;
        Ok(ManagedSkillArtifact {
            skill_name: skill.skill_name,
            skill_path: destination,
            enabled: false,
        })
    }
}

fn load_raw_core_config(workspace_root: &Path) -> Result<CoreConfig> {
    let path = CoreConfig::config_path(workspace_root);
    if !path.exists() {
        return Ok(CoreConfig::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(CoreConfig::default());
    }
    toml::from_str::<CoreConfig>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))
}

fn write_raw_core_config(workspace_root: &Path, config: &CoreConfig) -> Result<PathBuf> {
    let path = CoreConfig::config_path(workspace_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }
    // Management commands rewrite the typed core config intentionally. Arrays of
    // tables such as `mcp_servers` are easier to keep structurally correct when
    // the host owns serialization instead of patching nested TOML fragments.
    let mut serialized =
        toml::to_string_pretty(config).context("failed to serialize core config")?;
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    std::fs::write(&path, serialized)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn disabled_skill_root(managed_root: &Path) -> PathBuf {
    managed_root.join(DISABLED_SKILL_DIR)
}

async fn resolve_managed_skill(managed_root: &Path, name: &str) -> Result<ManagedSkillArtifact> {
    let active = find_managed_skill(managed_root, name, true).await?;
    let disabled = find_managed_skill(managed_root, name, false).await?;
    match (active, disabled) {
        (Some(skill), None) | (None, Some(skill)) => Ok(skill),
        (None, None) => bail!("unknown managed skill `{name}`"),
        (Some(_), Some(_)) => {
            bail!("managed skill `{name}` exists in both active and disabled locations")
        }
    }
}

async fn find_managed_skill(
    managed_root: &Path,
    name: &str,
    enabled: bool,
) -> Result<Option<ManagedSkillArtifact>> {
    let root = if enabled {
        managed_root.to_path_buf()
    } else {
        disabled_skill_root(managed_root)
    };
    let skill_dirs = collect_skill_directories(&root).await?;
    let mut matched = None;
    for skill_dir in skill_dirs {
        let skill = load_skill_from_dir(
            &skill_dir,
            &SkillRoot {
                path: managed_root.to_path_buf(),
                kind: SkillRootKind::Managed,
            },
        )
        .await
        .with_context(|| format!("failed to load managed skill {}", skill_dir.display()))?;
        if skill.name == name {
            if matched.is_some() {
                bail!("multiple managed skill copies matched `{name}`");
            }
            matched = Some(ManagedSkillArtifact {
                skill_name: skill.name,
                skill_path: skill_dir,
                enabled,
            });
        }
    }
    Ok(matched)
}

async fn collect_skill_directories(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(root)
        .await
        .with_context(|| format!("failed to read {}", root.display()))?;
    let mut directories = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if entry.file_type().await?.is_dir() && path.join("SKILL.md").exists() {
            directories.push(path);
        }
    }
    directories.sort();
    Ok(directories)
}

async fn copy_directory_tree(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        bail!("destination already exists: {}", destination.display());
    }
    fs::create_dir_all(destination)
        .await
        .with_context(|| format!("failed to create {}", destination.display()))?;
    copy_directory_entries(source, destination).await
}

fn copy_directory_entries<'a>(
    source: &'a Path,
    destination: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = fs::read_dir(source)
            .await
            .with_context(|| format!("failed to read {}", source.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type().await?.is_dir() {
                fs::create_dir_all(&destination_path)
                    .await
                    .with_context(|| format!("failed to create {}", destination_path.display()))?;
                copy_directory_entries(&entry_path, &destination_path).await?;
            } else {
                let _ = fs::copy(&entry_path, &destination_path)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to copy {} to {}",
                            entry_path.display(),
                            destination_path.display()
                        )
                    })?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        add_core_mcp_server, add_managed_skill, delete_core_mcp_server, delete_managed_skill,
        set_core_mcp_server_enabled, set_managed_skill_enabled,
    };
    use agent::AgentWorkspaceLayout;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use nanoclaw_config::CoreConfig;
    use std::path::Path;
    use tempfile::tempdir;

    fn stdio_server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "npx".to_string(),
                args: vec!["demo-mcp".to_string()],
                env: Default::default(),
                cwd: None,
            },
        }
    }

    #[test]
    fn add_core_mcp_server_persists_new_entry() {
        let dir = tempdir().unwrap();

        let path = add_core_mcp_server(dir.path(), stdio_server("docs")).unwrap();
        let config = CoreConfig::load_from_dir(dir.path()).unwrap();

        assert_eq!(path, CoreConfig::config_path(dir.path()));
        assert_eq!(config.mcp_servers.len(), 1);
        assert_eq!(config.mcp_servers[0].name.as_str(), "docs");
        assert!(config.mcp_servers[0].enabled);
    }

    #[test]
    fn set_core_mcp_server_enabled_persists_false() {
        let dir = tempdir().unwrap();
        add_core_mcp_server(dir.path(), stdio_server("docs")).unwrap();

        set_core_mcp_server_enabled(dir.path(), "docs", false).unwrap();
        let config = CoreConfig::load_from_dir(dir.path()).unwrap();

        assert_eq!(config.mcp_servers.len(), 1);
        assert!(!config.mcp_servers[0].enabled);
    }

    #[test]
    fn delete_core_mcp_server_removes_entry() {
        let dir = tempdir().unwrap();
        add_core_mcp_server(dir.path(), stdio_server("docs")).unwrap();

        delete_core_mcp_server(dir.path(), "docs").unwrap();
        let config = CoreConfig::load_from_dir(dir.path()).unwrap();

        assert!(config.mcp_servers.is_empty());
    }

    fn write_skill_source(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Demo skill\n---\nUse this skill carefully.\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn add_managed_skill_copies_into_workspace_root() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review");
        write_skill_source(&source, "review");

        let artifact = add_managed_skill(workspace.path(), &source).await.unwrap();

        assert_eq!(artifact.skill_name, "review");
        assert!(artifact.enabled);
        assert_eq!(
            artifact.skill_path,
            AgentWorkspaceLayout::new(workspace.path())
                .skills_dir()
                .join("review")
        );
        assert!(artifact.skill_path.join("SKILL.md").is_file());
    }

    #[tokio::test]
    async fn disable_and_enable_managed_skill_moves_between_locations() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review");
        write_skill_source(&source, "review");
        add_managed_skill(workspace.path(), &source).await.unwrap();

        let disabled = set_managed_skill_enabled(workspace.path(), "review", false)
            .await
            .unwrap();
        assert!(!disabled.enabled);
        assert!(disabled.skill_path.ends_with(".disabled/review"));

        let enabled = set_managed_skill_enabled(workspace.path(), "review", true)
            .await
            .unwrap();
        assert!(enabled.enabled);
        assert!(enabled.skill_path.ends_with(".nanoclaw/skills/review"));
    }

    #[tokio::test]
    async fn delete_managed_skill_removes_disabled_copy() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review");
        write_skill_source(&source, "review");
        add_managed_skill(workspace.path(), &source).await.unwrap();
        set_managed_skill_enabled(workspace.path(), "review", false)
            .await
            .unwrap();

        let artifact = delete_managed_skill(workspace.path(), "review")
            .await
            .unwrap();

        assert_eq!(artifact.skill_name, "review");
        assert!(!artifact.skill_path.exists());
    }
}
