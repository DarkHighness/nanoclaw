use error_stack::{IntoReport, Report};
use std::error::Error as StdError;
use std::fmt;
use thiserror::Error;

type ErrorSource = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider configuration error: {message}")]
    Config {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("provider protocol error: {message}")]
    Protocol {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("provider request error: {message}")]
    Request {
        message: String,
        #[source]
        source: Option<ErrorSource>,
    },
    #[error("provider JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ProviderError>;

impl ProviderError {
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn config_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::Config {
            source: Some(boxed_report_source(
                "provider configuration error",
                &message,
                error,
            )),
            message,
        }
    }

    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn protocol_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::Protocol {
            source: Some(boxed_report_source(
                "provider protocol error",
                &message,
                error,
            )),
            message,
        }
    }

    #[must_use]
    pub fn request(message: impl Into<String>) -> Self {
        Self::Request {
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub fn request_with_source<E>(message: impl Into<String>, error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let message = message.into();
        Self::Request {
            source: Some(boxed_report_source(
                "provider request error",
                &message,
                error,
            )),
            message,
        }
    }
}

impl From<ProviderError> for runtime::RuntimeError {
    fn from(value: ProviderError) -> Self {
        match value {
            ProviderError::Config { message, source }
            | ProviderError::Protocol { message, source }
            | ProviderError::Request { message, source } => match source {
                None => runtime::RuntimeError::model_backend(message),
                Some(source) => runtime::RuntimeError::model_backend_with_source(
                    message,
                    ProviderRuntimeSource(source),
                ),
            },
            ProviderError::Json(error) => runtime::RuntimeError::model_backend_with_source(
                "failed to serialize or decode provider JSON payload",
                error,
            ),
        }
    }
}

#[derive(Debug)]
struct ProviderDiagnostic {
    category: &'static str,
    message: String,
}

impl fmt::Display for ProviderDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category, self.message)
    }
}

impl StdError for ProviderDiagnostic {}

#[derive(Debug)]
struct ProviderDiagnosticSource {
    report: Report<ProviderDiagnostic>,
}

impl fmt::Display for ProviderDiagnosticSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.report.current_context().fmt(f)
    }
}

impl StdError for ProviderDiagnosticSource {}

#[derive(Debug)]
struct ProviderRuntimeSource(ErrorSource);

impl fmt::Display for ProviderRuntimeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl StdError for ProviderRuntimeSource {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.0.as_ref())
    }
}

fn boxed_report_source<E>(category: &'static str, message: &str, error: E) -> ErrorSource
where
    E: StdError + Send + Sync + 'static,
{
    Box::new(ProviderDiagnosticSource {
        report: error.into_report().change_context(ProviderDiagnostic {
            category,
            message: message.to_owned(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::ProviderError;
    use std::error::Error as _;

    #[test]
    fn source_backed_provider_errors_keep_diagnostics_attached() {
        let error = ProviderError::request_with_source(
            "failed to connect to upstream provider",
            std::io::Error::other("dial tcp timeout"),
        );

        assert!(error.source().is_some());
    }
}
