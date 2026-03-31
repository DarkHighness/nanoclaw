use serde::{Deserialize, Serialize};
use types::{BaselineId, CandidateId, PromotionDecision, PromotionDecisionKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RollbackAction {
    RestoreBaseline { baseline_id: BaselineId },
    AnnotateExperiment { note: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackPlan {
    pub candidate_id: CandidateId,
    pub reason: String,
    #[serde(default)]
    pub actions: Vec<RollbackAction>,
}

impl RollbackPlan {
    #[must_use]
    pub fn decision(&self) -> PromotionDecision {
        PromotionDecision {
            kind: PromotionDecisionKind::RolledBack,
            reason: self.reason.clone(),
        }
    }
}
