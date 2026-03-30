use error_stack::{IntoReport, Report};
use skills::SkillError;
use std::error::Error as StdError;
use std::fmt;
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
    },
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

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
