//! App-specific config loading for the code-agent example.
//!
//! Core runtime config comes from `nanoclaw-config`. This module owns only the
//! code-agent-specific host settings layered on top of the shared core config
//! surface.

pub use code_agent_contracts::{motion, statusline, theme};

use crate::motion::{TuiMotionConfig, TuiMotionField};
use crate::statusline::StatusLineConfig;
use crate::theme::{ThemeCatalog, load_theme_catalog};
use agent::types::{McpToolBoundaryClass, McpTransportKind};
use agent_env::{EnvMap, EnvVar};
use anyhow::{Context, Result};
use nanoclaw_config::{CoreConfig, load_optional_app_config};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, value};

const CODE_AGENT_APP_NAME: &str = "code-agent";
const CODE_AGENT_APP_CONFIG_PATH: &str = ".nanoclaw/apps/code-agent.toml";
const CODE_AGENT_LSP_ENABLED: EnvVar = EnvVar::new(
    "CODE_AGENT_LSP_ENABLED",
    "Whether code-agent should enable managed LSP-backed code-intel with lexical fallback.",
);
const CODE_AGENT_LSP_AUTO_INSTALL: EnvVar = EnvVar::new(
    "CODE_AGENT_LSP_AUTO_INSTALL",
    "Whether code-agent may auto-install supported LSP servers into the managed workspace cache.",
);
const CODE_AGENT_LSP_INSTALL_ROOT: EnvVar = EnvVar::new(
    "CODE_AGENT_LSP_INSTALL_ROOT",
    "Optional override for the managed LSP install/cache directory used by code-agent.",
);

#[derive(Clone, Debug)]
pub struct CodeAgentConfig {
    pub core: CoreConfig,
    pub lsp_enabled: bool,
    pub lsp_auto_install: bool,
    pub lsp_install_root: Option<PathBuf>,
    pub approval_policy: CodeAgentApprovalPolicyConfig,
    pub statusline: StatusLineConfig,
    pub motion: TuiMotionConfig,
    pub theme_catalog: ThemeCatalog,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeAgentApprovalPolicyConfig {
    pub default_mode: Option<CodeAgentApprovalRuleEffect>,
    pub rules: Vec<CodeAgentApprovalRule>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeAgentApprovalRuleEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeAgentApprovalRule {
    pub effect: CodeAgentApprovalRuleEffect,
    pub reason: Option<String>,
    pub tool_names: BTreeSet<String>,
    pub origins: Vec<CodeAgentApprovalOriginMatcher>,
    pub sources: Vec<CodeAgentApprovalSourceMatcher>,
    pub mcp_boundary: Option<CodeAgentApprovalMcpBoundaryMatcher>,
    pub exec: Option<ExecApprovalRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeAgentApprovalOriginMatcher {
    Local,
    McpServer(String),
    Provider(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeAgentApprovalSourceMatcher {
    Builtin,
    Dynamic,
    Plugin,
    McpTool,
    McpResource,
    ProviderBuiltin(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeAgentApprovalMcpBoundaryMatcher {
    pub transports: Vec<McpTransportKind>,
    pub boundary_classes: Vec<McpToolBoundaryClass>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecApprovalRule {
    ArgvExact(Vec<String>),
    ArgvPrefix(Vec<String>),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentAppConfig {
    lsp: CodeAgentLspConfig,
    approval: CodeAgentApprovalConfig,
    tui: CodeAgentTuiConfig,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct CodeAgentApprovalConfig {
    default_mode: Option<CodeAgentApprovalRuleEffect>,
    rules: Vec<CodeAgentApprovalRuleConfig>,
    auto_allow_builtin_local_tool_names: Vec<String>,
    auto_allow_local_stdio_mcp_resource_reads: bool,
    exec: CodeAgentExecApprovalConfig,
}

impl Default for CodeAgentApprovalConfig {
    fn default() -> Self {
        Self {
            default_mode: None,
            rules: Vec::new(),
            auto_allow_builtin_local_tool_names: vec![
                "web_search".to_string(),
                "web_fetch".to_string(),
            ],
            auto_allow_local_stdio_mcp_resource_reads: true,
            exec: CodeAgentExecApprovalConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentExecApprovalConfig {
    rules: Vec<CodeAgentExecApprovalRuleConfig>,
    always_approve_simple_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentExecApprovalRuleConfig {
    argv_exact: Vec<String>,
    argv_prefix: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentApprovalRuleConfig {
    effect: Option<CodeAgentApprovalRuleEffect>,
    reason: Option<String>,
    tool_names: Vec<String>,
    origins: Vec<String>,
    sources: Vec<String>,
    mcp_boundary: Option<CodeAgentApprovalMcpBoundaryMatcherConfig>,
    exec: Option<CodeAgentExecApprovalRuleConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentApprovalMcpBoundaryMatcherConfig {
    transports: Vec<McpTransportKind>,
    boundary_classes: Vec<McpToolBoundaryClass>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeAgentTuiConfig {
    statusline: StatusLineConfig,
    motion: TuiMotionConfig,
    theme: Option<String>,
    theme_file: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct CodeAgentLspConfig {
    enabled: bool,
    auto_install: bool,
    install_root: Option<String>,
}

impl Default for CodeAgentLspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_install: false,
            install_root: None,
        }
    }
}

impl CodeAgentConfig {
    pub fn load_from_dir(workspace_root: &Path, env_map: &EnvMap) -> Result<Self> {
        let core = CoreConfig::load_from_dir(workspace_root)?;
        let mut app =
            load_optional_app_config::<CodeAgentAppConfig>(workspace_root, CODE_AGENT_APP_NAME)?;
        if let Some(parsed) = env_map.get_bool_var(CODE_AGENT_LSP_ENABLED) {
            app.lsp.enabled = parsed;
        }
        if let Some(parsed) = env_map.get_bool_var(CODE_AGENT_LSP_AUTO_INSTALL) {
            app.lsp.auto_install = parsed;
        }
        if let Some(value) = env_map.get_non_empty_var(CODE_AGENT_LSP_INSTALL_ROOT) {
            app.lsp.install_root = Some(value);
        }

        Ok(Self {
            core,
            lsp_enabled: app.lsp.enabled,
            lsp_auto_install: app.lsp.auto_install,
            lsp_install_root: app
                .lsp
                .install_root
                .as_deref()
                .map(|value| resolve_path(workspace_root, value)),
            approval_policy: normalize_approval_policy(app.approval)?,
            statusline: app.tui.statusline,
            motion: app.tui.motion,
            theme_catalog: load_theme_catalog(
                workspace_root,
                app.tui.theme_file.as_deref(),
                app.tui.theme.as_deref(),
            )?,
        })
    }
}

pub fn persist_tui_theme_selection(workspace_root: &Path, theme_id: &str) -> Result<()> {
    let path = workspace_root.join(CODE_AGENT_APP_CONFIG_PATH);
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
    // Update only the targeted key so existing comments and formatting survive.
    let mut document = if raw.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw.parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display()))?
    };
    let root = document.as_table_mut();
    let tui_item = root.entry("tui").or_insert(Item::Table(Table::new()));
    if !tui_item.is_table() {
        *tui_item = Item::Table(Table::new());
    }
    tui_item
        .as_table_mut()
        .expect("tui config must be a TOML table")["theme"] = value(theme_id);

    let mut serialized = document.to_string();
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    std::fs::write(&path, serialized).with_context(|| format!("failed to write {}", path.display()))
}

pub fn persist_tui_motion_selection(
    workspace_root: &Path,
    field: TuiMotionField,
    enabled: bool,
) -> Result<()> {
    let path = workspace_root.join(CODE_AGENT_APP_CONFIG_PATH);
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
    let root = document.as_table_mut();
    let tui_item = root.entry("tui").or_insert(Item::Table(Table::new()));
    if !tui_item.is_table() {
        *tui_item = Item::Table(Table::new());
    }
    let tui = tui_item
        .as_table_mut()
        .expect("tui config must be a TOML table");
    let motion_item = tui.entry("motion").or_insert(Item::Table(Table::new()));
    if !motion_item.is_table() {
        *motion_item = Item::Table(Table::new());
    }
    let motion = motion_item
        .as_table_mut()
        .expect("tui.motion config must be a TOML table");
    match field {
        TuiMotionField::TranscriptCellIntro => {
            motion["transcript_cell_intro"] = value(enabled);
        }
    }

    let mut serialized = document.to_string();
    if !serialized.ends_with('\n') {
        serialized.push('\n');
    }
    std::fs::write(&path, serialized).with_context(|| format!("failed to write {}", path.display()))
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn normalize_exec_approval_rules(
    rules: Vec<CodeAgentExecApprovalRuleConfig>,
    legacy_prefixes: Vec<String>,
) -> Result<Vec<ExecApprovalRule>> {
    let mut normalized = Vec::new();
    for rule in rules {
        let exact = normalize_exec_approval_tokens(rule.argv_exact);
        let prefix = normalize_exec_approval_tokens(rule.argv_prefix);
        match (!exact.is_empty(), !prefix.is_empty()) {
            (true, false) => normalized.push(ExecApprovalRule::ArgvExact(exact)),
            (false, true) => normalized.push(ExecApprovalRule::ArgvPrefix(prefix)),
            (false, false) => {
                anyhow::bail!(
                    "approval.exec.rules entries require either argv_exact or argv_prefix"
                )
            }
            (true, true) => {
                anyhow::bail!(
                    "approval.exec.rules entries must set only one of argv_exact or argv_prefix"
                )
            }
        }
    }

    for legacy in legacy_prefixes {
        let trimmed = legacy.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(argv) = shlex::split(trimmed) else {
            anyhow::bail!(
                "approval.exec.always_approve_simple_prefixes entry `{trimmed}` is not valid shell tokenization"
            );
        };
        let argv = normalize_exec_approval_tokens(argv);
        if !argv.is_empty() {
            normalized.push(ExecApprovalRule::ArgvPrefix(argv));
        }
    }

    Ok(normalized)
}

fn normalize_approval_policy(
    config: CodeAgentApprovalConfig,
) -> Result<CodeAgentApprovalPolicyConfig> {
    let mut rules = normalize_explicit_approval_rules(config.rules)?;

    let auto_allow_builtin_local_tool_names =
        normalize_auto_allow_tool_names(config.auto_allow_builtin_local_tool_names);
    if !auto_allow_builtin_local_tool_names.is_empty() {
        rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("code-agent built-in local allowlist".to_string()),
            tool_names: auto_allow_builtin_local_tool_names,
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: None,
        });
    }

    if config.auto_allow_local_stdio_mcp_resource_reads {
        rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("code-agent local stdio MCP resource allowlist".to_string()),
            tool_names: ["read_mcp_resource".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::McpServer("*".to_string())],
            sources: vec![CodeAgentApprovalSourceMatcher::McpResource],
            mcp_boundary: Some(CodeAgentApprovalMcpBoundaryMatcher {
                transports: vec![McpTransportKind::Stdio],
                boundary_classes: vec![McpToolBoundaryClass::LocalProcess],
            }),
            exec: None,
        });
    }

    for exec_rule in normalize_exec_approval_rules(
        config.exec.rules,
        config.exec.always_approve_simple_prefixes,
    )? {
        rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("code-agent exec argv allowlist".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(exec_rule),
        });
    }

    Ok(CodeAgentApprovalPolicyConfig {
        default_mode: config.default_mode,
        rules,
    })
}

fn normalize_explicit_approval_rules(
    rules: Vec<CodeAgentApprovalRuleConfig>,
) -> Result<Vec<CodeAgentApprovalRule>> {
    rules
        .into_iter()
        .map(normalize_explicit_approval_rule)
        .collect()
}

fn normalize_explicit_approval_rule(
    rule: CodeAgentApprovalRuleConfig,
) -> Result<CodeAgentApprovalRule> {
    let Some(effect) = rule.effect else {
        anyhow::bail!("approval.rules entries require an effect");
    };
    let tool_names = normalize_auto_allow_tool_names(rule.tool_names);
    let origins = rule
        .origins
        .into_iter()
        .map(|value| parse_approval_origin_matcher(&value))
        .collect::<Result<Vec<_>>>()?;
    let sources = rule
        .sources
        .into_iter()
        .map(|value| parse_approval_source_matcher(&value))
        .collect::<Result<Vec<_>>>()?;
    let mcp_boundary = rule
        .mcp_boundary
        .map(|value| CodeAgentApprovalMcpBoundaryMatcher {
            transports: value.transports,
            boundary_classes: value.boundary_classes,
        });
    let exec = match rule.exec {
        Some(exec) => Some(normalize_exec_approval_rule(exec)?),
        None => None,
    };

    let mut tool_names = tool_names;
    let mut origins = origins;
    let mut sources = sources;

    if exec.is_some() {
        if tool_names.is_empty() {
            tool_names.insert("exec_command".to_string());
        }
        if tool_names.iter().any(|value| value != "exec_command") {
            anyhow::bail!("approval.rules exec matcher can only target exec_command");
        }
        if origins.is_empty() {
            origins.push(CodeAgentApprovalOriginMatcher::Local);
        }
        if sources.is_empty() {
            sources.push(CodeAgentApprovalSourceMatcher::Builtin);
        }
    }

    if tool_names.is_empty()
        && origins.is_empty()
        && sources.is_empty()
        && mcp_boundary.is_none()
        && exec.is_none()
    {
        anyhow::bail!(
            "approval.rules entries must include at least one matcher or use approval.default_mode"
        );
    }

    Ok(CodeAgentApprovalRule {
        effect,
        reason: rule
            .reason
            .map(|value| value.trim().to_string())
            .filter(|v| !v.is_empty()),
        tool_names,
        origins,
        sources,
        mcp_boundary,
        exec,
    })
}

fn normalize_exec_approval_rule(rule: CodeAgentExecApprovalRuleConfig) -> Result<ExecApprovalRule> {
    let exact = normalize_exec_approval_tokens(rule.argv_exact);
    let prefix = normalize_exec_approval_tokens(rule.argv_prefix);
    match (!exact.is_empty(), !prefix.is_empty()) {
        (true, false) => Ok(ExecApprovalRule::ArgvExact(exact)),
        (false, true) => Ok(ExecApprovalRule::ArgvPrefix(prefix)),
        (false, false) => {
            anyhow::bail!("approval exec matcher requires either argv_exact or argv_prefix")
        }
        (true, true) => {
            anyhow::bail!("approval exec matcher must set only one of argv_exact or argv_prefix")
        }
    }
}

fn parse_approval_origin_matcher(value: &str) -> Result<CodeAgentApprovalOriginMatcher> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("local") {
        return Ok(CodeAgentApprovalOriginMatcher::Local);
    }
    if let Some(server_name) = value.strip_prefix("mcp:") {
        let server_name = server_name.trim();
        if server_name.is_empty() {
            anyhow::bail!("approval origin matcher `mcp:` requires a server name");
        }
        return Ok(CodeAgentApprovalOriginMatcher::McpServer(
            server_name.to_string(),
        ));
    }
    if let Some(provider) = value.strip_prefix("provider:") {
        let provider = provider.trim();
        if provider.is_empty() {
            anyhow::bail!("approval origin matcher `provider:` requires a provider name");
        }
        return Ok(CodeAgentApprovalOriginMatcher::Provider(
            provider.to_string(),
        ));
    }
    anyhow::bail!("unsupported approval origin matcher `{value}`")
}

fn parse_approval_source_matcher(value: &str) -> Result<CodeAgentApprovalSourceMatcher> {
    let value = value.trim();
    match value {
        "builtin" => Ok(CodeAgentApprovalSourceMatcher::Builtin),
        "dynamic" => Ok(CodeAgentApprovalSourceMatcher::Dynamic),
        "plugin" => Ok(CodeAgentApprovalSourceMatcher::Plugin),
        "mcp_tool" => Ok(CodeAgentApprovalSourceMatcher::McpTool),
        "mcp_resource" => Ok(CodeAgentApprovalSourceMatcher::McpResource),
        _ => {
            if let Some(provider) = value.strip_prefix("provider_builtin:") {
                let provider = provider.trim();
                if provider.is_empty() {
                    anyhow::bail!(
                        "approval source matcher `provider_builtin:` requires a provider name"
                    );
                }
                Ok(CodeAgentApprovalSourceMatcher::ProviderBuiltin(
                    provider.to_string(),
                ))
            } else {
                anyhow::bail!("unsupported approval source matcher `{value}`")
            }
        }
    }
}

fn normalize_auto_allow_tool_names(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_exec_approval_tokens(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CODE_AGENT_APP_CONFIG_PATH, CodeAgentApprovalOriginMatcher, CodeAgentApprovalPolicyConfig,
        CodeAgentApprovalRule, CodeAgentApprovalRuleEffect, CodeAgentApprovalSourceMatcher,
        CodeAgentConfig, ExecApprovalRule,
    };
    use agent_env::EnvMap;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;
    use toml_edit::DocumentMut;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn loads_lsp_flags_from_env() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            format!(
                "OPENAI_API_KEY=test-key\nCODE_AGENT_LSP_ENABLED=false\nCODE_AGENT_LSP_AUTO_INSTALL=true\nCODE_AGENT_LSP_INSTALL_ROOT={}\n",
                dir.path().join(".cache/lsp").display()
            ),
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert!(!config.lsp_enabled);
        assert!(config.lsp_auto_install);
        assert_eq!(config.lsp_install_root, Some(dir.path().join(".cache/lsp")));
    }

    #[tokio::test]
    async fn loads_statusline_flags_from_app_config() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"
                [tui.statusline]
                model = false
                repo = true
                branch = false
                clock = false
                session = true
            "#,
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert!(!config.statusline.model);
        assert!(config.statusline.repo);
        assert!(!config.statusline.branch);
        assert!(!config.statusline.clock);
        assert!(config.statusline.session);
    }

    #[tokio::test]
    async fn loads_motion_flags_from_app_config() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"
                [tui.motion]
                transcript_cell_intro = false
            "#,
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert!(!config.motion.transcript_cell_intro);
    }

    #[tokio::test]
    async fn loads_exec_approval_rules_from_app_config() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"
                [approval]
                default_mode = "ask"
                auto_allow_builtin_local_tool_names = [" web_search ", "web_fetch", ""]
                auto_allow_local_stdio_mcp_resource_reads = false

                [[approval.rules]]
                effect = "deny"
                reason = "block dangerous pushes"
                tool_names = ["exec_command"]

                [approval.rules.exec]
                argv_exact = ["git", "push"]

                [approval.exec]
                always_approve_simple_prefixes = [" git diff --stat ", ""]

                [[approval.exec.rules]]
                argv_prefix = ["git", "status"]

                [[approval.exec.rules]]
                argv_exact = ["cargo", "test", "-p", "store"]
            "#,
        )
        .unwrap();

        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert_eq!(
            config.approval_policy,
            CodeAgentApprovalPolicyConfig {
                default_mode: Some(CodeAgentApprovalRuleEffect::Ask),
                rules: vec![
                    CodeAgentApprovalRule {
                        effect: CodeAgentApprovalRuleEffect::Deny,
                        reason: Some("block dangerous pushes".to_string()),
                        tool_names: ["exec_command".to_string()].into_iter().collect(),
                        origins: vec![CodeAgentApprovalOriginMatcher::Local],
                        sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                        mcp_boundary: None,
                        exec: Some(ExecApprovalRule::ArgvExact(vec![
                            "git".to_string(),
                            "push".to_string()
                        ])),
                    },
                    CodeAgentApprovalRule {
                        effect: CodeAgentApprovalRuleEffect::Allow,
                        reason: Some("code-agent built-in local allowlist".to_string()),
                        tool_names: ["web_fetch".to_string(), "web_search".to_string()]
                            .into_iter()
                            .collect(),
                        origins: vec![CodeAgentApprovalOriginMatcher::Local],
                        sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                        mcp_boundary: None,
                        exec: None,
                    },
                    CodeAgentApprovalRule {
                        effect: CodeAgentApprovalRuleEffect::Allow,
                        reason: Some("code-agent exec argv allowlist".to_string()),
                        tool_names: ["exec_command".to_string()].into_iter().collect(),
                        origins: vec![CodeAgentApprovalOriginMatcher::Local],
                        sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                        mcp_boundary: None,
                        exec: Some(ExecApprovalRule::ArgvPrefix(vec![
                            "git".to_string(),
                            "status".to_string()
                        ])),
                    },
                    CodeAgentApprovalRule {
                        effect: CodeAgentApprovalRuleEffect::Allow,
                        reason: Some("code-agent exec argv allowlist".to_string()),
                        tool_names: ["exec_command".to_string()].into_iter().collect(),
                        origins: vec![CodeAgentApprovalOriginMatcher::Local],
                        sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                        mcp_boundary: None,
                        exec: Some(ExecApprovalRule::ArgvExact(vec![
                            "cargo".to_string(),
                            "test".to_string(),
                            "-p".to_string(),
                            "store".to_string()
                        ])),
                    },
                    CodeAgentApprovalRule {
                        effect: CodeAgentApprovalRuleEffect::Allow,
                        reason: Some("code-agent exec argv allowlist".to_string()),
                        tool_names: ["exec_command".to_string()].into_iter().collect(),
                        origins: vec![CodeAgentApprovalOriginMatcher::Local],
                        sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                        mcp_boundary: None,
                        exec: Some(ExecApprovalRule::ArgvPrefix(vec![
                            "git".to_string(),
                            "diff".to_string(),
                            "--stat".to_string()
                        ])),
                    }
                ]
            }
        );
    }

    #[tokio::test]
    async fn loads_theme_catalog_from_configured_theme_file() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"
                [tui]
                theme = "paper"
                theme_file = ".nanoclaw/apps/code-agent-themes.toml"
            "#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("code-agent-themes.toml"),
            r##"
                active = "paper"

                [themes.paper]
                summary = "light paper"
                bg = "#faf6ef"
                main_bg = "#f5f0e7"
                footer_bg = "#efe8de"
                bottom_pane_bg = "#e7dfd2"
                border_active = "#8b8175"
                text = "#2b241d"
                muted = "#6f665d"
                subtle = "#9d9388"
                accent = "#2f7c82"
                user = "#9a6a2f"
                assistant = "#3c7c56"
                error = "#b4554f"
                warn = "#b37a21"
                header = "#17120d"
            "##,
        )
        .unwrap();

        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let config = CodeAgentConfig::load_from_dir(dir.path(), &env_map).unwrap();

        assert_eq!(config.theme_catalog.active_theme, "paper");
        assert!(
            config
                .theme_catalog
                .themes
                .iter()
                .any(|theme| theme.id == "paper")
        );
        assert!(
            config
                .theme_catalog
                .themes
                .iter()
                .any(|theme| theme.id == "graphite")
        );
    }

    #[test]
    fn persists_tui_theme_selection_into_new_app_config() {
        let dir = tempdir().unwrap();

        super::persist_tui_theme_selection(dir.path(), "paper").unwrap();

        let raw = std::fs::read_to_string(dir.path().join(CODE_AGENT_APP_CONFIG_PATH)).unwrap();
        assert!(raw.contains("theme = \"paper\""));
    }

    #[test]
    fn persists_tui_theme_selection_without_clobbering_other_tui_settings() {
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"
                [tui]
                theme_file = ".nanoclaw/apps/code-agent-themes.toml"

                [tui.statusline]
                model = false
            "#,
        )
        .unwrap();

        super::persist_tui_theme_selection(dir.path(), "glacier").unwrap();

        let raw = std::fs::read_to_string(app_dir.join("code-agent.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&raw).unwrap();
        assert_eq!(parsed["tui"]["theme"].as_str(), Some("glacier"));
        assert_eq!(
            parsed["tui"]["theme_file"].as_str(),
            Some(".nanoclaw/apps/code-agent-themes.toml")
        );
        assert_eq!(parsed["tui"]["statusline"]["model"].as_bool(), Some(false));
    }

    #[test]
    fn persists_tui_theme_selection_without_dropping_comments() {
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("code-agent.toml"),
            r#"# keep the app comment
[tui]
# keep the theme source note
theme_file = ".nanoclaw/apps/code-agent-themes.toml"
"#,
        )
        .unwrap();

        super::persist_tui_theme_selection(dir.path(), "graphite").unwrap();

        let raw = std::fs::read_to_string(app_dir.join("code-agent.toml")).unwrap();
        assert!(raw.contains("# keep the app comment"));
        assert!(raw.contains("# keep the theme source note"));
        assert!(raw.contains("theme = \"graphite\""));
    }

    #[test]
    fn persists_tui_motion_selection_without_clobbering_other_tui_settings() {
        let dir = tempdir().unwrap();
        let app_dir = dir.path().join(".nanoclaw/apps");
        std::fs::create_dir_all(&app_dir).unwrap();
        let path = app_dir.join("code-agent.toml");
        std::fs::write(
            &path,
            r#"
                [tui]
                theme = "fjord"

                [tui.statusline]
                model = false
            "#,
        )
        .unwrap();

        super::persist_tui_motion_selection(
            dir.path(),
            crate::motion::TuiMotionField::TranscriptCellIntro,
            false,
        )
        .unwrap();

        let parsed = std::fs::read_to_string(&path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        assert_eq!(parsed["tui"]["theme"].as_str(), Some("fjord"));
        assert_eq!(parsed["tui"]["statusline"]["model"].as_bool(), Some(false));
        assert_eq!(
            parsed["tui"]["motion"]["transcript_cell_intro"].as_bool(),
            Some(false)
        );
    }
}
