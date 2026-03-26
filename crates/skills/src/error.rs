use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("skill I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("skill YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("skill TOML error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("skill regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("failed to read {path}: {source}")]
    ReadPath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid skill format: {0}")]
    InvalidFormat(String),
}

pub type Result<T> = std::result::Result<T, SkillError>;

impl SkillError {
    #[must_use]
    pub fn invalid_format(message: impl Into<String>) -> Self {
        Self::InvalidFormat(message.into())
    }

    #[must_use]
    pub fn read_path(path: impl Into<String>, source: std::io::Error) -> Self {
        Self::ReadPath {
            path: path.into(),
            source,
        }
    }
}
