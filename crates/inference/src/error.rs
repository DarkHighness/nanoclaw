use thiserror::Error;

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("inference JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inference HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("inference input error: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, InferenceError>;

impl InferenceError {
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }
}
