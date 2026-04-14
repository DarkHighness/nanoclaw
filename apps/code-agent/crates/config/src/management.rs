use agent::mcp::McpServerConfig;
use anyhow::{Context, Result, anyhow, bail};
use nanoclaw_config::CoreConfig;
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::{add_core_mcp_server, delete_core_mcp_server, set_core_mcp_server_enabled};
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use nanoclaw_config::CoreConfig;
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
}
