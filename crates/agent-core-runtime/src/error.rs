use agent_core_skills::SkillError;
use agent_core_store::RunStoreError;
use agent_core_tools::ToolError;
use agent_core_types::AgentCoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    AgentCore(#[from] AgentCoreError),
    #[error(transparent)]
    Skills(#[from] SkillError),
    #[error(transparent)]
    Store(#[from] RunStoreError),
    #[error(transparent)]
    Tools(#[from] ToolError),
    #[error("runtime I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("runtime regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("runtime HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("runtime state error: {0}")]
    InvalidState(String),
    #[error("runtime hook error: {0}")]
    Hook(String),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

impl RuntimeError {
    #[must_use]
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::InvalidState(message.into())
    }

    #[must_use]
    pub fn hook(message: impl Into<String>) -> Self {
        Self::Hook(message.into())
    }
}
