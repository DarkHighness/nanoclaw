use crate::{ArtifactId, ArtifactVersionId, EventId, ExperimentId, SignalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Prompt,
    Skill,
    Workflow,
    Hook,
    Verifier,
    RuntimePatch,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactSourceRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<ExperimentId>,
    #[serde(default)]
    pub signal_ids: Vec<SignalId>,
    #[serde(default)]
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub case_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactVersion {
    pub version_id: ArtifactVersionId,
    pub kind: ArtifactKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_version_id: Option<ArtifactVersionId>,
    #[serde(default)]
    pub source_signal_ids: Vec<SignalId>,
    #[serde(default)]
    pub source_task_ids: Vec<String>,
    #[serde(default)]
    pub source_case_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPromotionDecisionKind {
    Promoted,
    Rejected,
    RolledBack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactPromotionDecision {
    pub kind: ArtifactPromotionDecisionKind,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_version_id: Option<ArtifactVersionId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactEvaluationSummary {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub verifier_summary: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactLedgerEventKind {
    VersionProposed {
        version: ArtifactVersion,
    },
    VersionEvaluated {
        version_id: ArtifactVersionId,
        evaluation: ArtifactEvaluationSummary,
    },
    VersionPromoted {
        version_id: ArtifactVersionId,
        decision: ArtifactPromotionDecision,
    },
    VersionRejected {
        version_id: ArtifactVersionId,
        decision: ArtifactPromotionDecision,
    },
    VersionRolledBack {
        version_id: ArtifactVersionId,
        decision: ArtifactPromotionDecision,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactLedgerEventEnvelope {
    pub id: EventId,
    pub artifact_id: ArtifactId,
    pub timestamp_ms: u128,
    pub event: ArtifactLedgerEventKind,
}

impl ArtifactLedgerEventEnvelope {
    #[must_use]
    pub fn new(artifact_id: ArtifactId, event: ArtifactLedgerEventKind) -> Self {
        Self {
            id: EventId::new(),
            artifact_id,
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
        ArtifactKind, ArtifactLedgerEventEnvelope, ArtifactLedgerEventKind,
        ArtifactPromotionDecision, ArtifactPromotionDecisionKind, ArtifactVersion,
    };
    use crate::{ArtifactId, ArtifactVersionId};
    use serde_json::Value;

    #[test]
    fn artifact_event_envelope_keeps_registered_versions() {
        let envelope = ArtifactLedgerEventEnvelope::new(
            ArtifactId::from("artifact-prompt"),
            ArtifactLedgerEventKind::VersionProposed {
                version: ArtifactVersion {
                    version_id: ArtifactVersionId::from("version-1"),
                    kind: ArtifactKind::Prompt,
                    label: "prompt-v1".to_string(),
                    description: Some("baseline prompt".to_string()),
                    parent_version_id: None,
                    source_signal_ids: vec!["signal-1".into()],
                    source_task_ids: vec!["task-1".to_string()],
                    source_case_ids: vec!["case-1".to_string()],
                    payload: serde_json::json!({"prompt":"baseline"}),
                    metadata: Value::Null,
                },
            },
        );

        match envelope.event {
            ArtifactLedgerEventKind::VersionProposed { version } => {
                assert_eq!(version.kind, ArtifactKind::Prompt);
                assert_eq!(version.label, "prompt-v1");
            }
            other => panic!("unexpected artifact event: {other:?}"),
        }
    }

    #[test]
    fn promotion_decision_retains_reason() {
        let decision = ArtifactPromotionDecision {
            kind: ArtifactPromotionDecisionKind::Promoted,
            reason: "passed self-regression corpus".to_string(),
            rollback_version_id: Some(ArtifactVersionId::from("version-1")),
        };

        let event = ArtifactLedgerEventEnvelope::new(
            ArtifactId::from("artifact-prompt"),
            ArtifactLedgerEventKind::VersionPromoted {
                version_id: ArtifactVersionId::from("version-2"),
                decision: decision.clone(),
            },
        );

        match event.event {
            ArtifactLedgerEventKind::VersionPromoted {
                decision: stored, ..
            } => {
                assert_eq!(stored.kind, ArtifactPromotionDecisionKind::Promoted);
                assert_eq!(stored.reason, decision.reason);
                assert_eq!(stored.rollback_version_id, decision.rollback_version_id);
            }
            other => panic!("unexpected artifact event: {other:?}"),
        }
    }
}
