use serde::{Deserialize, Serialize};
use types::{BaselineSpec, CandidateSpec, ExperimentId, ExperimentTarget};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateSubmission {
    pub experiment_id: ExperimentId,
    pub candidate: CandidateSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineSpec>,
}

impl CandidateSubmission {
    #[must_use]
    pub fn target(&self) -> ExperimentTarget {
        self.candidate.target
    }
}
