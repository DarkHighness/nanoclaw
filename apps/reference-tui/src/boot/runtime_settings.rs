use crate::config::AgentCoreConfig;
use tools::{SandboxPolicy, ToolExecutionContext};

pub(super) fn context_tokens(config: &AgentCoreConfig) -> usize {
    config.runtime.context_tokens.unwrap_or(128_000)
}

pub(super) fn build_sandbox_policy(
    config: &AgentCoreConfig,
    tool_context: &ToolExecutionContext,
) -> SandboxPolicy {
    // The host config only controls whether missing enforcement backends are a
    // hard error or a best-effort fallback. Filesystem and network posture
    // still derive from the tool context so runtime path policy and local
    // process policy do not drift apart.
    tool_context
        .sandbox_scope()
        .recommended_policy()
        .with_fail_if_unavailable(config.runtime.sandbox_fail_if_unavailable)
}
