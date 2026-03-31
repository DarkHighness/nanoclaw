use crate::{
    CriticReport, ExperimentArchive, MetaError, PromotionGate, PromotionInput,
    ThresholdPromotionGate, benchmark::evaluate_candidate,
};
use evals::{BuiltinEvaluatorSpec, EvaluationReport};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use store::{ExperimentSummary, SessionStore};
use types::{
    BaselineSpec, CandidateId, CandidateSpec, ExperimentId, ExperimentSpec, PromotionDecision,
    PromotionDecisionKind,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImprovementCandidatePlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<CandidateId>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub config: serde_json::Value,
    #[serde(default)]
    pub critic_report: CriticReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfflineImprovePlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<ExperimentId>,
    pub spec: ExperimentSpec,
    pub baseline: BaselineSpec,
    #[serde(default)]
    pub candidates: Vec<ImprovementCandidatePlan>,
    #[serde(default)]
    pub evaluators: Vec<BuiltinEvaluatorSpec>,
    #[serde(default)]
    pub promotion_gate: ThresholdPromotionGate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImprovementCandidateOutcome {
    pub candidate: CandidateSpec,
    pub evaluation: EvaluationReport,
    pub decision: PromotionDecision,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImproveRunOutcome {
    pub experiment_id: ExperimentId,
    pub winner_candidate_id: Option<CandidateId>,
    pub candidates: Vec<ImprovementCandidateOutcome>,
    pub summary: ExperimentSummary,
}

#[derive(Clone)]
pub struct OfflineImproveRunner<S: SessionStore + ?Sized> {
    archive: ExperimentArchive<S>,
}

impl<S: SessionStore + ?Sized> OfflineImproveRunner<S> {
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self {
            archive: ExperimentArchive::new(store),
        }
    }

    pub async fn run(
        &self,
        plan: OfflineImprovePlan,
    ) -> std::result::Result<ImproveRunOutcome, MetaError> {
        if plan.candidates.is_empty() {
            return Err(MetaError::InvalidPlan(
                "improve plan must define at least one candidate".to_string(),
            ));
        }

        let experiment_id = plan.experiment_id.unwrap_or_default();
        self.archive
            .start(experiment_id.clone(), plan.spec.clone())
            .await?;
        self.archive
            .pin_baseline(&experiment_id, plan.baseline.clone())
            .await?;

        let mut outcomes: Vec<ImprovementCandidateOutcome> =
            Vec::with_capacity(plan.candidates.len());
        let mut winner_index: Option<usize> = None;

        for candidate_plan in plan.candidates {
            let candidate = CandidateSpec {
                candidate_id: candidate_plan.candidate_id.unwrap_or_default(),
                baseline_id: plan.baseline.baseline_id.clone(),
                target: plan.spec.target,
                label: candidate_plan.label,
                description: candidate_plan.description,
                config: candidate_plan.config,
            };
            self.archive
                .record_candidate(&experiment_id, candidate.clone())
                .await?;

            let evaluation = evaluate_candidate(
                &experiment_id,
                &plan.spec,
                &plan.baseline,
                &candidate,
                &plan.evaluators,
            )
            .await?;
            self.archive
                .record_evaluation(&experiment_id, &evaluation)
                .await?;

            let decision = plan.promotion_gate.decide(&PromotionInput {
                candidate_id: candidate.candidate_id.clone(),
                evaluation: evaluation.clone(),
                critic_report: candidate_plan.critic_report,
            });

            if decision.kind == PromotionDecisionKind::Promoted {
                if let Some(index) = winner_index {
                    // Preserve first-wins semantics on equal scores so plan order
                    // remains the deterministic tie-breaker for promotable variants.
                    let current_best = outcomes[index]
                        .evaluation
                        .score
                        .unwrap_or(f64::NEG_INFINITY);
                    let candidate_score = evaluation.score.unwrap_or(f64::NEG_INFINITY);
                    if candidate_score > current_best {
                        winner_index = Some(outcomes.len());
                    }
                } else {
                    winner_index = Some(outcomes.len());
                }
            }

            outcomes.push(ImprovementCandidateOutcome {
                candidate,
                evaluation,
                decision,
            });
        }

        // Record non-winning outcomes first so experiment summaries retain the
        // promoted winner as the last decision when a winner exists.
        for (index, outcome) in outcomes.iter_mut().enumerate() {
            if outcome.decision.kind != PromotionDecisionKind::Promoted {
                self.archive
                    .record_decision(
                        &experiment_id,
                        outcome.candidate.candidate_id.clone(),
                        outcome.decision.clone(),
                    )
                    .await?;
                continue;
            }

            if Some(index) == winner_index {
                continue;
            }

            outcome.decision = PromotionDecision {
                kind: PromotionDecisionKind::Rejected,
                reason: format!(
                    "candidate {} passed promotion gate but was not the top-scoring promotable variant",
                    outcome.candidate.candidate_id
                ),
            };
            self.archive
                .record_decision(
                    &experiment_id,
                    outcome.candidate.candidate_id.clone(),
                    outcome.decision.clone(),
                )
                .await?;
        }

        let winner_candidate_id =
            winner_index.map(|index| outcomes[index].candidate.candidate_id.clone());
        if let Some(index) = winner_index {
            let outcome = &outcomes[index];
            self.archive
                .record_decision(
                    &experiment_id,
                    outcome.candidate.candidate_id.clone(),
                    outcome.decision.clone(),
                )
                .await?;
        }

        let summary = self
            .archive
            .summary(&experiment_id)
            .await?
            .unwrap_or_else(|| {
                panic!(
                    "experiment summary missing after improve run: {}",
                    experiment_id
                )
            });

        Ok(ImproveRunOutcome {
            experiment_id,
            winner_candidate_id,
            candidates: outcomes,
            summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ImprovementCandidatePlan, OfflineImprovePlan, OfflineImproveRunner};
    use crate::{CriticFinding, CriticReport, CriticSeverity};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use std::sync::Arc;
    use store::{InMemorySessionStore, SessionStore};
    use types::{
        BaselineId, BaselineSpec, CandidateId, ExperimentId, ExperimentSpec, ExperimentTarget,
        PromotionDecisionKind,
    };

    fn plan() -> OfflineImprovePlan {
        OfflineImprovePlan {
            experiment_id: Some(ExperimentId::from("experiment-improve")),
            spec: ExperimentSpec {
                target: ExperimentTarget::Policy,
                goal: "select the strongest approval policy variant".to_string(),
                source_session_id: Some("session-improve".into()),
                source_agent_session_id: Some("agent-improve".into()),
                metadata: json!({"pack":"policy-tournament"}),
            },
            baseline: BaselineSpec {
                baseline_id: BaselineId::from("baseline-improve"),
                target: ExperimentTarget::Policy,
                label: "policy-v1".to_string(),
                description: None,
                config: Some(json!({"profile":"baseline"})),
            },
            candidates: vec![
                ImprovementCandidatePlan {
                    candidate_id: Some(CandidateId::from("candidate-mid")),
                    label: "policy-v2".to_string(),
                    description: Some("raise approval score modestly".to_string()),
                    config: json!({
                        "profile": "strict",
                        "metrics": { "score": 0.91 }
                    }),
                    critic_report: CriticReport::default(),
                },
                ImprovementCandidatePlan {
                    candidate_id: Some(CandidateId::from("candidate-best")),
                    label: "policy-v3".to_string(),
                    description: Some("raise approval score further".to_string()),
                    config: json!({
                        "profile": "strict-plus",
                        "metrics": { "score": 0.96 }
                    }),
                    critic_report: CriticReport::default(),
                },
            ],
            evaluators: vec![
                evals::BuiltinEvaluatorSpec::ConfigPointerDefined {
                    evaluator_name: "profile_defined".to_string(),
                    pointer: "/profile".to_string(),
                },
                evals::BuiltinEvaluatorSpec::ConfigPointerEquals {
                    evaluator_name: "profile_is_strict_plus".to_string(),
                    pointer: "/profile".to_string(),
                    expected: json!("strict-plus"),
                },
            ],
            promotion_gate: crate::ThresholdPromotionGate {
                minimum_score: Some(0.4),
                require_passed: false,
                ..crate::ThresholdPromotionGate::default()
            },
        }
    }

    #[test]
    fn improve_runner_promotes_only_top_scoring_candidate() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let runner = OfflineImproveRunner::new(store.clone());

            let outcome = runner.run(plan()).await.unwrap();

            assert_eq!(
                outcome.winner_candidate_id,
                Some(CandidateId::from("candidate-best"))
            );
            assert_eq!(outcome.candidates.len(), 2);
            assert_eq!(
                outcome.candidates[0].decision.kind,
                PromotionDecisionKind::Rejected
            );
            assert!(
                outcome.candidates[0]
                    .decision
                    .reason
                    .contains("top-scoring promotable variant")
            );
            assert_eq!(
                outcome.candidates[1].decision.kind,
                PromotionDecisionKind::Promoted
            );
            assert_eq!(outcome.summary.candidate_count, 2);
            assert_eq!(
                outcome.summary.promoted_candidate_id,
                Some(CandidateId::from("candidate-best"))
            );
            assert_eq!(
                outcome.summary.last_decision,
                Some(PromotionDecisionKind::Promoted)
            );

            let experiments = store.list_experiments().await.unwrap();
            assert_eq!(experiments.len(), 1);
        });
    }

    #[test]
    fn improve_runner_rejects_invalid_empty_plan() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let runner = OfflineImproveRunner::new(store);
            let mut plan = plan();
            plan.candidates.clear();

            let error = runner.run(plan).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("improve plan must define at least one candidate")
            );
        });
    }

    #[test]
    fn improve_runner_keeps_blocked_candidates_out_of_winner_set() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let runner = OfflineImproveRunner::new(store);
            let mut plan = plan();
            plan.candidates[1].critic_report = CriticReport {
                findings: vec![CriticFinding {
                    code: "unsafe_regression".to_string(),
                    severity: CriticSeverity::Blocking,
                    summary: "unsafe regression".to_string(),
                }],
            };

            let outcome = runner.run(plan).await.unwrap();
            assert_eq!(
                outcome.winner_candidate_id,
                Some(CandidateId::from("candidate-mid"))
            );
            assert_eq!(
                outcome.candidates[0].decision.kind,
                PromotionDecisionKind::Promoted
            );
            assert_eq!(
                outcome.candidates[1].decision.kind,
                PromotionDecisionKind::Rejected
            );
            assert!(
                outcome.candidates[1]
                    .decision
                    .reason
                    .contains("unsafe regression")
            );
        });
    }
}
