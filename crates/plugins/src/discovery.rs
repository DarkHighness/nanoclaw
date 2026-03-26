use crate::error::{PluginError, Result};
use crate::manifest::PluginManifest;
use mcp::McpServerConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use types::HookRegistration;

const MANIFEST_RELATIVE_PATH: &str = ".nanoclaw-plugin/plugin.toml";

#[derive(Clone, Debug)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    pub root_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub skill_roots: Vec<PathBuf>,
    pub hooks: Vec<HookRegistration>,
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginDiagnosticLevel {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginDiagnostic {
    pub level: PluginDiagnosticLevel,
    pub code: &'static str,
    pub message: String,
    pub plugin_id: Option<String>,
    pub path: Option<PathBuf>,
}

impl PluginDiagnostic {
    #[must_use]
    pub fn warning(
        code: &'static str,
        message: impl Into<String>,
        plugin_id: Option<String>,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            level: PluginDiagnosticLevel::Warning,
            code,
            message: message.into(),
            plugin_id,
            path,
        }
    }

    #[must_use]
    pub fn error(
        code: &'static str,
        message: impl Into<String>,
        plugin_id: Option<String>,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            level: PluginDiagnosticLevel::Error,
            code,
            message: message.into(),
            plugin_id,
            path,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PluginDiscovery {
    pub plugins: Vec<DiscoveredPlugin>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Clone, Debug, Deserialize)]
struct HookFile {
    #[serde(default)]
    hooks: Vec<HookRegistration>,
}

#[derive(Clone, Debug, Deserialize)]
struct McpFile {
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
}

use serde::Deserialize;

pub fn discover_plugins(roots: &[PathBuf]) -> Result<PluginDiscovery> {
    let mut discovered = PluginDiscovery::default();
    let mut seen_ids = BTreeSet::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        if !root.is_dir() {
            discovered.diagnostics.push(PluginDiagnostic::warning(
                "plugin_root_not_directory",
                format!("plugin root is not a directory: {}", root.display()),
                None,
                Some(root.clone()),
            ));
            continue;
        }

        let mut entries = fs::read_dir(root)?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        entries.sort();

        for plugin_root in entries {
            let manifest_path = plugin_root.join(MANIFEST_RELATIVE_PATH);
            if !manifest_path.exists() {
                continue;
            }
            match load_plugin(&plugin_root, &manifest_path) {
                Ok(plugin) => {
                    if !seen_ids.insert(plugin.manifest.id.clone()) {
                        discovered.diagnostics.push(PluginDiagnostic::warning(
                            "plugin_duplicate_id",
                            format!(
                                "duplicate plugin id `{}` ignored at {}",
                                plugin.manifest.id,
                                plugin_root.display()
                            ),
                            Some(plugin.manifest.id.clone()),
                            Some(manifest_path),
                        ));
                        continue;
                    }
                    discovered.plugins.push(plugin);
                }
                Err(error) => {
                    discovered.diagnostics.push(PluginDiagnostic::error(
                        "plugin_load_failed",
                        error.to_string(),
                        None,
                        Some(manifest_path),
                    ));
                }
            }
        }
    }

    Ok(discovered)
}

fn load_plugin(plugin_root: &Path, manifest_path: &Path) -> Result<DiscoveredPlugin> {
    let raw = fs::read_to_string(manifest_path)?;
    let manifest: PluginManifest = toml::from_str(&raw)?;
    if manifest.id.trim().is_empty() {
        return Err(PluginError::invalid_manifest(
            manifest_path.to_path_buf(),
            "plugin id cannot be empty",
        ));
    }

    let skill_roots = manifest
        .components
        .skill_roots
        .iter()
        .map(|value| resolve_safe_relative_path(plugin_root, value))
        .collect::<Result<Vec<_>>>()?;

    let hook_paths = manifest
        .components
        .hook_files
        .iter()
        .map(|value| resolve_safe_relative_path(plugin_root, value))
        .collect::<Result<Vec<_>>>()?;
    let mcp_paths = manifest
        .components
        .mcp_files
        .iter()
        .map(|value| resolve_safe_relative_path(plugin_root, value))
        .collect::<Result<Vec<_>>>()?;

    let mut hooks = Vec::new();
    for path in hook_paths {
        let parsed: HookFile = toml::from_str(&fs::read_to_string(&path)?)?;
        hooks.extend(parsed.hooks);
    }

    let mut mcp_servers = Vec::new();
    for path in mcp_paths {
        let mut parsed: McpFile = toml::from_str(&fs::read_to_string(&path)?)?;
        for server in &mut parsed.mcp_servers {
            if let mcp::McpTransportConfig::Stdio { cwd: Some(cwd), .. } = &mut server.transport {
                let rebased = resolve_safe_relative_path(plugin_root, cwd)?;
                *cwd = rebased.to_string_lossy().to_string();
            }
        }
        mcp_servers.extend(parsed.mcp_servers);
    }

    Ok(DiscoveredPlugin {
        manifest,
        root_dir: plugin_root.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        skill_roots,
        hooks,
        mcp_servers,
    })
}

fn resolve_safe_relative_path(root: &Path, value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Err(PluginError::invalid_path(
            value,
            "absolute paths are not allowed in plugin manifests",
        ));
    }

    // Plugin-owned paths must remain inside the plugin root. We reject ".."
    // components up front so plugin metadata cannot escape into host files.
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(PluginError::invalid_path(
                value,
                "parent/root/prefix path components are not allowed",
            ));
        }
    }
    Ok(root.join(path))
}

pub fn plugins_by_id(plugins: Vec<DiscoveredPlugin>) -> BTreeMap<String, DiscoveredPlugin> {
    plugins
        .into_iter()
        .map(|plugin| (plugin.manifest.id.clone(), plugin))
        .collect()
}
