use error_stack::{IntoReport, Report};
use std::error::Error as StdError;
use std::fmt;
use thiserror::Error;

type ErrorSource = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("inference JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inference HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("inference input error: {message}")]
    InvalidInput {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("inference service error: {message}")]
    Service {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
}

pub type Result<T> = std::result::Result<T, InferenceError>;

impl InferenceError {
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn invalid_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::InvalidInput {
            source: Some(boxed_report_source(
                "inference input error",
                &message,
                error,
            )),
            message,
        }
    }

    #[must_use]
    pub fn service(message: impl Into<String>) -> Self {
        Self::Service {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn service_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::Service {
            source: Some(boxed_report_source(
                "inference service error",
                &message,
                error,
            )),
            message,
        }
    }
}

#[derive(Debug)]
struct InferenceDiagnostic {
    category: &'static str,
    message: String,
}

impl fmt::Display for InferenceDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category, self.message)
    }
}

impl StdError for InferenceDiagnostic {}

#[derive(Debug)]
struct InferenceDiagnosticSource {
    report: Report<InferenceDiagnostic>,
}

impl fmt::Display for InferenceDiagnosticSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.report.current_context().fmt(f)
    }
}

impl StdError for InferenceDiagnosticSource {}

fn boxed_report_source<E>(category: &'static str, message: &str, error: E) -> ErrorSource
where
    E: StdError + Send + Sync + 'static,
{
    Box::new(InferenceDiagnosticSource {
        report: error.into_report().change_context(InferenceDiagnostic {
            category,
            message: message.to_owned(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::InferenceError;
    use std::error::Error as _;

    #[test]
    fn source_backed_inference_errors_keep_diagnostics_attached() {
        let error = InferenceError::service_with_source(
            "failed to contact inference service",
            std::io::Error::other("connection reset"),
        );

        assert!(error.source().is_some());
    }
}
