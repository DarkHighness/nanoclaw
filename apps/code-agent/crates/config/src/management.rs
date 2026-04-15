use agent::mcp::{McpServerConfig, McpTransportConfig};
use agent::plugins::{
    PluginEntryConfig, PluginKind, PluginManifest, PluginState, discover_plugins,
};
use agent::skills::{SkillRoot, SkillRootKind, load_skill_from_dir};
use agent::types::PluginId;
use agent::{AgentWorkspaceLayout, PluginBootResolverConfig, build_plugin_activation_plan};
use agent_env::{EnvMap, EnvVar, vars};
use anyhow::{Context, Result, anyhow, bail};
use include_dir::{Dir, DirEntry, include_dir};
use nanoclaw_config::CoreConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tokio::fs;
use toml_edit::{Array, DocumentMut, Item, Table, value};

const DISABLED_SKILL_DIR: &str = ".disabled";
const BUILTIN_CONTEXT7_SERVER: &str = "context7";
const BUILTIN_PLAYWRIGHT_SERVER: &str = "playwright";
const BUILTIN_CONTEXT7_PACKAGE: &str = "@upstash/context7-mcp@latest";
const BUILTIN_PLAYWRIGHT_PACKAGE: &str = "@playwright/mcp@latest";
// Bundle built-in skills with their companion references, scripts, agents, and
// assets instead of flattening them to SKILL.md-only copies. This tree contains
// an exact vendored subset of openai/skills @
// e6afb0d74cc75d220df2faf3dd6c635c2dc6a108 plus host-authored Linux
// performance skills documented in skills/CUSTOM_BUILTIN_SKILLS.txt.
static BUILTIN_SKILLS_SOURCE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../skills");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSkillArtifact {
    pub skill_name: String,
    pub skill_path: PathBuf,
    pub enabled: bool,
    pub builtin: bool,
    pub skill_root: SkillRoot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedPluginArtifact {
    pub plugin_id: PluginId,
    pub plugin_path: PathBuf,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSkillDetail {
    pub skill_name: String,
    pub description: String,
    pub skill_path: PathBuf,
    pub enabled: bool,
    pub builtin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedPluginDetail {
    pub plugin_id: PluginId,
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub kind: String,
    pub plugin_path: PathBuf,
    pub enabled: bool,
    pub reason: String,
    pub contribution_summary: String,
}

pub fn list_core_mcp_servers(workspace_root: &Path) -> Result<Vec<McpServerConfig>> {
    let env_map = EnvMap::from_workspace_dir(workspace_root)?;
    let config = load_raw_core_config(workspace_root)?;
    Ok(merged_core_mcp_servers(&env_map, &config.mcp_servers))
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
    if config.mcp_servers.len() != original_len {
        return write_raw_core_config(workspace_root, &config);
    }
    if builtin_mcp_definition(name).is_some() {
        bail!(
            "built-in MCP server `{name}` cannot be deleted; use `mcp disable {name}` or `/mcp` instead"
        );
    }
    bail!("unknown MCP server `{name}`");
}

pub fn set_core_mcp_server_enabled(
    workspace_root: &Path,
    name: &str,
    enabled: bool,
) -> Result<PathBuf> {
    let mut config = load_raw_core_config(workspace_root)?;
    if let Some(server) = config
        .mcp_servers
        .iter_mut()
        .find(|server| server.name.as_str() == name)
    {
        if server.enabled == enabled {
            let verb = if enabled { "enabled" } else { "disabled" };
            bail!("MCP server `{name}` is already {verb}");
        }
        server.enabled = enabled;
        return write_raw_core_config(workspace_root, &config);
    }

    let env_map = EnvMap::from_workspace_dir(workspace_root)?;
    let mut server = builtin_core_mcp_server(&env_map, name)
        .ok_or_else(|| anyhow!("unknown MCP server `{name}`"))?;
    if server.enabled == enabled {
        let verb = if enabled { "enabled" } else { "disabled" };
        bail!("MCP server `{name}` is already {verb}");
    }
    server.enabled = enabled;
    config.mcp_servers.push(server);
    write_raw_core_config(workspace_root, &config)
}

pub(crate) fn materialize_builtin_core_mcp_servers(env_map: &EnvMap, config: &mut CoreConfig) {
    // Built-in MCP servers behave like default managed entries: they should be
    // visible to CLI/TUI management even in a clean workspace, but they should
    // only be persisted once the operator explicitly overrides their state.
    config.mcp_servers = merged_core_mcp_servers(env_map, &config.mcp_servers);
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

pub fn builtin_skill_root(workspace_root: &Path) -> PathBuf {
    AgentWorkspaceLayout::new(workspace_root)
        .apps_dir()
        .join("code-agent")
        .join("builtin-skills")
}

fn write_embedded_skill_dir(dir: &Dir<'_>, destination: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(child) => {
                let child_destination = destination.join(
                    child
                        .path()
                        .file_name()
                        .ok_or_else(|| anyhow!("missing directory name in embedded skill tree"))?,
                );
                std::fs::create_dir_all(&child_destination)
                    .with_context(|| format!("failed to create {}", child_destination.display()))?;
                write_embedded_skill_dir(child, &child_destination)?;
            }
            DirEntry::File(file) => {
                let file_destination = destination.join(
                    file.path()
                        .file_name()
                        .ok_or_else(|| anyhow!("missing file name in embedded skill tree"))?,
                );
                std::fs::write(&file_destination, file.contents())
                    .with_context(|| format!("failed to write {}", file_destination.display()))?;
            }
        }
    }
    Ok(())
}

pub fn disabled_builtin_skill_names(workspace_root: &Path) -> Result<BTreeSet<String>> {
    let app = nanoclaw_config::load_optional_app_config::<super::CodeAgentAppConfig>(
        workspace_root,
        super::CODE_AGENT_APP_NAME,
    )?;
    Ok(app
        .skills
        .disabled_builtin
        .into_iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect())
}

fn persist_builtin_skill_enabled(workspace_root: &Path, name: &str, enabled: bool) -> Result<()> {
    let path = workspace_root.join(super::CODE_AGENT_APP_CONFIG_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let raw = if path.exists() {
        std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let mut document = if raw.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw.parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    };

    let mut disabled = disabled_builtin_skill_names(workspace_root)?;
    if enabled {
        disabled.remove(name);
    } else {
        disabled.insert(name.to_string());
    }

    let root = document.as_table_mut();
    let skills_item = root.entry("skills").or_insert(Item::Table(Table::new()));
    if !skills_item.is_table() {
        *skills_item = Item::Table(Table::new());
    }
    let skills = skills_item
        .as_table_mut()
        .expect("skills config must be a TOML table");
    if disabled.is_empty() {
        skills.remove("disabled_builtin");
    } else {
        let mut items = Array::new();
        for name in disabled {
            items.push(name);
        }
        skills["disabled_builtin"] = value(items);
    }

    let mut serialized = document.to_string();
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    std::fs::write(&path, serialized).with_context(|| format!("failed to write {}", path.display()))
}

pub fn filter_unavailable_builtin_mcp_servers(
    env_map: &EnvMap,
    servers: Vec<McpServerConfig>,
    warnings: &mut Vec<String>,
) -> Vec<McpServerConfig> {
    servers
        .into_iter()
        .filter_map(|server| {
            let Some(definition) = builtin_mcp_definition(server.name.as_str()) else {
                return Some(server);
            };
            if !matches_builtin_core_mcp_server(&server, &definition) {
                return Some(server);
            }
            if select_builtin_launcher(env_map, &definition.launchers).is_some() {
                return Some(server);
            }
            warnings.push(format!(
                "built-in MCP server `{}` is enabled but no supported launcher is available in PATH; install one of {} or disable it with `mcp disable {}` or `/mcp`",
                definition.name,
                render_launcher_list(&definition.launchers),
                definition.name,
            ));
            None
        })
        .collect()
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
        builtin: false,
        skill_root: SkillRoot::managed(managed_root),
    })
}

pub async fn list_managed_skills(workspace_root: &Path) -> Result<Vec<ManagedSkillArtifact>> {
    materialize_builtin_skills(workspace_root)?;
    let managed_root = AgentWorkspaceLayout::new(workspace_root).skills_dir();
    let builtin_root = builtin_skill_root(workspace_root);
    let disabled_builtin = disabled_builtin_skill_names(workspace_root)?;
    let mut skills = collect_managed_skill_artifacts(&managed_root, true).await?;
    skills.extend(collect_managed_skill_artifacts(&managed_root, false).await?);
    let existing_names = skills
        .iter()
        .map(|artifact| artifact.skill_name.clone())
        .collect::<BTreeSet<_>>();
    skills.extend(
        collect_builtin_skill_artifacts(&builtin_root, &disabled_builtin, &existing_names).await?,
    );
    skills.sort_by(|left, right| left.skill_name.cmp(&right.skill_name));
    Ok(skills)
}

pub async fn list_managed_skill_details(workspace_root: &Path) -> Result<Vec<ManagedSkillDetail>> {
    let artifacts = list_managed_skills(workspace_root).await?;
    let mut details = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        details.push(load_managed_skill_detail_from_artifact(artifact).await?);
    }
    Ok(details)
}

pub async fn load_managed_skill_detail(
    workspace_root: &Path,
    name: &str,
) -> Result<ManagedSkillDetail> {
    let artifact = resolve_managed_skill(workspace_root, name).await?;
    load_managed_skill_detail_from_artifact(artifact).await
}

pub async fn delete_managed_skill(
    workspace_root: &Path,
    name: &str,
) -> Result<ManagedSkillArtifact> {
    let skill = resolve_managed_skill(workspace_root, name).await?;
    if skill.builtin {
        bail!(
            "built-in skill `{name}` cannot be deleted; use `skill disable {name}` or `/skill` instead"
        );
    }
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
    let skill = resolve_managed_skill(workspace_root, name).await?;
    if skill.enabled == enabled {
        let verb = if enabled { "enabled" } else { "disabled" };
        let scope = if skill.builtin {
            "built-in skill"
        } else {
            "managed skill"
        };
        bail!("{scope} `{}` is already {verb}", skill.skill_name);
    }
    if skill.builtin {
        persist_builtin_skill_enabled(workspace_root, &skill.skill_name, enabled)?;
        return Ok(ManagedSkillArtifact { enabled, ..skill });
    }
    let disabled_root = disabled_skill_root(&managed_root);
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
            builtin: false,
            skill_root: SkillRoot::managed(managed_root),
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
            builtin: false,
            skill_root: SkillRoot::managed(managed_root),
        })
    }
}

pub async fn add_managed_plugin(
    workspace_root: &Path,
    source: &Path,
) -> Result<ManagedPluginArtifact> {
    let managed_root = AgentWorkspaceLayout::new(workspace_root).plugins_dir();
    let source = source
        .canonicalize()
        .with_context(|| format!("failed to resolve source plugin {}", source.display()))?;
    let manifest = load_plugin_manifest(&source)?;
    let existing_ids = discover_plugin_ids(workspace_root, &load_raw_core_config(workspace_root)?)?;
    if existing_ids
        .iter()
        .any(|candidate| candidate == &manifest.id)
    {
        bail!(
            "plugin `{}` is already discoverable from the current plugin roots",
            manifest.id
        );
    }
    fs::create_dir_all(&managed_root)
        .await
        .with_context(|| format!("failed to create {}", managed_root.display()))?;
    let destination = managed_root.join(manifest.id.as_str());
    if destination == source {
        bail!(
            "source plugin is already installed at {}",
            destination.display()
        );
    }
    copy_directory_tree(&source, &destination).await?;

    let mut config = load_raw_core_config(workspace_root)?;
    ensure_managed_plugin_root(&mut config);
    apply_plugin_enabled_override(&mut config, &manifest.id, true);
    write_raw_core_config(workspace_root, &config)?;

    Ok(ManagedPluginArtifact {
        plugin_id: manifest.id,
        plugin_path: destination,
        enabled: true,
    })
}

pub fn list_managed_plugin_details(workspace_root: &Path) -> Result<Vec<ManagedPluginDetail>> {
    let config = load_raw_core_config(workspace_root)?;
    let managed_root = AgentWorkspaceLayout::new(workspace_root).plugins_dir();
    if !managed_root.exists() {
        return Ok(Vec::new());
    }

    let resolved_roots = config.resolved_plugin_roots(workspace_root);
    let mut discovery_roots = resolved_roots.clone();
    if !discovery_roots
        .iter()
        .any(|candidate| candidate == &managed_root)
    {
        discovery_roots.push(managed_root.clone());
    }
    let discovery = discover_plugins(&discovery_roots).with_context(|| {
        format!(
            "failed to discover plugins under {}",
            workspace_root.display()
        )
    })?;
    let activation_plan = build_plugin_activation_plan(
        workspace_root,
        &PluginBootResolverConfig {
            enabled: config.plugins.enabled,
            roots: resolved_roots,
            include_builtin: config.plugins.include_builtin,
            allow: config.plugins.allow.clone(),
            deny: config.plugins.deny.clone(),
            entries: config.plugins.entries.clone(),
            slots: config.plugins.slots.clone(),
        },
    )?;
    let mut states = activation_plan
        .plugin_states
        .into_iter()
        .map(|state| (state.plugin_id.clone(), state))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut plugins = discovery
        .plugins
        .into_iter()
        .filter(|plugin| plugin.root_dir.starts_with(&managed_root))
        .map(|plugin| {
            let state = states.remove(&plugin.manifest.id);
            ManagedPluginDetail {
                plugin_id: plugin.manifest.id.clone(),
                name: plugin.manifest.name.clone(),
                description: plugin.manifest.description.clone(),
                version: plugin.manifest.version.clone(),
                kind: plugin_kind_label(plugin.manifest.kind).to_string(),
                plugin_path: plugin.root_dir,
                enabled: state
                    .as_ref()
                    .map(|state| state.enabled)
                    .unwrap_or(plugin.manifest.enabled_by_default),
                reason: state
                    .as_ref()
                    .map(|state| state.reason.clone())
                    .unwrap_or_else(|| "not included in activation plan".to_string()),
                contribution_summary: state
                    .as_ref()
                    .map(plugin_contribution_summary)
                    .unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();
    plugins.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    Ok(plugins)
}

pub fn load_managed_plugin_detail(
    workspace_root: &Path,
    plugin_id: &str,
) -> Result<ManagedPluginDetail> {
    let plugin_id = PluginId::from(plugin_id);
    list_managed_plugin_details(workspace_root)?
        .into_iter()
        .find(|plugin| plugin.plugin_id == plugin_id)
        .ok_or_else(|| anyhow!("unknown managed plugin `{plugin_id}`"))
}

pub fn set_managed_plugin_enabled(
    workspace_root: &Path,
    plugin_id: &str,
    enabled: bool,
) -> Result<PathBuf> {
    let mut config = load_raw_core_config(workspace_root)?;
    let plugin_id = PluginId::from(plugin_id);
    let discovered = discover_plugin_ids(workspace_root, &config)?;
    if !discovered.iter().any(|candidate| candidate == &plugin_id)
        && !config.plugins.entries.contains_key(&plugin_id)
    {
        bail!("unknown plugin `{}`", plugin_id);
    }
    if managed_plugin_path(workspace_root, &plugin_id).exists() {
        ensure_managed_plugin_root(&mut config);
    }
    apply_plugin_enabled_override(&mut config, &plugin_id, enabled);
    write_raw_core_config(workspace_root, &config)
}

pub async fn delete_managed_plugin(
    workspace_root: &Path,
    plugin_id: &str,
) -> Result<ManagedPluginArtifact> {
    let plugin_id = PluginId::from(plugin_id);
    let plugin_path = managed_plugin_path(workspace_root, &plugin_id);
    if !plugin_path.join(".nanoclaw-plugin/plugin.toml").exists() {
        bail!("unknown managed plugin `{}`", plugin_id);
    }
    fs::remove_dir_all(&plugin_path)
        .await
        .with_context(|| format!("failed to delete {}", plugin_path.display()))?;

    let mut config = load_raw_core_config(workspace_root)?;
    config.plugins.entries.remove(&plugin_id);
    config
        .plugins
        .allow
        .retain(|candidate| candidate != &plugin_id);
    config
        .plugins
        .deny
        .retain(|candidate| candidate != &plugin_id);
    if config.plugins.slots.memory.as_ref() == Some(&plugin_id) {
        config.plugins.slots.memory = None;
    }
    write_raw_core_config(workspace_root, &config)?;

    Ok(ManagedPluginArtifact {
        plugin_id,
        plugin_path,
        enabled: false,
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltinMcpLauncher {
    PnpmDlx,
    Npx,
    Bunx,
}

impl BuiltinMcpLauncher {
    fn label(self) -> &'static str {
        match self {
            Self::PnpmDlx => "`pnpm dlx`",
            Self::Npx => "`npx`",
            Self::Bunx => "`bunx`",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::PnpmDlx => "pnpm",
            Self::Npx => "npx",
            Self::Bunx => "bunx",
        }
    }

    fn render(self, package: &str) -> (String, Vec<String>) {
        match self {
            Self::PnpmDlx => (
                "pnpm".to_string(),
                vec!["dlx".to_string(), package.to_string()],
            ),
            Self::Npx => (
                "npx".to_string(),
                vec!["-y".to_string(), package.to_string()],
            ),
            Self::Bunx => ("bunx".to_string(), vec![package.to_string()]),
        }
    }
}

#[derive(Clone, Copy)]
struct BuiltinMcpDefinition {
    name: &'static str,
    package: &'static str,
    launchers: &'static [BuiltinMcpLauncher],
    passthrough_env: &'static [EnvVar],
}

const NODE_PACKAGE_LAUNCHERS: &[BuiltinMcpLauncher] = &[
    BuiltinMcpLauncher::PnpmDlx,
    BuiltinMcpLauncher::Npx,
    BuiltinMcpLauncher::Bunx,
];

fn builtin_mcp_definition(name: &str) -> Option<BuiltinMcpDefinition> {
    match name {
        BUILTIN_CONTEXT7_SERVER => Some(BuiltinMcpDefinition {
            name: BUILTIN_CONTEXT7_SERVER,
            package: BUILTIN_CONTEXT7_PACKAGE,
            launchers: NODE_PACKAGE_LAUNCHERS,
            passthrough_env: &[vars::CONTEXT7_API_KEY],
        }),
        BUILTIN_PLAYWRIGHT_SERVER => Some(BuiltinMcpDefinition {
            name: BUILTIN_PLAYWRIGHT_SERVER,
            package: BUILTIN_PLAYWRIGHT_PACKAGE,
            launchers: NODE_PACKAGE_LAUNCHERS,
            passthrough_env: &[],
        }),
        _ => None,
    }
}

fn builtin_core_mcp_servers(env_map: &EnvMap) -> Vec<McpServerConfig> {
    [BUILTIN_CONTEXT7_SERVER, BUILTIN_PLAYWRIGHT_SERVER]
        .into_iter()
        .filter_map(|name| builtin_core_mcp_server(env_map, name))
        .collect()
}

fn builtin_core_mcp_server(env_map: &EnvMap, name: &str) -> Option<McpServerConfig> {
    let definition = builtin_mcp_definition(name)?;
    let launcher = select_builtin_launcher(env_map, &definition.launchers)
        .unwrap_or(*definition.launchers.first().expect("built-in launcher"));
    let (command, args) = launcher.render(definition.package);
    Some(McpServerConfig {
        name: definition.name.into(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command,
            args,
            env: builtin_mcp_env(env_map, definition.passthrough_env),
            cwd: None,
        },
    })
}

fn merged_core_mcp_servers(
    env_map: &EnvMap,
    configured: &[McpServerConfig],
) -> Vec<McpServerConfig> {
    let mut merged = configured.to_vec();
    for builtin in builtin_core_mcp_servers(env_map) {
        if configured.iter().any(|server| server.name == builtin.name) {
            continue;
        }
        merged.push(builtin);
    }
    merged
}

fn builtin_mcp_env(env_map: &EnvMap, passthrough: &[EnvVar]) -> BTreeMap<String, String> {
    passthrough
        .iter()
        .filter_map(|variable| {
            env_map
                .get_non_empty_var(*variable)
                .map(|value| (variable.key.to_string(), value))
        })
        .collect()
}

fn select_builtin_launcher(
    env_map: &EnvMap,
    launchers: &[BuiltinMcpLauncher],
) -> Option<BuiltinMcpLauncher> {
    launchers
        .iter()
        .copied()
        .find(|launcher| command_exists_in_path(env_map, launcher.executable()))
}

fn matches_builtin_core_mcp_server(
    server: &McpServerConfig,
    definition: &BuiltinMcpDefinition,
) -> bool {
    let McpTransportConfig::Stdio { command, args, .. } = &server.transport else {
        return false;
    };
    // Built-in launcher preflight should not second-guess arbitrary user
    // overrides that happen to reuse a reserved built-in name.
    definition.launchers.iter().any(|launcher| {
        let (expected_command, expected_args) = launcher.render(definition.package);
        command == &expected_command && args == &expected_args
    })
}

fn render_launcher_list(launchers: &[BuiltinMcpLauncher]) -> String {
    launchers
        .iter()
        .map(|launcher| launcher.label())
        .collect::<Vec<_>>()
        .join(", ")
}

fn command_exists_in_path(env_map: &EnvMap, command: &str) -> bool {
    let candidate = Path::new(command);
    if candidate.components().count() > 1 {
        return candidate.is_file();
    }
    let Some(path) = env_map.get_raw("PATH") else {
        return false;
    };
    std::env::split_paths(path).any(|dir| executable_candidate_exists(&dir, command))
}

fn executable_candidate_exists(dir: &Path, command: &str) -> bool {
    let direct = dir.join(command);
    if direct.is_file() {
        return true;
    }
    #[cfg(windows)]
    {
        for extension in ["exe", "cmd", "bat"] {
            if dir.join(format!("{command}.{extension}")).is_file() {
                return true;
            }
        }
    }
    false
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

fn managed_plugin_path(workspace_root: &Path, plugin_id: &PluginId) -> PathBuf {
    AgentWorkspaceLayout::new(workspace_root)
        .plugins_dir()
        .join(plugin_id.as_str())
}

async fn resolve_managed_skill(workspace_root: &Path, name: &str) -> Result<ManagedSkillArtifact> {
    materialize_builtin_skills(workspace_root)?;
    let managed_root = AgentWorkspaceLayout::new(workspace_root).skills_dir();
    let active = find_managed_skill(&managed_root, name, true).await?;
    let disabled = find_managed_skill(&managed_root, name, false).await?;
    match (active, disabled) {
        (Some(skill), None) | (None, Some(skill)) => Ok(skill),
        (None, None) => find_builtin_skill(workspace_root, name)
            .await?
            .ok_or_else(|| anyhow!("unknown managed or built-in skill `{name}`")),
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
                builtin: false,
                skill_root: SkillRoot::managed(managed_root.to_path_buf()),
            });
        }
    }
    Ok(matched)
}

async fn find_builtin_skill(
    workspace_root: &Path,
    name: &str,
) -> Result<Option<ManagedSkillArtifact>> {
    let builtin_root = builtin_skill_root(workspace_root);
    let disabled_builtin = disabled_builtin_skill_names(workspace_root)?;
    let skill_dirs = collect_skill_directories(&builtin_root).await?;
    let mut matched = None;
    for skill_dir in skill_dirs {
        let skill = load_skill_from_dir(&skill_dir, &SkillRoot::external(builtin_root.clone()))
            .await
            .with_context(|| format!("failed to load built-in skill {}", skill_dir.display()))?;
        if skill.name == name {
            if matched.is_some() {
                bail!("multiple built-in skill copies matched `{name}`");
            }
            matched = Some(ManagedSkillArtifact {
                skill_name: skill.name.clone(),
                skill_path: skill_dir,
                enabled: !disabled_builtin.contains(&skill.name),
                builtin: true,
                skill_root: SkillRoot::external(builtin_root.clone()),
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

async fn collect_managed_skill_artifacts(
    managed_root: &Path,
    enabled: bool,
) -> Result<Vec<ManagedSkillArtifact>> {
    let root = if enabled {
        managed_root.to_path_buf()
    } else {
        disabled_skill_root(managed_root)
    };
    let skill_dirs = collect_skill_directories(&root).await?;
    let mut skills = Vec::with_capacity(skill_dirs.len());
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
        skills.push(ManagedSkillArtifact {
            skill_name: skill.name,
            skill_path: skill_dir,
            enabled,
            builtin: false,
            skill_root: SkillRoot::managed(managed_root.to_path_buf()),
        });
    }
    Ok(skills)
}

async fn collect_builtin_skill_artifacts(
    builtin_root: &Path,
    disabled_builtin: &BTreeSet<String>,
    excluded_names: &BTreeSet<String>,
) -> Result<Vec<ManagedSkillArtifact>> {
    let skill_dirs = collect_skill_directories(builtin_root).await?;
    let mut skills = Vec::with_capacity(skill_dirs.len());
    for skill_dir in skill_dirs {
        let skill =
            load_skill_from_dir(&skill_dir, &SkillRoot::external(builtin_root.to_path_buf()))
                .await
                .with_context(|| {
                    format!("failed to load built-in skill {}", skill_dir.display())
                })?;
        if excluded_names.contains(&skill.name) {
            continue;
        }
        skills.push(ManagedSkillArtifact {
            skill_name: skill.name.clone(),
            skill_path: skill_dir,
            enabled: !disabled_builtin.contains(&skill.name),
            builtin: true,
            skill_root: SkillRoot::external(builtin_root.to_path_buf()),
        });
    }
    Ok(skills)
}

async fn load_managed_skill_detail_from_artifact(
    artifact: ManagedSkillArtifact,
) -> Result<ManagedSkillDetail> {
    let skill = load_skill_from_dir(&artifact.skill_path, &artifact.skill_root)
        .await
        .with_context(|| format!("failed to load skill {}", artifact.skill_path.display()))?;
    Ok(ManagedSkillDetail {
        skill_name: skill.name,
        description: skill.description,
        skill_path: artifact.skill_path,
        enabled: artifact.enabled,
        builtin: artifact.builtin,
    })
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

fn load_plugin_manifest(plugin_root: &Path) -> Result<PluginManifest> {
    let manifest_path = plugin_root.join(".nanoclaw-plugin/plugin.toml");
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    toml::from_str::<PluginManifest>(&raw)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))
}

fn discover_plugin_ids(workspace_root: &Path, config: &CoreConfig) -> Result<Vec<PluginId>> {
    let mut roots = config.resolved_plugin_roots(workspace_root);
    let managed_root = AgentWorkspaceLayout::new(workspace_root).plugins_dir();
    if managed_root.exists() && !roots.iter().any(|candidate| candidate == &managed_root) {
        roots.push(managed_root);
    }
    if config.plugins.include_builtin {
        let builtin_root = workspace_root.join("builtin-plugins");
        if builtin_root.exists() && !roots.iter().any(|candidate| candidate == &builtin_root) {
            roots.push(builtin_root);
        }
    }
    let discovery = discover_plugins(&roots).with_context(|| {
        format!(
            "failed to discover plugins under {}",
            workspace_root.display()
        )
    })?;
    Ok(discovery
        .plugins
        .into_iter()
        .map(|plugin| plugin.manifest.id)
        .collect())
}

fn ensure_managed_plugin_root(config: &mut CoreConfig) {
    if !config
        .plugins
        .roots
        .iter()
        .any(|candidate| candidate == agent::NANOCLAW_PLUGINS_DIR_RELATIVE)
    {
        config
            .plugins
            .roots
            .push(agent::NANOCLAW_PLUGINS_DIR_RELATIVE.to_string());
    }
}

fn apply_plugin_enabled_override(config: &mut CoreConfig, plugin_id: &PluginId, enabled: bool) {
    config.plugins.enabled = true;
    if enabled {
        config
            .plugins
            .deny
            .retain(|candidate| candidate != plugin_id);
        if !config.plugins.allow.is_empty() && !config.plugins.allow.contains(plugin_id) {
            config.plugins.allow.push(plugin_id.clone());
        }
    }
    let entry = config
        .plugins
        .entries
        .entry(plugin_id.clone())
        .or_insert_with(PluginEntryConfig::default);
    entry.enabled = Some(enabled);
}

fn plugin_kind_label(kind: PluginKind) -> &'static str {
    match kind {
        PluginKind::Bundle => "bundle",
        PluginKind::Memory => "memory",
    }
}

fn plugin_contribution_summary(plugin: &PluginState) -> String {
    let contributions = &plugin.contributions;
    let mut parts = Vec::new();
    if contributions.instruction_count > 0 {
        parts.push(format!("instructions={}", contributions.instruction_count));
    }
    if !contributions.skill_roots.is_empty() {
        parts.push(format!("skills={}", contributions.skill_roots.len()));
    }
    if contributions.custom_tool_root_count > 0 {
        parts.push(format!(
            "custom_tools={}",
            contributions.custom_tool_root_count
        ));
    }
    if !contributions.hook_names.is_empty() {
        parts.push(format!("hooks={}", contributions.hook_names.len()));
    }
    if !contributions.mcp_servers.is_empty() {
        parts.push(format!("mcp={}", contributions.mcp_servers.len()));
    }
    if contributions.runtime_driver.is_some() {
        parts.push("runtime_driver=1".to_string());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{
        BUILTIN_CONTEXT7_SERVER, BUILTIN_PLAYWRIGHT_SERVER, add_core_mcp_server,
        add_managed_plugin, add_managed_skill, delete_core_mcp_server, delete_managed_plugin,
        delete_managed_skill, disabled_builtin_skill_names, filter_unavailable_builtin_mcp_servers,
        list_core_mcp_servers, list_managed_plugin_details, list_managed_skill_details,
        materialize_builtin_skills, set_core_mcp_server_enabled, set_managed_plugin_enabled,
        set_managed_skill_enabled,
    };
    use agent::AgentWorkspaceLayout;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent_env::EnvMap;
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
    fn list_core_mcp_servers_includes_builtin_entries_by_default() {
        let dir = tempdir().unwrap();

        let servers = list_core_mcp_servers(dir.path()).unwrap();

        assert!(
            servers
                .iter()
                .any(|server| server.name.as_str() == BUILTIN_CONTEXT7_SERVER && server.enabled)
        );
        assert!(
            servers
                .iter()
                .any(|server| server.name.as_str() == BUILTIN_PLAYWRIGHT_SERVER && server.enabled)
        );
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
    fn set_core_mcp_server_enabled_persists_builtin_override() {
        let dir = tempdir().unwrap();

        set_core_mcp_server_enabled(dir.path(), BUILTIN_CONTEXT7_SERVER, false).unwrap();
        let config = CoreConfig::load_from_dir(dir.path()).unwrap();

        assert_eq!(config.mcp_servers.len(), 1);
        assert_eq!(config.mcp_servers[0].name.as_str(), BUILTIN_CONTEXT7_SERVER);
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

    #[test]
    fn delete_core_mcp_server_rejects_builtin_entries() {
        let dir = tempdir().unwrap();

        let error = delete_core_mcp_server(dir.path(), BUILTIN_PLAYWRIGHT_SERVER).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("built-in MCP server `playwright` cannot be deleted")
        );
    }

    #[test]
    fn filtering_unavailable_builtin_mcp_servers_reports_missing_launchers() {
        let env_map = EnvMap::default();
        let servers = list_core_mcp_servers(tempdir().unwrap().path()).unwrap();
        let mut warnings = Vec::new();

        let retained = filter_unavailable_builtin_mcp_servers(&env_map, servers, &mut warnings);

        assert!(retained.is_empty());
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("built-in MCP server `context7` is enabled"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("`pnpm dlx`, `npx`, `bunx`"))
        );
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
        assert!(!artifact.builtin);
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
        assert!(!disabled.builtin);
        assert!(disabled.skill_path.ends_with(".disabled/review"));

        let enabled = set_managed_skill_enabled(workspace.path(), "review", true)
            .await
            .unwrap();
        assert!(enabled.enabled);
        assert!(!enabled.builtin);
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

    #[tokio::test]
    async fn list_managed_skill_details_includes_description() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review");
        write_skill_source(&source, "review");
        add_managed_skill(workspace.path(), &source).await.unwrap();

        let details = list_managed_skill_details(workspace.path()).await.unwrap();
        let review = details
            .iter()
            .find(|detail| detail.skill_name == "review")
            .expect("managed review skill");

        assert!(details.len() >= 1);
        assert_eq!(review.description, "Demo skill");
        assert!(review.enabled);
        assert!(!review.builtin);
    }

    #[tokio::test]
    async fn list_managed_skill_details_includes_builtin_skills_by_default() {
        let workspace = tempdir().unwrap();
        materialize_builtin_skills(workspace.path()).unwrap();

        let details = list_managed_skill_details(workspace.path()).await.unwrap();

        assert!(details.iter().any(|detail| {
            detail.skill_name == "frontend-skill" && detail.builtin && detail.enabled
        }));
        assert!(details.iter().any(|detail| {
            detail.skill_name == "skill-creator" && detail.builtin && detail.enabled
        }));
    }

    #[tokio::test]
    async fn disabling_builtin_skill_persists_disabled_list() {
        let workspace = tempdir().unwrap();
        materialize_builtin_skills(workspace.path()).unwrap();

        let disabled = set_managed_skill_enabled(workspace.path(), "frontend-skill", false)
            .await
            .unwrap();
        let details = list_managed_skill_details(workspace.path()).await.unwrap();

        assert!(disabled.builtin);
        assert!(!disabled.enabled);
        assert!(
            disabled
                .skill_path
                .ends_with(".nanoclaw/apps/code-agent/builtin-skills/frontend-skill")
        );
        assert!(
            disabled_builtin_skill_names(workspace.path())
                .unwrap()
                .contains("frontend-skill")
        );
        assert!(details.iter().any(|detail| {
            detail.skill_name == "frontend-skill" && detail.builtin && !detail.enabled
        }));
    }

    #[tokio::test]
    async fn delete_managed_skill_rejects_builtin_entries() {
        let workspace = tempdir().unwrap();
        materialize_builtin_skills(workspace.path()).unwrap();

        let error = delete_managed_skill(workspace.path(), "frontend-skill")
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("built-in skill `frontend-skill` cannot be deleted")
        );
    }

    #[tokio::test]
    async fn builtin_skills_yield_to_managed_name_collisions_in_management_views() {
        let workspace = tempdir().unwrap();
        materialize_builtin_skills(workspace.path()).unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("code-review");
        write_skill_source(&source, "frontend-skill");
        add_managed_skill(workspace.path(), &source).await.unwrap();

        let details = list_managed_skill_details(workspace.path()).await.unwrap();

        let matches = details
            .iter()
            .filter(|detail| detail.skill_name == "frontend-skill")
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1);
        assert!(!matches[0].builtin);
        assert_eq!(
            matches[0].skill_path,
            AgentWorkspaceLayout::new(workspace.path())
                .skills_dir()
                .join("frontend-skill")
        );
    }

    fn write_plugin_source(dir: &Path, id: &str) {
        std::fs::create_dir_all(dir.join(".nanoclaw-plugin")).unwrap();
        std::fs::write(
            dir.join(".nanoclaw-plugin/plugin.toml"),
            format!("id = \"{id}\"\nkind = \"bundle\"\nenabled_by_default = true\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn add_managed_plugin_copies_into_workspace_root_and_persists_root() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review-policy");
        write_plugin_source(&source, "review-policy");

        let artifact = add_managed_plugin(workspace.path(), &source).await.unwrap();
        let config = CoreConfig::load_from_dir(workspace.path()).unwrap();

        assert_eq!(artifact.plugin_id.as_str(), "review-policy");
        assert!(artifact.enabled);
        assert!(
            artifact
                .plugin_path
                .join(".nanoclaw-plugin/plugin.toml")
                .is_file()
        );
        assert!(
            config
                .plugins
                .roots
                .iter()
                .any(|root| root == agent::NANOCLAW_PLUGINS_DIR_RELATIVE)
        );
        assert_eq!(config.plugins.entries["review-policy"].enabled, Some(true));
    }

    #[test]
    fn set_managed_plugin_enabled_persists_entry_override() {
        let workspace = tempdir().unwrap();
        let managed_root = AgentWorkspaceLayout::new(workspace.path()).plugins_dir();
        let plugin_dir = managed_root.join("review-policy");
        write_plugin_source(&plugin_dir, "review-policy");
        let mut config = CoreConfig::default();
        config
            .plugins
            .roots
            .push(agent::NANOCLAW_PLUGINS_DIR_RELATIVE.to_string());
        super::write_raw_core_config(workspace.path(), &config).unwrap();

        set_managed_plugin_enabled(workspace.path(), "review-policy", false).unwrap();
        let config = CoreConfig::load_from_dir(workspace.path()).unwrap();

        assert_eq!(config.plugins.entries["review-policy"].enabled, Some(false));
    }

    #[tokio::test]
    async fn delete_managed_plugin_removes_files_and_config_references() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review-policy");
        write_plugin_source(&source, "review-policy");
        add_managed_plugin(workspace.path(), &source).await.unwrap();

        let artifact = delete_managed_plugin(workspace.path(), "review-policy")
            .await
            .unwrap();
        let config = CoreConfig::load_from_dir(workspace.path()).unwrap();

        assert_eq!(artifact.plugin_id.as_str(), "review-policy");
        assert!(!artifact.plugin_path.exists());
        assert!(!config.plugins.entries.contains_key("review-policy"));
    }

    #[tokio::test]
    async fn list_managed_plugin_details_reports_effective_enablement() {
        let workspace = tempdir().unwrap();
        let source_root = tempdir().unwrap();
        let source = source_root.path().join("review-policy");
        write_plugin_source(&source, "review-policy");
        add_managed_plugin(workspace.path(), &source).await.unwrap();
        set_managed_plugin_enabled(workspace.path(), "review-policy", false).unwrap();

        let details = list_managed_plugin_details(workspace.path()).unwrap();

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].plugin_id.as_str(), "review-policy");
        assert_eq!(details[0].kind, "bundle");
        assert!(!details[0].enabled);
        assert!(details[0].reason.contains("disabled"));
    }
}
