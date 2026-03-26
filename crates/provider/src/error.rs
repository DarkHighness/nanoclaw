use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider configuration error: {0}")]
    Config(String),
    #[error("provider protocol error: {0}")]
    Protocol(String),
    #[error("provider request error: {0}")]
    Request(String),
    #[error("provider JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ProviderError>;

impl ProviderError {
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    #[must_use]
    pub fn request(message: impl Into<String>) -> Self {
        Self::Request(message.into())
    }
}

impl From<ProviderError> for runtime::RuntimeError {
    fn from(value: ProviderError) -> Self {
        types::AgentCoreError::ModelBackend(value.to_string()).into()
    }
}
