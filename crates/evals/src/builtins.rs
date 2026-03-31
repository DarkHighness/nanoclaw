use crate::{EvaluationContext, Evaluator, EvaluatorError, EvaluatorOutcome};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuiltinEvaluatorSpec {
    ConfigPointerDefined {
        evaluator_name: String,
        pointer: String,
    },
    ConfigPointerEquals {
        evaluator_name: String,
        pointer: String,
        expected: Value,
    },
    ConfigPointerMinimum {
        evaluator_name: String,
        pointer: String,
        minimum: f64,
    },
}

impl BuiltinEvaluatorSpec {
    #[must_use]
    pub fn evaluator_name(&self) -> &str {
        match self {
            Self::ConfigPointerDefined { evaluator_name, .. }
            | Self::ConfigPointerEquals { evaluator_name, .. }
            | Self::ConfigPointerMinimum { evaluator_name, .. } => evaluator_name,
        }
    }

    #[must_use]
    pub fn build(&self) -> Arc<dyn Evaluator> {
        match self {
            Self::ConfigPointerDefined {
                evaluator_name,
                pointer,
            } => Arc::new(ConfigPointerDefinedEvaluator {
                evaluator_name: evaluator_name.clone(),
                pointer: pointer.clone(),
            }),
            Self::ConfigPointerEquals {
                evaluator_name,
                pointer,
                expected,
            } => Arc::new(ConfigPointerEqualsEvaluator {
                evaluator_name: evaluator_name.clone(),
                pointer: pointer.clone(),
                expected: expected.clone(),
            }),
            Self::ConfigPointerMinimum {
                evaluator_name,
                pointer,
                minimum,
            } => Arc::new(ConfigPointerMinimumEvaluator {
                evaluator_name: evaluator_name.clone(),
                pointer: pointer.clone(),
                minimum: *minimum,
            }),
        }
    }
}

struct ConfigPointerDefinedEvaluator {
    evaluator_name: String,
    pointer: String,
}

#[async_trait]
impl Evaluator for ConfigPointerDefinedEvaluator {
    fn name(&self) -> &str {
        &self.evaluator_name
    }

    async fn evaluate(
        &self,
        context: &EvaluationContext,
    ) -> Result<EvaluatorOutcome, EvaluatorError> {
        let value = pointer_value(context, &self.pointer);
        let passed = value.is_some_and(|value| !value.is_null());
        Ok(EvaluatorOutcome {
            evaluator_name: self.evaluator_name.clone(),
            passed,
            score: Some(if passed { 1.0 } else { 0.0 }),
            summary: if passed {
                format!("candidate config defines {}", self.pointer)
            } else {
                format!("candidate config is missing {}", self.pointer)
            },
            details: Some(json!({
                "pointer": self.pointer,
                "actual": value.cloned(),
            })),
        })
    }
}

struct ConfigPointerEqualsEvaluator {
    evaluator_name: String,
    pointer: String,
    expected: Value,
}

#[async_trait]
impl Evaluator for ConfigPointerEqualsEvaluator {
    fn name(&self) -> &str {
        &self.evaluator_name
    }

    async fn evaluate(
        &self,
        context: &EvaluationContext,
    ) -> Result<EvaluatorOutcome, EvaluatorError> {
        let actual = pointer_value(context, &self.pointer).cloned();
        let passed = actual.as_ref() == Some(&self.expected);
        Ok(EvaluatorOutcome {
            evaluator_name: self.evaluator_name.clone(),
            passed,
            score: Some(if passed { 1.0 } else { 0.0 }),
            summary: if passed {
                format!(
                    "candidate config matches expected value at {}",
                    self.pointer
                )
            } else {
                format!(
                    "candidate config differs from expected value at {}",
                    self.pointer
                )
            },
            details: Some(json!({
                "pointer": self.pointer,
                "expected": self.expected,
                "actual": actual,
            })),
        })
    }
}

struct ConfigPointerMinimumEvaluator {
    evaluator_name: String,
    pointer: String,
    minimum: f64,
}

#[async_trait]
impl Evaluator for ConfigPointerMinimumEvaluator {
    fn name(&self) -> &str {
        &self.evaluator_name
    }

    async fn evaluate(
        &self,
        context: &EvaluationContext,
    ) -> Result<EvaluatorOutcome, EvaluatorError> {
        let actual = pointer_value(context, &self.pointer).and_then(Value::as_f64);
        let passed = actual.is_some_and(|actual| actual >= self.minimum);
        Ok(EvaluatorOutcome {
            evaluator_name: self.evaluator_name.clone(),
            passed,
            score: actual.map(|actual| (actual / self.minimum).min(1.0)),
            summary: if passed {
                format!(
                    "candidate config meets minimum {:.3} at {}",
                    self.minimum, self.pointer
                )
            } else {
                format!(
                    "candidate config is below minimum {:.3} at {}",
                    self.minimum, self.pointer
                )
            },
            details: Some(json!({
                "pointer": self.pointer,
                "minimum": self.minimum,
                "actual": actual,
            })),
        })
    }
}

fn pointer_value<'a>(context: &'a EvaluationContext, pointer: &str) -> Option<&'a Value> {
    context.candidate.config.pointer(pointer)
}

#[cfg(test)]
mod tests {
    use super::BuiltinEvaluatorSpec;
    use crate::EvaluationContext;
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use types::{
        BaselineId, CandidateId, CandidateSpec, ExperimentId, ExperimentSpec, ExperimentTarget,
    };

    fn fixture_context() -> EvaluationContext {
        EvaluationContext::for_candidate(
            ExperimentId::from("experiment-builtin"),
            &ExperimentSpec {
                target: ExperimentTarget::Policy,
                goal: "keep approval score above threshold".to_string(),
                source_session_id: None,
                source_agent_session_id: None,
                metadata: json!({}),
            },
            CandidateSpec {
                candidate_id: CandidateId::from("candidate-builtin"),
                baseline_id: BaselineId::from("baseline-builtin"),
                target: ExperimentTarget::Policy,
                label: "policy-v2".to_string(),
                description: None,
                config: json!({
                    "profile": "strict",
                    "metrics": { "score": 0.93 }
                }),
            },
            None,
        )
    }

    #[test]
    fn defined_evaluator_checks_for_non_null_pointer() {
        run_current_thread_test(async {
            let evaluator = BuiltinEvaluatorSpec::ConfigPointerDefined {
                evaluator_name: "profile_defined".to_string(),
                pointer: "/profile".to_string(),
            }
            .build();

            let outcome = evaluator.evaluate(&fixture_context()).await.unwrap();
            assert!(outcome.passed);
            assert_eq!(outcome.score, Some(1.0));
        });
    }

    #[test]
    fn minimum_evaluator_reads_numeric_pointer() {
        run_current_thread_test(async {
            let evaluator = BuiltinEvaluatorSpec::ConfigPointerMinimum {
                evaluator_name: "score_gate".to_string(),
                pointer: "/metrics/score".to_string(),
                minimum: 0.9,
            }
            .build();

            let outcome = evaluator.evaluate(&fixture_context()).await.unwrap();
            assert!(outcome.passed);
            assert_eq!(
                outcome.summary,
                "candidate config meets minimum 0.900 at /metrics/score"
            );
        });
    }
}
