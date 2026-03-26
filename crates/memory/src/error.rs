use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("memory I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("memory JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("memory TOML decode error: {0}")]
    TomlDecode(#[from] toml::de::Error),
    #[error("memory TOML encode error: {0}")]
    TomlEncode(#[from] toml::ser::Error),
    #[error("memory regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("memory glob error: {0}")]
    Glob(#[from] globset::Error),
    #[error("memory input error: {0}")]
    InvalidInput(String),
    #[error("memory path `{0}` is outside allowed roots")]
    PathOutsideWorkspace(String),
    #[error("memory path `{0}` is not part of the configured corpus")]
    PathNotInCorpus(String),
}

pub type Result<T> = std::result::Result<T, MemoryError>;

impl MemoryError {
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }
}
