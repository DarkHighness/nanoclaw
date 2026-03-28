use crate::options::AppOptions;
use agent::ToolExecutionContext;
use agent::tools::{SandboxBackendStatus, SandboxPolicy};
use agent_env::EnvMap;
use nanoclaw_config::{AgentSandboxMode, ResolvedAgentProfile};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

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
                "sandbox enforcement unavailable; local processes will fall back to host execution: {reason}"
            );
            eprintln!(
                "warning: sandbox enforcement unavailable; local processes will fall back to host execution: {reason}"
            );
        }
        SandboxBackendStatus::NotRequired => {}
    }
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
