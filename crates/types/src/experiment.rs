use crate::{AgentSessionId, BaselineId, CandidateId, EventId, ExperimentId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentTarget {
    Prompt,
    Skill,
    Policy,
    Workflow,
    CodePatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExperimentSpec {
    pub target: ExperimentTarget,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent_session_id: Option<AgentSessionId>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BaselineSpec {
    pub baseline_id: BaselineId,
    pub target: ExperimentTarget,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CandidateSpec {
    pub candidate_id: CandidateId,
    pub baseline_id: BaselineId,
    pub target: ExperimentTarget,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub config: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvaluatorSummary {
    pub evaluator_name: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CandidateEvaluationSummary {
    pub candidate_id: CandidateId,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub summary: String,
    #[serde(default)]
    pub evaluators: Vec<EvaluatorSummary>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDecisionKind {
    Promoted,
    Rejected,
    RolledBack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromotionDecision {
    pub kind: PromotionDecisionKind,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExperimentEventKind {
    Started {
        spec: ExperimentSpec,
    },
    BaselinePinned {
        baseline: BaselineSpec,
    },
    CandidateGenerated {
        candidate: CandidateSpec,
    },
    CandidateEvaluated {
        evaluation: CandidateEvaluationSummary,
    },
    CandidatePromoted {
        candidate_id: CandidateId,
        decision: PromotionDecision,
    },
    CandidateRejected {
        candidate_id: CandidateId,
        decision: PromotionDecision,
    },
    CandidateRolledBack {
        candidate_id: CandidateId,
        decision: PromotionDecision,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExperimentEventEnvelope {
    pub id: EventId,
    pub experiment_id: ExperimentId,
    pub timestamp_ms: u128,
    pub event: ExperimentEventKind,
}

impl ExperimentEventEnvelope {
    #[must_use]
    pub fn new(experiment_id: ExperimentId, event: ExperimentEventKind) -> Self {
        Self {
            id: EventId::new(),
            experiment_id,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| value.as_millis()),
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BaselineSpec, CandidateEvaluationSummary, CandidateSpec, EvaluatorSummary,
        ExperimentEventEnvelope, ExperimentEventKind, ExperimentSpec, ExperimentTarget,
        PromotionDecision, PromotionDecisionKind,
    };
    use crate::{BaselineId, CandidateId, ExperimentId};

    #[test]
    fn experiment_envelope_captures_candidate_lifecycle() {
        let experiment_id = ExperimentId::from("experiment_demo");
        let baseline_id = BaselineId::from("baseline_demo");
        let candidate_id = CandidateId::from("candidate_demo");
        let started = ExperimentEventEnvelope::new(
            experiment_id.clone(),
            ExperimentEventKind::Started {
                spec: ExperimentSpec {
                    target: ExperimentTarget::Prompt,
                    goal: "reduce false positives in review mode".to_string(),
                    source_session_id: None,
                    source_agent_session_id: None,
                    metadata: serde_json::json!({"suite":"review-smoke"}),
                },
            },
        );
        let generated = ExperimentEventEnvelope::new(
            experiment_id.clone(),
            ExperimentEventKind::CandidateGenerated {
                candidate: CandidateSpec {
                    candidate_id: candidate_id.clone(),
                    baseline_id: baseline_id.clone(),
                    target: ExperimentTarget::Prompt,
                    label: "prompt-v2".to_string(),
                    description: Some("tighten reviewer rubric".to_string()),
                    config: serde_json::json!({"profile":"reviewer_v2"}),
                },
            },
        );
        let evaluated = ExperimentEventEnvelope::new(
            experiment_id.clone(),
            ExperimentEventKind::CandidateEvaluated {
                evaluation: CandidateEvaluationSummary {
                    candidate_id: candidate_id.clone(),
                    passed: true,
                    score: Some(0.91),
                    summary: "candidate clears required evaluators".to_string(),
                    evaluators: vec![EvaluatorSummary {
                        evaluator_name: "schema".to_string(),
                        passed: true,
                        score: Some(1.0),
                        summary: "schema valid".to_string(),
                        details: None,
                    }],
                },
            },
        );
        let promoted = ExperimentEventEnvelope::new(
            experiment_id,
            ExperimentEventKind::CandidatePromoted {
                candidate_id: candidate_id.clone(),
                decision: PromotionDecision {
                    kind: PromotionDecisionKind::Promoted,
                    reason: "improved benchmark score".to_string(),
                },
            },
        );

        match started.event {
            ExperimentEventKind::Started { spec } => {
                assert_eq!(spec.target, ExperimentTarget::Prompt);
            }
            other => panic!("unexpected started event: {other:?}"),
        }
        match generated.event {
            ExperimentEventKind::CandidateGenerated { candidate } => {
                assert_eq!(candidate.candidate_id, candidate_id);
                assert_eq!(candidate.baseline_id, baseline_id);
            }
            other => panic!("unexpected generated event: {other:?}"),
        }
        match evaluated.event {
            ExperimentEventKind::CandidateEvaluated { evaluation } => {
                assert!(evaluation.passed);
                assert_eq!(evaluation.evaluators.len(), 1);
            }
            other => panic!("unexpected evaluated event: {other:?}"),
        }
        match promoted.event {
            ExperimentEventKind::CandidatePromoted { decision, .. } => {
                assert_eq!(decision.kind, PromotionDecisionKind::Promoted);
            }
            other => panic!("unexpected promoted event: {other:?}"),
        }
    }

    #[test]
    fn baseline_event_retains_optional_config() {
        let envelope = ExperimentEventEnvelope::new(
            "experiment_1".into(),
            ExperimentEventKind::BaselinePinned {
                baseline: BaselineSpec {
                    baseline_id: "baseline_1".into(),
                    target: ExperimentTarget::Skill,
                    label: "skill-baseline".to_string(),
                    description: None,
                    config: Some(serde_json::json!({"skill":"docs_research"})),
                },
            },
        );

        match envelope.event {
            ExperimentEventKind::BaselinePinned { baseline } => {
                assert_eq!(baseline.target, ExperimentTarget::Skill);
                assert_eq!(baseline.config.unwrap()["skill"], "docs_research");
            }
            other => panic!("unexpected baseline event: {other:?}"),
        }
    }
}
