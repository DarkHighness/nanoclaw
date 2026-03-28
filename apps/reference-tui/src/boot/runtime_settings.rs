use crate::config::AgentCoreConfig;
use nanoclaw_config::AgentSandboxMode;
use tools::{
    FilesystemPolicy, HostEscapePolicy, NetworkPolicy, SandboxMode, SandboxPolicy,
    ToolExecutionContext,
};

pub(super) fn context_tokens(config: &AgentCoreConfig) -> usize {
    config.primary_profile.context_window_tokens
}

pub(super) fn build_sandbox_policy(
    config: &AgentCoreConfig,
    tool_context: &ToolExecutionContext,
) -> SandboxPolicy {
    let base_policy = tool_context.sandbox_scope().recommended_policy();
    let fail_if_unavailable = config.core.host.sandbox_fail_if_unavailable;
    match config.primary_profile.sandbox {
        AgentSandboxMode::DangerFullAccess => {
            SandboxPolicy::permissive().with_fail_if_unavailable(fail_if_unavailable)
        }
        AgentSandboxMode::WorkspaceWrite => {
            base_policy.with_fail_if_unavailable(fail_if_unavailable)
        }
        AgentSandboxMode::ReadOnly => SandboxPolicy {
            mode: SandboxMode::ReadOnly,
            filesystem: FilesystemPolicy {
                readable_roots: base_policy.filesystem.readable_roots,
                writable_roots: Vec::new(),
                executable_roots: base_policy.filesystem.executable_roots,
                protected_paths: base_policy.filesystem.protected_paths,
            },
            network: match base_policy.network {
                NetworkPolicy::Full => NetworkPolicy::Off,
                other => other,
            },
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable,
        },
    }
}
