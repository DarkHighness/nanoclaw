use crate::{EvaluationContext, EvaluatorOutcome};
use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EvaluatorError {
    #[error("evaluator `{evaluator_name}` failed: {message}")]
    Execution {
        evaluator_name: String,
        message: String,
    },
}

impl EvaluatorError {
    #[must_use]
    pub fn execution(evaluator_name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Execution {
            evaluator_name: evaluator_name.into(),
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait Evaluator: Send + Sync {
    fn name(&self) -> &str;

    async fn evaluate(
        &self,
        context: &EvaluationContext,
    ) -> Result<EvaluatorOutcome, EvaluatorError>;
}
