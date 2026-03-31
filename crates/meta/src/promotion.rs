use crate::CriticReport;
use evals::EvaluationReport;
use serde::{Deserialize, Serialize};
use types::{CandidateId, PromotionDecision, PromotionDecisionKind};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionInput {
    pub candidate_id: CandidateId,
    pub evaluation: EvaluationReport,
    #[serde(default)]
    pub critic_report: CriticReport,
}

pub trait PromotionGate {
    fn decide(&self, input: &PromotionInput) -> PromotionDecision;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThresholdPromotionGate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_score: Option<f64>,
    #[serde(default = "default_true")]
    pub require_passed: bool,
    #[serde(default = "default_true")]
    pub reject_on_blocking_findings: bool,
}

impl Default for ThresholdPromotionGate {
    fn default() -> Self {
        Self {
            minimum_score: None,
            require_passed: true,
            reject_on_blocking_findings: true,
        }
    }
}

impl PromotionGate for ThresholdPromotionGate {
    fn decide(&self, input: &PromotionInput) -> PromotionDecision {
        if self.reject_on_blocking_findings && input.critic_report.has_blockers() {
            return PromotionDecision {
                kind: PromotionDecisionKind::Rejected,
                reason: input
                    .critic_report
                    .rejection_reason()
                    .unwrap_or_else(|| "blocking critic findings present".to_string()),
            };
        }
        if self.require_passed && !input.evaluation.passed {
            return PromotionDecision {
                kind: PromotionDecisionKind::Rejected,
                reason: format!("evaluation failed: {}", input.evaluation.summary),
            };
        }
        if let Some(minimum_score) = self.minimum_score {
            let score = input.evaluation.score.unwrap_or_default();
            if score < minimum_score {
                return PromotionDecision {
                    kind: PromotionDecisionKind::Rejected,
                    reason: format!(
                        "score {:.3} below promotion threshold {:.3}",
                        score, minimum_score
                    ),
                };
            }
        }

        PromotionDecision {
            kind: PromotionDecisionKind::Promoted,
            reason: format!("candidate {} passed promotion gate", input.candidate_id),
        }
    }
}

const fn default_true() -> bool {
    true
}
