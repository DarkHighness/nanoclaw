use serde::{Deserialize, Serialize};
use serde_json::Value;
use types::{
    AgentSessionId, BaselineSpec, CandidateSpec, ExperimentId, ExperimentSpec, ExperimentTarget,
    SessionId,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationContext {
    pub experiment_id: ExperimentId,
    pub target: ExperimentTarget,
    pub goal: String,
    pub candidate: CandidateSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent_session_id: Option<AgentSessionId>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl EvaluationContext {
    #[must_use]
    pub fn for_candidate(
        experiment_id: ExperimentId,
        spec: &ExperimentSpec,
        candidate: CandidateSpec,
        baseline: Option<BaselineSpec>,
    ) -> Self {
        Self {
            experiment_id,
            target: candidate.target,
            goal: spec.goal.clone(),
            candidate,
            baseline,
            source_session_id: spec.source_session_id.clone(),
            source_agent_session_id: spec.source_agent_session_id.clone(),
            metadata: spec.metadata.clone(),
        }
    }
}
