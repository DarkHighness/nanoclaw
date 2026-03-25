use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool input error: {0}")]
    InvalidInput(String),
    #[error("tool state error: {0}")]
    InvalidState(String),
    #[error("tool I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tool JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("tool regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("tool glob error: {0}")]
    Glob(#[from] globset::Error),
    #[error("tool ignore-walk error: {0}")]
    IgnoreWalk(#[from] ignore::Error),
    #[error("tool base64 error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("tool path error: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),
    #[cfg(feature = "web-tools")]
    #[error("tool HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, ToolError>;

impl ToolError {
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    #[must_use]
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::InvalidState(message.into())
    }
}
