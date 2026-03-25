use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentCoreError {
    #[error("hook blocked execution: {0}")]
    HookBlocked(String),
    #[error("tool denied: {0}")]
    ToolDenied(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("model backend error: {0}")]
    ModelBackend(String),
    #[error("tool error: {0}")]
    Tool(String),
}
