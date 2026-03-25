use thiserror::Error;

#[derive(Debug, Error)]
pub enum RigError {
    #[error("provider configuration error: {0}")]
    Config(String),
    #[error("provider protocol error: {0}")]
    Protocol(String),
    #[error("provider request error: {0}")]
    Provider(String),
    #[error("provider JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, RigError>;

impl RigError {
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    #[must_use]
    pub fn provider(message: impl Into<String>) -> Self {
        Self::Provider(message.into())
    }
}

impl From<RigError> for agent_core_runtime::RuntimeError {
    fn from(value: RigError) -> Self {
        agent_core_types::AgentCoreError::ModelBackend(value.to_string()).into()
    }
}
