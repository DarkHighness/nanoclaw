use error_stack::{IntoReport, Report};
use skills::SkillError;
use std::error::Error as StdError;
use std::fmt;
use std::time::Duration;
use store::SessionStoreError;
use thiserror::Error;
use tools::ToolError;
use types::AgentCoreError;

type ErrorSource = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    AgentCore(#[from] AgentCoreError),
    #[error(transparent)]
    Skills(#[from] SkillError),
    #[error(transparent)]
    Store(#[from] SessionStoreError),
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
    #[error("runtime state error: {message}")]
    InvalidState {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("runtime hook error: {message}")]
    Hook {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("model backend error: {message}")]
    ModelBackend {
        message: String,
        #[source]
        source: Option<ErrorSource>,
        status_code: Option<u16>,
        retryable: bool,
        retry_after: Option<Duration>,
    },
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelBackendRetryHint {
    pub status_code: u16,
    pub retry_after: Option<Duration>,
}

impl RuntimeError {
    #[must_use]
    pub fn invalid_state(message: impl Into<String>) -> Self {
        Self::InvalidState {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn invalid_state_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::InvalidState {
            source: Some(boxed_report_source("runtime state error", &message, error)),
            message,
        }
    }

    #[must_use]
    pub fn hook(message: impl Into<String>) -> Self {
        Self::Hook {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn hook_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::Hook {
            source: Some(boxed_report_source("runtime hook error", &message, error)),
            message,
        }
    }

    #[must_use]
    pub fn model_backend(message: impl Into<String>) -> Self {
        Self::ModelBackend {
            message: message.into(),
            source: None,
            status_code: None,
            retryable: false,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn model_backend_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::ModelBackend {
            source: Some(boxed_report_source("model backend error", &message, error)),
            message,
            status_code: None,
            retryable: false,
            retry_after: None,
        }
    }

    #[must_use]
    pub fn model_backend_request(
        message: impl Into<String>,
        status_code: u16,
        retryable: bool,
        retry_after: Option<Duration>,
    ) -> Self {
        Self::ModelBackend {
            message: message.into(),
            source: None,
            status_code: Some(status_code),
            retryable,
            retry_after,
        }
    }

    #[must_use]
    pub fn model_backend_request_with_source<E>(
        message: impl Into<String>,
        status_code: u16,
        retryable: bool,
        retry_after: Option<Duration>,
        error: E,
    ) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::ModelBackend {
            source: Some(boxed_report_source("model backend error", &message, error)),
            message,
            status_code: Some(status_code),
            retryable,
            retry_after,
        }
    }

    #[must_use]
    pub fn model_backend_status_code(&self) -> Option<u16> {
        match self {
            Self::ModelBackend { status_code, .. } => *status_code,
            _ => None,
        }
    }

    #[must_use]
    pub fn model_backend_retry_hint(&self) -> Option<ModelBackendRetryHint> {
        match self {
            Self::ModelBackend {
                status_code: Some(status_code),
                retryable: true,
                retry_after,
                ..
            } => Some(ModelBackendRetryHint {
                status_code: *status_code,
                retry_after: *retry_after,
            }),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct RuntimeDiagnostic {
    category: &'static str,
    message: String,
}

impl fmt::Display for RuntimeDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category, self.message)
    }
}

impl StdError for RuntimeDiagnostic {}

#[derive(Debug)]
struct RuntimeDiagnosticSource {
    report: Report<RuntimeDiagnostic>,
}

impl fmt::Display for RuntimeDiagnosticSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.report.current_context().fmt(f)
    }
}

impl StdError for RuntimeDiagnosticSource {}

fn boxed_report_source<E>(category: &'static str, message: &str, error: E) -> ErrorSource
where
    E: StdError + Send + Sync + 'static,
{
    Box::new(RuntimeDiagnosticSource {
        report: error.into_report().change_context(RuntimeDiagnostic {
            category,
            message: message.to_owned(),
        }),
    })
}
