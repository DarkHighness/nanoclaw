use crate::options::AppOptions;
use agent::ToolExecutionContext;
use agent::tools::{
    SandboxBackendStatus, SandboxPolicy, describe_sandbox_policy, sandbox_backend_status,
};
use agent_env::EnvMap;
use nanoclaw_config::{AgentSandboxMode, ResolvedAgentProfile};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SandboxPreflight {
    pub(crate) policy: SandboxPolicy,
    pub(crate) status: SandboxBackendStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SandboxFallbackNotice {
    pub(crate) policy_summary: String,
    pub(crate) reason: String,
    pub(crate) risk_summary: String,
    pub(crate) setup_steps: Vec<String>,
}

pub(crate) fn build_sandbox_policy(
    options: &AppOptions,
    tool_context: &ToolExecutionContext,
) -> SandboxPolicy {
    let base_policy = tool_context.sandbox_scope().recommended_policy();
    match options.primary_profile.sandbox {
        AgentSandboxMode::DangerFullAccess => SandboxPolicy::permissive()
            .with_fail_if_unavailable(options.sandbox_fail_if_unavailable),
        AgentSandboxMode::WorkspaceWrite => {
            base_policy.with_fail_if_unavailable(options.sandbox_fail_if_unavailable)
        }
        AgentSandboxMode::ReadOnly => SandboxPolicy {
            mode: agent::tools::SandboxMode::ReadOnly,
            filesystem: agent::tools::FilesystemPolicy {
                readable_roots: base_policy.filesystem.readable_roots,
                writable_roots: Vec::new(),
                executable_roots: base_policy.filesystem.executable_roots,
                protected_paths: base_policy.filesystem.protected_paths,
            },
            network: match base_policy.network {
                agent::tools::NetworkPolicy::Full => agent::tools::NetworkPolicy::Off,
                other => other,
            },
            host_escape: agent::tools::HostEscapePolicy::Deny,
            fail_if_unavailable: options.sandbox_fail_if_unavailable,
        },
    }
}

pub(crate) fn inject_process_env(env_map: &EnvMap) {
    // This runs before the Tokio runtime starts, so mutating process env is safe here.
    env_map.apply_to_process();
}

pub(crate) fn build_tool_context(
    workspace_root: &Path,
    options: &AppOptions,
) -> ToolExecutionContext {
    ToolExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        worktree_root: Some(workspace_root.to_path_buf()),
        workspace_only: options.workspace_only,
        model_context_window_tokens: Some(options.primary_profile.context_window_tokens),
        ..Default::default()
    }
}

pub(crate) fn tool_context_for_profile(
    base: &ToolExecutionContext,
    profile: &ResolvedAgentProfile,
) -> ToolExecutionContext {
    let mut context = base.clone();
    context.model_context_window_tokens = Some(profile.context_window_tokens);
    let base_policy = base.sandbox_policy();
    match profile.sandbox {
        AgentSandboxMode::DangerFullAccess => {
            context.workspace_only = false;
            context.read_only_roots.clear();
            context.writable_roots.clear();
            context.exec_roots.clear();
            context.network_policy = Some(agent::tools::NetworkPolicy::Full);
            context.effective_sandbox_policy = Some(
                agent::tools::SandboxPolicy::permissive()
                    .with_fail_if_unavailable(base_policy.fail_if_unavailable),
            );
        }
        AgentSandboxMode::WorkspaceWrite => {
            context.workspace_only = true;
            context.effective_sandbox_policy = Some(
                context
                    .sandbox_scope()
                    .recommended_policy()
                    .with_fail_if_unavailable(base_policy.fail_if_unavailable),
            );
        }
        AgentSandboxMode::ReadOnly => {
            context.workspace_only = true;
            context.read_only_roots = profile_read_only_roots(base);
            context.writable_roots.clear();
            context.network_policy = Some(
                match base
                    .network_policy
                    .clone()
                    .unwrap_or(agent::tools::NetworkPolicy::Off)
                {
                    agent::tools::NetworkPolicy::Full => agent::tools::NetworkPolicy::Off,
                    other => other,
                },
            );
            let derived = context
                .sandbox_scope()
                .recommended_policy()
                .with_fail_if_unavailable(base_policy.fail_if_unavailable);
            context.effective_sandbox_policy = Some(agent::tools::SandboxPolicy {
                mode: agent::tools::SandboxMode::ReadOnly,
                filesystem: agent::tools::FilesystemPolicy {
                    readable_roots: derived.filesystem.readable_roots,
                    writable_roots: Vec::new(),
                    executable_roots: derived.filesystem.executable_roots,
                    protected_paths: derived.filesystem.protected_paths,
                },
                network: derived.network,
                host_escape: agent::tools::HostEscapePolicy::Deny,
                fail_if_unavailable: derived.fail_if_unavailable,
            });
        }
    }
    context
}

pub(crate) fn log_sandbox_status(status: &SandboxBackendStatus) {
    match status {
        SandboxBackendStatus::Available { kind } => {
            info!(backend = kind.as_str(), "sandbox backend available");
        }
        SandboxBackendStatus::Unavailable { reason } => {
            warn!(
                "sandbox enforcement unavailable; local processes may fall back to host execution: {reason}"
            );
        }
        SandboxBackendStatus::NotRequired => {}
    }
}

pub(crate) fn inspect_sandbox_preflight(
    workspace_root: &Path,
    options: &AppOptions,
) -> SandboxPreflight {
    let tool_context = build_tool_context(workspace_root, options);
    let policy = build_sandbox_policy(options, &tool_context);
    let status = sandbox_backend_status(&policy);
    SandboxPreflight { policy, status }
}

pub(crate) fn build_sandbox_fallback_notice(
    preflight: &SandboxPreflight,
) -> Option<SandboxFallbackNotice> {
    let SandboxBackendStatus::Unavailable { reason } = &preflight.status else {
        return None;
    };
    if !preflight.policy.requires_enforcement() {
        return None;
    }
    Some(SandboxFallbackNotice {
        policy_summary: describe_sandbox_policy(&preflight.policy, &preflight.status),
        reason: reason.clone(),
        risk_summary: "continuing will disable sandbox enforcement; shell access, command hooks, stdio MCP servers, and managed code-intel helpers stay disabled to avoid host subprocess execution".to_string(),
        setup_steps: platform_sandbox_setup_steps(reason),
    })
}

fn profile_read_only_roots(base: &ToolExecutionContext) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    roots.insert(base.effective_root().to_path_buf());
    if let Some(worktree_root) = base.worktree_root.clone() {
        roots.insert(worktree_root);
    }
    roots.extend(base.additional_roots.iter().cloned());
    roots.extend(base.read_only_roots.iter().cloned());
    roots.extend(base.writable_roots.iter().cloned());
    roots.extend(base.exec_roots.iter().cloned());
    roots.into_iter().collect()
}

#[cfg(target_os = "linux")]
fn platform_sandbox_setup_steps(reason: &str) -> Vec<String> {
    let mut steps = vec![
        "Install `bubblewrap` and `uidmap` so `bwrap`, `newuidmap`, and `newgidmap` are available.".to_string(),
        "Ensure unprivileged user namespaces are enabled (`kernel.unprivileged_userns_clone=1` and `user.max_user_namespaces` is non-zero).".to_string(),
    ];
    if reason.contains("uid map") || reason.contains("Permission denied") {
        steps.push(
            "If you are inside Docker/Podman/LXC/WSL or behind AppArmor/SELinux restrictions, allow uid/gid map setup or run on a host that permits unprivileged user namespaces.".to_string(),
        );
    } else {
        steps.push(
            "If probing still fails after installation, inspect the host/container policy that blocks unprivileged namespaces.".to_string(),
        );
    }
    steps
}

#[cfg(target_os = "macos")]
fn platform_sandbox_setup_steps(_reason: &str) -> Vec<String> {
    vec![
        "Ensure the built-in macOS `sandbox-exec` binary is present and executable.".to_string(),
        "If the host image removed seatbelt support, run on a macOS host with sandboxing enabled."
            .to_string(),
    ]
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_sandbox_setup_steps(_reason: &str) -> Vec<String> {
    vec![
        "Run Code Agent on a host that provides a supported enforcing sandbox backend.".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{SandboxPreflight, build_sandbox_fallback_notice};
    use agent::tools::{NetworkPolicy, SandboxBackendStatus, SandboxMode, SandboxPolicy};

    #[test]
    fn fallback_notice_requires_unavailable_enforcing_backend() {
        let permissive = SandboxPreflight {
            policy: SandboxPolicy::permissive(),
            status: SandboxBackendStatus::Unavailable {
                reason: "not required".to_string(),
            },
        };
        assert!(build_sandbox_fallback_notice(&permissive).is_none());

        let restrictive = SandboxPreflight {
            policy: SandboxPolicy {
                mode: SandboxMode::WorkspaceWrite,
                network: NetworkPolicy::Off,
                ..SandboxPolicy::recommended_for_scope(&Default::default())
            },
            status: SandboxBackendStatus::Unavailable {
                reason: "uid map denied".to_string(),
            },
        };
        let notice = build_sandbox_fallback_notice(&restrictive).unwrap();
        assert!(notice.policy_summary.contains("workspace-write"));
        assert!(notice.reason.contains("uid map denied"));
        assert!(!notice.setup_steps.is_empty());
    }
}
