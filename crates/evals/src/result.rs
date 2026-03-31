use serde::{Deserialize, Serialize};
use serde_json::Value;
use types::{CandidateEvaluationSummary, CandidateId, EvaluatorSummary};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluatorOutcome {
    pub evaluator_name: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl From<EvaluatorOutcome> for EvaluatorSummary {
    fn from(value: EvaluatorOutcome) -> Self {
        Self {
            evaluator_name: value.evaluator_name,
            passed: value.passed,
            score: value.score,
            summary: value.summary,
            details: value.details,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub candidate_id: CandidateId,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub summary: String,
    #[serde(default)]
    pub evaluators: Vec<EvaluatorOutcome>,
}

impl EvaluationReport {
    #[must_use]
    pub fn from_evaluator_outcomes(
        candidate_id: CandidateId,
        evaluators: Vec<EvaluatorOutcome>,
    ) -> Self {
        let passed = evaluators.iter().all(|outcome| outcome.passed);
        let passed_count = evaluators.iter().filter(|outcome| outcome.passed).count();
        let scored = evaluators
            .iter()
            .filter_map(|outcome| outcome.score)
            .collect::<Vec<_>>();
        let score = if scored.is_empty() {
            None
        } else {
            Some(scored.iter().sum::<f64>() / scored.len() as f64)
        };
        let summary = if evaluators.is_empty() {
            "no evaluators configured".to_string()
        } else {
            format!("{passed_count} of {} evaluators passed", evaluators.len())
        };

        Self {
            candidate_id,
            passed,
            score,
            summary,
            evaluators,
        }
    }

    #[must_use]
    pub fn to_candidate_summary(&self) -> CandidateEvaluationSummary {
        CandidateEvaluationSummary {
            candidate_id: self.candidate_id.clone(),
            passed: self.passed,
            score: self.score,
            summary: self.summary.clone(),
            evaluators: self
                .evaluators
                .clone()
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}
