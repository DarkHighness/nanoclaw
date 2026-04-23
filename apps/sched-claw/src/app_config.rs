use agent_env::EnvMap;
use anyhow::Result;
use nanoclaw_config::{CoreConfig, ResolvedAgentProfile, load_optional_app_config};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub use sched_claw_daemon_core::daemon_client::DaemonClientConfig;
pub use sched_claw_domain::paths::{APP_NAME, app_state_dir};
const DEFAULT_DAEMON_TIMEOUT_MS: u64 = 30_000;
const MAX_DAEMON_TIMEOUT_MS: u64 = 5 * 60_000;

#[derive(Clone, Debug, Default)]
pub struct CliOverrides {
    pub system_prompt: Option<String>,
    pub skill_roots: Vec<PathBuf>,
    pub daemon_socket: Option<PathBuf>,
    pub sandbox_fail_if_unavailable: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct SchedClawConfig {
    pub env_map: EnvMap,
    pub core: CoreConfig,
    pub primary_profile: ResolvedAgentProfile,
    pub skill_roots: Vec<PathBuf>,
    pub disabled_builtin_skills: BTreeSet<String>,
    pub disabled_tools: BTreeSet<String>,
    pub daemon: DaemonClientConfig,
    pub workspace_only: bool,
    pub sandbox_fail_if_unavailable: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct SchedClawAppConfig {
    skills: SkillConfig,
    tools: ToolConfig,
    daemon: DaemonConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct SkillConfig {
    disabled_builtin: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct ToolConfig {
    disabled: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct DaemonConfig {
    socket: Option<String>,
    request_timeout_ms: Option<u64>,
}

impl SchedClawConfig {
    pub fn load_from_dir(workspace_root: &Path, overrides: &CliOverrides) -> Result<Self> {
        let env_map = EnvMap::from_workspace_dir(workspace_root)?;
        let core = CoreConfig::load_from_dir(workspace_root)?;
        let app = load_optional_app_config::<SchedClawAppConfig>(workspace_root, APP_NAME)?;
        let mut primary_profile = core.resolve_primary_agent()?;
        if let Some(system_prompt) = overrides
            .system_prompt
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            primary_profile.system_prompt = Some(system_prompt.to_string());
        }

        let mut skill_roots = core.resolved_skill_roots(workspace_root);
        skill_roots.extend(overrides.skill_roots.iter().cloned());
        dedup_paths(&mut skill_roots);

        let socket_path = overrides
            .daemon_socket
            .clone()
            .or_else(|| {
                app.daemon
                    .socket
                    .as_deref()
                    .map(|value| resolve_path(workspace_root, value))
            })
            .unwrap_or_else(|| default_daemon_socket_path(workspace_root));
        let request_timeout_ms = app
            .daemon
            .request_timeout_ms
            .unwrap_or(DEFAULT_DAEMON_TIMEOUT_MS)
            .clamp(1_000, MAX_DAEMON_TIMEOUT_MS);
        let sandbox_fail_if_unavailable = overrides
            .sandbox_fail_if_unavailable
            .unwrap_or(core.host.sandbox_fail_if_unavailable);

        Ok(Self {
            env_map,
            core: core.clone(),
            primary_profile,
            skill_roots,
            disabled_builtin_skills: normalize_string_set(app.skills.disabled_builtin),
            disabled_tools: normalize_string_set(app.tools.disabled),
            daemon: DaemonClientConfig {
                socket_path,
                request_timeout_ms,
            },
            workspace_only: core.host.workspace_only,
            sandbox_fail_if_unavailable,
        })
    }
}

pub fn default_daemon_socket_path(workspace_root: &Path) -> PathBuf {
    app_state_dir(workspace_root).join("sched-claw.sock")
}

fn normalize_string_set(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn dedup_paths(values: &mut Vec<PathBuf>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn resolve_path(workspace_root: &Path, value: &str) -> PathBuf {
    let candidate = PathBuf::from(value.trim());
    if candidate.is_absolute() {
        candidate
    } else {
        workspace_root.join(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::{CliOverrides, SchedClawConfig, app_state_dir, default_daemon_socket_path};
    use tempfile::tempdir;

    #[test]
    fn defaults_socket_into_app_state_dir() {
        let dir = tempdir().unwrap();

        let config = SchedClawConfig::load_from_dir(dir.path(), &CliOverrides::default()).unwrap();

        assert_eq!(
            config.daemon.socket_path,
            default_daemon_socket_path(dir.path())
        );
        assert_eq!(
            app_state_dir(dir.path()),
            dir.path().join(".nanoclaw/apps/sched-claw")
        );
    }
}
