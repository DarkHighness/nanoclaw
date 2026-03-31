use crate::{EvaluationContext, EvaluationReport, Evaluator};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EvaluatorRegistryError {
    #[error("duplicate evaluator name: {0}")]
    DuplicateName(String),
    #[error(transparent)]
    Evaluator(#[from] crate::EvaluatorError),
}

#[derive(Clone, Default)]
pub struct EvaluatorRegistry {
    evaluators: Vec<Arc<dyn Evaluator>>,
}

impl EvaluatorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<E>(&mut self, evaluator: E) -> Result<(), EvaluatorRegistryError>
    where
        E: Evaluator + 'static,
    {
        self.register_arc(Arc::new(evaluator))
    }

    pub fn register_arc(
        &mut self,
        evaluator: Arc<dyn Evaluator>,
    ) -> Result<(), EvaluatorRegistryError> {
        if self
            .evaluators
            .iter()
            .any(|existing| existing.name() == evaluator.name())
        {
            return Err(EvaluatorRegistryError::DuplicateName(
                evaluator.name().to_string(),
            ));
        }
        self.evaluators.push(evaluator);
        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.evaluators.is_empty()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.evaluators
            .iter()
            .map(|evaluator| evaluator.name().to_string())
            .collect()
    }

    pub async fn evaluate(
        &self,
        context: &EvaluationContext,
    ) -> Result<EvaluationReport, EvaluatorRegistryError> {
        let mut outcomes = Vec::with_capacity(self.evaluators.len());

        // Evaluator order is part of the contract because hosts use it to keep
        // scorecards stable across repeated experiment runs and diff views.
        for evaluator in &self.evaluators {
            outcomes.push(evaluator.evaluate(context).await?);
        }

        Ok(EvaluationReport::from_evaluator_outcomes(
            context.candidate.candidate_id.clone(),
            outcomes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::EvaluatorRegistry;
    use crate::{EvaluationContext, Evaluator, EvaluatorError, EvaluatorOutcome};
    use async_trait::async_trait;
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use types::{
        BaselineId, CandidateId, CandidateSpec, ExperimentId, ExperimentSpec, ExperimentTarget,
    };

    struct PassEvaluator;

    #[async_trait]
    impl Evaluator for PassEvaluator {
        fn name(&self) -> &str {
            "schema"
        }

        async fn evaluate(
            &self,
            _context: &EvaluationContext,
        ) -> Result<EvaluatorOutcome, EvaluatorError> {
            Ok(EvaluatorOutcome {
                evaluator_name: self.name().to_string(),
                passed: true,
                score: Some(1.0),
                summary: "schema valid".to_string(),
                details: None,
            })
        }
    }

    struct FailEvaluator;

    #[async_trait]
    impl Evaluator for FailEvaluator {
        fn name(&self) -> &str {
            "regression"
        }

        async fn evaluate(
            &self,
            _context: &EvaluationContext,
        ) -> Result<EvaluatorOutcome, EvaluatorError> {
            Ok(EvaluatorOutcome {
                evaluator_name: self.name().to_string(),
                passed: false,
                score: Some(0.1),
                summary: "regression detected".to_string(),
                details: Some(json!({"failed_case":"retry-loop"})),
            })
        }
    }

    fn fixture_context() -> EvaluationContext {
        EvaluationContext::for_candidate(
            ExperimentId::from("experiment-1"),
            &ExperimentSpec {
                target: ExperimentTarget::Prompt,
                goal: "stabilize tool planning".to_string(),
                source_session_id: None,
                source_agent_session_id: None,
                metadata: json!({"suite":"planner"}),
            },
            CandidateSpec {
                candidate_id: CandidateId::from("candidate-1"),
                baseline_id: BaselineId::from("baseline-1"),
                target: ExperimentTarget::Prompt,
                label: "prompt-v2".to_string(),
                description: None,
                config: json!({"profile":"v2"}),
            },
            None,
        )
    }

    #[test]
    fn registry_rejects_duplicate_names() {
        let mut registry = EvaluatorRegistry::new();
        registry.register(PassEvaluator).unwrap();
        let error = registry.register(PassEvaluator).unwrap_err();
        assert!(matches!(
            error,
            crate::EvaluatorRegistryError::DuplicateName(name) if name == "schema"
        ));
    }

    #[test]
    fn registry_aggregates_evaluator_results() {
        run_current_thread_test(async {
            let mut registry = EvaluatorRegistry::new();
            registry.register(PassEvaluator).unwrap();
            registry.register(FailEvaluator).unwrap();

            let report = registry.evaluate(&fixture_context()).await.unwrap();
            assert_eq!(report.evaluators.len(), 2);
            assert!(!report.passed);
            assert_eq!(report.candidate_id.as_str(), "candidate-1");
            assert_eq!(report.summary, "1 of 2 evaluators passed");
            assert_eq!(report.score, Some(0.55));
        });
    }
}
