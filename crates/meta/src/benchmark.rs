use crate::{
    CriticReport, ExperimentArchive, PromotionGate, PromotionInput, ThresholdPromotionGate,
};
use evals::{BuiltinEvaluatorSpec, EvaluationContext, EvaluationReport, EvaluatorRegistry};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use store::{ExperimentSummary, SessionStore};
use thiserror::Error;
use types::{BaselineSpec, CandidateSpec, ExperimentId, ExperimentSpec, PromotionDecision};

#[derive(Debug, Error)]
pub enum MetaError {
    #[error(transparent)]
    Store(#[from] store::SessionStoreError),
    #[error(transparent)]
    EvaluatorRegistry(#[from] evals::EvaluatorRegistryError),
    #[error("{0}")]
    InvalidPlan(String),
}

pub type Result<T> = std::result::Result<T, MetaError>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfflineBenchmarkPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<ExperimentId>,
    pub spec: ExperimentSpec,
    pub baseline: BaselineSpec,
    pub candidate: CandidateSpec,
    #[serde(default)]
    pub evaluators: Vec<BuiltinEvaluatorSpec>,
    #[serde(default)]
    pub critic_report: CriticReport,
    #[serde(default)]
    pub promotion_gate: ThresholdPromotionGate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRunOutcome {
    pub experiment_id: ExperimentId,
    pub evaluation: EvaluationReport,
    pub decision: PromotionDecision,
    pub summary: ExperimentSummary,
}

#[derive(Clone)]
pub struct OfflineBenchmarkRunner<S: SessionStore + ?Sized> {
    archive: ExperimentArchive<S>,
}

impl<S: SessionStore + ?Sized> OfflineBenchmarkRunner<S> {
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self {
            archive: ExperimentArchive::new(store),
        }
    }

    pub async fn run(&self, plan: OfflineBenchmarkPlan) -> Result<BenchmarkRunOutcome> {
        let experiment_id = plan.experiment_id.unwrap_or_default();
        self.archive
            .start(experiment_id.clone(), plan.spec.clone())
            .await?;
        self.archive
            .pin_baseline(&experiment_id, plan.baseline.clone())
            .await?;
        self.archive
            .record_candidate(&experiment_id, plan.candidate.clone())
            .await?;

        let evaluation = evaluate_candidate(
            &experiment_id,
            &plan.spec,
            &plan.baseline,
            &plan.candidate,
            &plan.evaluators,
        )
        .await?;
        self.archive
            .record_evaluation(&experiment_id, &evaluation)
            .await?;

        let decision = plan.promotion_gate.decide(&PromotionInput {
            candidate_id: plan.candidate.candidate_id.clone(),
            evaluation: evaluation.clone(),
            critic_report: plan.critic_report,
        });
        self.archive
            .record_decision(
                &experiment_id,
                plan.candidate.candidate_id.clone(),
                decision.clone(),
            )
            .await?;

        let summary = self
            .archive
            .summary(&experiment_id)
            .await?
            .unwrap_or_else(|| {
                panic!(
                    "experiment summary missing after benchmark run: {}",
                    experiment_id
                )
            });

        Ok(BenchmarkRunOutcome {
            experiment_id,
            evaluation,
            decision,
            summary,
        })
    }
}

pub(crate) async fn evaluate_candidate(
    experiment_id: &ExperimentId,
    spec: &ExperimentSpec,
    baseline: &BaselineSpec,
    candidate: &CandidateSpec,
    evaluators: &[BuiltinEvaluatorSpec],
) -> Result<EvaluationReport> {
    let mut registry = EvaluatorRegistry::new();
    for evaluator in evaluators {
        registry.register_arc(evaluator.build())?;
    }
    let context = EvaluationContext::for_candidate(
        experiment_id.clone(),
        spec,
        candidate.clone(),
        Some(baseline.clone()),
    );
    Ok(registry.evaluate(&context).await?)
}

#[cfg(test)]
mod tests {
    use super::{BenchmarkRunOutcome, OfflineBenchmarkPlan, OfflineBenchmarkRunner};
    use crate::{CriticFinding, CriticReport, CriticSeverity};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use std::sync::Arc;
    use store::{InMemorySessionStore, SessionStore};
    use types::{
        BaselineId, BaselineSpec, CandidateId, CandidateSpec, ExperimentSpec, ExperimentTarget,
        PromotionDecisionKind,
    };

    fn plan() -> OfflineBenchmarkPlan {
        OfflineBenchmarkPlan {
            experiment_id: Some("experiment-benchmark".into()),
            spec: ExperimentSpec {
                target: ExperimentTarget::Policy,
                goal: "keep approval score above threshold".to_string(),
                source_session_id: Some("session-benchmark".into()),
                source_agent_session_id: Some("agent-benchmark".into()),
                metadata: json!({"pack":"policy-smoke"}),
            },
            baseline: BaselineSpec {
                baseline_id: BaselineId::from("baseline-benchmark"),
                target: ExperimentTarget::Policy,
                label: "policy-v1".to_string(),
                description: None,
                config: Some(json!({"profile":"baseline"})),
            },
            candidate: CandidateSpec {
                candidate_id: CandidateId::from("candidate-benchmark"),
                baseline_id: BaselineId::from("baseline-benchmark"),
                target: ExperimentTarget::Policy,
                label: "policy-v2".to_string(),
                description: Some("raise approval score".to_string()),
                config: json!({
                    "profile": "strict",
                    "metrics": { "score": 0.94 }
                }),
            },
            evaluators: vec![
                evals::BuiltinEvaluatorSpec::ConfigPointerDefined {
                    evaluator_name: "profile_defined".to_string(),
                    pointer: "/profile".to_string(),
                },
                evals::BuiltinEvaluatorSpec::ConfigPointerMinimum {
                    evaluator_name: "score_gate".to_string(),
                    pointer: "/metrics/score".to_string(),
                    minimum: 0.9,
                },
            ],
            critic_report: CriticReport::default(),
            promotion_gate: crate::ThresholdPromotionGate {
                minimum_score: Some(0.9),
                ..crate::ThresholdPromotionGate::default()
            },
        }
    }

    #[test]
    fn benchmark_runner_records_successful_run_in_archive() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let runner = OfflineBenchmarkRunner::new(store.clone());

            let outcome = runner.run(plan()).await.unwrap();
            assert_success(outcome);

            let experiments = store.list_experiments().await.unwrap();
            assert_eq!(experiments.len(), 1);
            assert_eq!(
                experiments[0].last_decision,
                Some(PromotionDecisionKind::Promoted)
            );
        });
    }

    #[test]
    fn benchmark_runner_respects_blocking_critic_findings() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let runner = OfflineBenchmarkRunner::new(store);
            let mut plan = plan();
            plan.critic_report = CriticReport {
                findings: vec![CriticFinding {
                    code: "unsafe_regression".to_string(),
                    severity: CriticSeverity::Blocking,
                    summary: "unsafe regression".to_string(),
                }],
            };

            let outcome = runner.run(plan).await.unwrap();
            assert_eq!(outcome.decision.kind, PromotionDecisionKind::Rejected);
            assert!(outcome.decision.reason.contains("unsafe regression"));
        });
    }

    fn assert_success(outcome: BenchmarkRunOutcome) {
        assert_eq!(outcome.experiment_id.as_str(), "experiment-benchmark");
        assert!(outcome.evaluation.passed);
        assert_eq!(outcome.evaluation.evaluators.len(), 2);
        assert_eq!(outcome.decision.kind, PromotionDecisionKind::Promoted);
        assert_eq!(outcome.summary.candidate_count, 1);
        assert_eq!(outcome.summary.baseline_count, 1);
    }
}
