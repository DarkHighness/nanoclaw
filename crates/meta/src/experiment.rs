use evals::EvaluationReport;
use std::sync::Arc;
use store::{ExperimentSummary, Result, SessionStore, summarize_experiment_events};
use types::{
    BaselineSpec, CandidateId, CandidateSpec, ExperimentEventEnvelope, ExperimentEventKind,
    ExperimentId, ExperimentSpec, PromotionDecision, PromotionDecisionKind,
};

#[derive(Clone)]
pub struct ExperimentArchive<S: SessionStore + ?Sized> {
    // Experiment history is append-only because promotion and rollback need an
    // audit trail that survives candidate replacement and baseline rotation.
    store: Arc<S>,
}

impl<S: SessionStore + ?Sized> ExperimentArchive<S> {
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    pub async fn start(&self, experiment_id: ExperimentId, spec: ExperimentSpec) -> Result<()> {
        self.store
            .append_experiment(ExperimentEventEnvelope::new(
                experiment_id,
                ExperimentEventKind::Started { spec },
            ))
            .await
    }

    pub async fn pin_baseline(
        &self,
        experiment_id: &ExperimentId,
        baseline: BaselineSpec,
    ) -> Result<()> {
        self.store
            .append_experiment(ExperimentEventEnvelope::new(
                experiment_id.clone(),
                ExperimentEventKind::BaselinePinned { baseline },
            ))
            .await
    }

    pub async fn record_candidate(
        &self,
        experiment_id: &ExperimentId,
        candidate: CandidateSpec,
    ) -> Result<()> {
        self.store
            .append_experiment(ExperimentEventEnvelope::new(
                experiment_id.clone(),
                ExperimentEventKind::CandidateGenerated { candidate },
            ))
            .await
    }

    pub async fn record_evaluation(
        &self,
        experiment_id: &ExperimentId,
        report: &EvaluationReport,
    ) -> Result<()> {
        self.store
            .append_experiment(ExperimentEventEnvelope::new(
                experiment_id.clone(),
                ExperimentEventKind::CandidateEvaluated {
                    evaluation: report.to_candidate_summary(),
                },
            ))
            .await
    }

    pub async fn record_decision(
        &self,
        experiment_id: &ExperimentId,
        candidate_id: CandidateId,
        decision: PromotionDecision,
    ) -> Result<()> {
        let event = match decision.kind {
            PromotionDecisionKind::Promoted => ExperimentEventKind::CandidatePromoted {
                candidate_id,
                decision,
            },
            PromotionDecisionKind::Rejected => ExperimentEventKind::CandidateRejected {
                candidate_id,
                decision,
            },
            PromotionDecisionKind::RolledBack => ExperimentEventKind::CandidateRolledBack {
                candidate_id,
                decision,
            },
        };
        self.store
            .append_experiment(ExperimentEventEnvelope::new(experiment_id.clone(), event))
            .await
    }

    pub async fn summary(&self, experiment_id: &ExperimentId) -> Result<Option<ExperimentSummary>> {
        let events = self.store.experiment_events(experiment_id).await?;
        Ok(summarize_experiment_events(experiment_id, &events))
    }

    pub async fn events(
        &self,
        experiment_id: &ExperimentId,
    ) -> Result<Vec<ExperimentEventEnvelope>> {
        self.store.experiment_events(experiment_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::ExperimentArchive;
    use crate::{CriticReport, PromotionGate, PromotionInput, ThresholdPromotionGate};
    use evals::{EvaluationReport, EvaluatorOutcome};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::json;
    use std::sync::Arc;
    use store::{InMemorySessionStore, SessionStore};
    use types::{
        AgentSessionId, BaselineId, BaselineSpec, CandidateId, CandidateSpec, ExperimentId,
        ExperimentSpec, ExperimentTarget, PromotionDecisionKind, SessionId,
    };

    #[test]
    fn archive_writes_full_candidate_lifecycle() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let archive = ExperimentArchive::new(store.clone());
            let experiment_id = ExperimentId::from("experiment-archive");
            let baseline_id = BaselineId::from("baseline-archive");
            let candidate_id = CandidateId::from("candidate-archive");

            archive
                .start(
                    experiment_id.clone(),
                    ExperimentSpec {
                        target: ExperimentTarget::Prompt,
                        goal: "reduce planner retry churn".to_string(),
                        source_session_id: Some(SessionId::from("session-archive")),
                        source_agent_session_id: Some(AgentSessionId::from("agent-archive")),
                        metadata: json!({"suite":"planner-regression"}),
                    },
                )
                .await
                .unwrap();
            archive
                .pin_baseline(
                    &experiment_id,
                    BaselineSpec {
                        baseline_id: baseline_id.clone(),
                        target: ExperimentTarget::Prompt,
                        label: "prompt-v1".to_string(),
                        description: None,
                        config: None,
                    },
                )
                .await
                .unwrap();
            archive
                .record_candidate(
                    &experiment_id,
                    CandidateSpec {
                        candidate_id: candidate_id.clone(),
                        baseline_id,
                        target: ExperimentTarget::Prompt,
                        label: "prompt-v2".to_string(),
                        description: Some("tighten planning rubric".to_string()),
                        config: json!({"profile":"v2"}),
                    },
                )
                .await
                .unwrap();

            let report = EvaluationReport::from_evaluator_outcomes(
                candidate_id.clone(),
                vec![EvaluatorOutcome {
                    evaluator_name: "schema".to_string(),
                    passed: true,
                    score: Some(0.95),
                    summary: "schema valid".to_string(),
                    details: None,
                }],
            );
            archive
                .record_evaluation(&experiment_id, &report)
                .await
                .unwrap();

            let decision = ThresholdPromotionGate {
                minimum_score: Some(0.9),
                ..ThresholdPromotionGate::default()
            }
            .decide(&PromotionInput {
                candidate_id: candidate_id.clone(),
                evaluation: report,
                critic_report: CriticReport::default(),
            });
            archive
                .record_decision(&experiment_id, candidate_id, decision)
                .await
                .unwrap();

            let summary = archive.summary(&experiment_id).await.unwrap().unwrap();
            assert_eq!(summary.candidate_count, 1);
            assert_eq!(summary.baseline_count, 1);
            assert_eq!(summary.last_decision, Some(PromotionDecisionKind::Promoted));
            assert_eq!(summary.goal.as_deref(), Some("reduce planner retry churn"));

            let listed = store.list_experiments().await.unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].experiment_id, experiment_id);
        });
    }
}
