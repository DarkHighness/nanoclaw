use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid plugin manifest at {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },
    #[error("invalid plugin path `{value}`: {message}")]
    InvalidPath { value: String, message: String },
}

pub type Result<T> = std::result::Result<T, PluginError>;

impl PluginError {
    #[must_use]
    pub fn invalid_manifest(path: PathBuf, message: impl Into<String>) -> Self {
        Self::InvalidManifest {
            path,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn invalid_path(value: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidPath {
            value: value.into(),
            message: message.into(),
        }
    }
}
