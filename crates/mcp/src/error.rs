use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("MCP I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCP JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("MCP invalid header name: {0}")]
    HeaderName(#[from] http::header::InvalidHeaderName),
    #[error("MCP invalid header value: {0}")]
    HeaderValue(#[from] http::header::InvalidHeaderValue),
    #[error("MCP protocol error: {0}")]
    Protocol(String),
    #[error("MCP transport error: {0}")]
    Transport(String),
}

pub type Result<T> = std::result::Result<T, McpError>;

impl McpError {
    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    #[must_use]
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }
}
