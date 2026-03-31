use std::sync::Arc;
use store::{ArtifactSummary, Result, SessionStore, summarize_artifact_events};
use types::{
    ArtifactEvaluationSummary, ArtifactId, ArtifactLedgerEventEnvelope, ArtifactLedgerEventKind,
    ArtifactPromotionDecision, ArtifactPromotionDecisionKind, ArtifactVersion, ArtifactVersionId,
};

#[derive(Clone)]
pub struct ArtifactArchive<S: SessionStore + ?Sized> {
    // Artifact history stays append-only so promotion, rejection, and rollback
    // decisions remain auditable after the active version pointer changes.
    store: Arc<S>,
}

impl<S: SessionStore + ?Sized> ArtifactArchive<S> {
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    pub async fn propose_version(
        &self,
        artifact_id: ArtifactId,
        version: ArtifactVersion,
    ) -> Result<()> {
        self.store
            .append_artifact(ArtifactLedgerEventEnvelope::new(
                artifact_id,
                ArtifactLedgerEventKind::VersionProposed { version },
            ))
            .await
    }

    pub async fn record_evaluation(
        &self,
        artifact_id: &ArtifactId,
        version_id: ArtifactVersionId,
        evaluation: ArtifactEvaluationSummary,
    ) -> Result<()> {
        self.store
            .append_artifact(ArtifactLedgerEventEnvelope::new(
                artifact_id.clone(),
                ArtifactLedgerEventKind::VersionEvaluated {
                    version_id,
                    evaluation,
                },
            ))
            .await
    }

    pub async fn record_decision(
        &self,
        artifact_id: &ArtifactId,
        version_id: ArtifactVersionId,
        decision: ArtifactPromotionDecision,
    ) -> Result<()> {
        let event = match decision.kind {
            ArtifactPromotionDecisionKind::Promoted => ArtifactLedgerEventKind::VersionPromoted {
                version_id,
                decision,
            },
            ArtifactPromotionDecisionKind::Rejected => ArtifactLedgerEventKind::VersionRejected {
                version_id,
                decision,
            },
            ArtifactPromotionDecisionKind::RolledBack => {
                ArtifactLedgerEventKind::VersionRolledBack {
                    version_id,
                    decision,
                }
            }
        };
        self.store
            .append_artifact(ArtifactLedgerEventEnvelope::new(artifact_id.clone(), event))
            .await
    }

    pub async fn summary(&self, artifact_id: &ArtifactId) -> Result<Option<ArtifactSummary>> {
        let events = self.store.artifact_events(artifact_id).await?;
        Ok(summarize_artifact_events(artifact_id, &events))
    }

    pub async fn events(
        &self,
        artifact_id: &ArtifactId,
    ) -> Result<Vec<ArtifactLedgerEventEnvelope>> {
        self.store.artifact_events(artifact_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::ArtifactArchive;
    use nanoclaw_test_support::run_current_thread_test;
    use std::sync::Arc;
    use store::{InMemorySessionStore, SessionStore};
    use types::{
        ArtifactEvaluationSummary, ArtifactId, ArtifactKind, ArtifactPromotionDecision,
        ArtifactPromotionDecisionKind, ArtifactVersion, ArtifactVersionId,
    };

    #[test]
    fn archive_records_artifact_version_lifecycle() {
        run_current_thread_test(async {
            let store = Arc::new(InMemorySessionStore::new());
            let archive = ArtifactArchive::new(store.clone());
            let artifact_id = ArtifactId::from("artifact-runtime-prompt");
            let version_id = ArtifactVersionId::from("artifact-runtime-prompt-v2");

            archive
                .propose_version(
                    artifact_id.clone(),
                    ArtifactVersion {
                        version_id: version_id.clone(),
                        kind: ArtifactKind::Prompt,
                        label: "runtime-prompt-v2".to_string(),
                        description: Some("tighten retry instructions".to_string()),
                        parent_version_id: Some("artifact-runtime-prompt-v1".into()),
                        source_signal_ids: vec!["signal-1".into()],
                        source_task_ids: vec!["task-1".to_string()],
                        source_case_ids: vec!["case-1".to_string()],
                        payload: serde_json::json!({"prompt":"new prompt"}),
                        metadata: serde_json::json!({"owner":"nanoclaw"}),
                    },
                )
                .await
                .unwrap();
            archive
                .record_evaluation(
                    &artifact_id,
                    version_id.clone(),
                    ArtifactEvaluationSummary {
                        summary: "clears self-regression validation".to_string(),
                        verifier_summary: serde_json::json!({"suite":"self-regression"}),
                    },
                )
                .await
                .unwrap();
            archive
                .record_decision(
                    &artifact_id,
                    version_id.clone(),
                    ArtifactPromotionDecision {
                        kind: ArtifactPromotionDecisionKind::Promoted,
                        reason: "validation cleared".to_string(),
                        rollback_version_id: Some("artifact-runtime-prompt-v1".into()),
                    },
                )
                .await
                .unwrap();

            let summary = archive.summary(&artifact_id).await.unwrap().unwrap();
            assert_eq!(summary.version_count, 1);
            assert_eq!(summary.promoted_version_id, Some(version_id));
            assert_eq!(
                summary.last_decision,
                Some(ArtifactPromotionDecisionKind::Promoted)
            );

            let listed = store.list_artifacts().await.unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].artifact_id, artifact_id);
        });
    }
}
