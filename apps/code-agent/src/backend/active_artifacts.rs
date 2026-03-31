use agent::types::{ArtifactId, ArtifactKind, ArtifactLedgerEventEnvelope, ArtifactVersion};
use anyhow::Result;
use futures::{StreamExt, TryStreamExt, stream};
use std::sync::Arc;
use store::{ArtifactSummary, SessionStore};

const ACTIVE_ARTIFACT_FETCH_CONCURRENCY_LIMIT: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActiveArtifactVersion {
    pub(crate) artifact_id: ArtifactId,
    pub(crate) version: ArtifactVersion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveArtifactStartupEntry {
    pub(crate) artifact_ref: String,
    pub(crate) version_ref: String,
    pub(crate) kind: ArtifactKind,
    pub(crate) label: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ActiveArtifactSet {
    pub(crate) versions: Vec<ActiveArtifactVersion>,
    pub(crate) warnings: Vec<String>,
}

impl ActiveArtifactSet {
    pub(crate) fn startup_entries(&self) -> Vec<ActiveArtifactStartupEntry> {
        self.versions
            .iter()
            .map(|entry| ActiveArtifactStartupEntry {
                artifact_ref: entry.artifact_id.to_string(),
                version_ref: entry.version.version_id.to_string(),
                kind: entry.version.kind,
                label: entry.version.label.clone(),
            })
            .collect()
    }
}

pub(crate) async fn load_active_artifacts(
    store: &Arc<dyn SessionStore>,
) -> Result<ActiveArtifactSet> {
    let active_summaries = store
        .list_artifacts()
        .await?
        .into_iter()
        .filter(|summary| summary.active_version_id.is_some())
        .collect::<Vec<_>>();
    if active_summaries.is_empty() {
        return Ok(ActiveArtifactSet::default());
    }

    let tasks = active_summaries
        .into_iter()
        .enumerate()
        .map(|(index, summary)| {
            let store = store.clone();
            async move {
                let events = store.artifact_events(&summary.artifact_id).await?;
                Ok::<_, store::SessionStoreError>((
                    index,
                    resolve_active_artifact_summary(summary, &events),
                ))
            }
        })
        .collect::<Vec<_>>();
    let mut indexed = stream::iter(tasks)
        .buffer_unordered(ACTIVE_ARTIFACT_FETCH_CONCURRENCY_LIMIT)
        .try_collect::<Vec<_>>()
        .await?;
    indexed.sort_by_key(|(index, _)| *index);

    let mut versions = Vec::new();
    let mut warnings = Vec::new();
    for (_, resolved) in indexed {
        if let Some(version) = resolved.version {
            versions.push(version);
        }
        if let Some(warning) = resolved.warning {
            warnings.push(warning);
        }
    }

    // Persisted event order is append-only, but boot should still produce a
    // deterministic overlay order so repeated startups do not reshuffle the
    // active guidance that gets injected into new runtimes.
    versions.sort_by(|left, right| {
        artifact_kind_sort_key(left.version.kind)
            .cmp(&artifact_kind_sort_key(right.version.kind))
            .then_with(|| left.artifact_id.as_str().cmp(right.artifact_id.as_str()))
            .then_with(|| {
                left.version
                    .version_id
                    .as_str()
                    .cmp(right.version.version_id.as_str())
            })
    });

    Ok(ActiveArtifactSet { versions, warnings })
}

#[derive(Debug)]
struct ResolvedActiveArtifact {
    version: Option<ActiveArtifactVersion>,
    warning: Option<String>,
}

fn resolve_active_artifact_summary(
    summary: ArtifactSummary,
    events: &[ArtifactLedgerEventEnvelope],
) -> ResolvedActiveArtifact {
    let Some(active_version_id) = summary.active_version_id.clone() else {
        return ResolvedActiveArtifact {
            version: None,
            warning: None,
        };
    };

    let version = events.iter().find_map(|event| match &event.event {
        agent::types::ArtifactLedgerEventKind::VersionProposed { version }
            if version.version_id == active_version_id =>
        {
            Some(version.clone())
        }
        _ => None,
    });

    match version {
        Some(version) => ResolvedActiveArtifact {
            version: Some(ActiveArtifactVersion {
                artifact_id: summary.artifact_id,
                version,
            }),
            warning: None,
        },
        None => ResolvedActiveArtifact {
            version: None,
            warning: Some(format!(
                "active artifact {} points to missing proposed version {}; skipped startup overlay",
                summary.artifact_id, active_version_id
            )),
        },
    }
}

fn artifact_kind_sort_key(kind: ArtifactKind) -> u8 {
    match kind {
        ArtifactKind::Prompt => 0,
        ArtifactKind::Skill => 1,
        ArtifactKind::Workflow => 2,
        ArtifactKind::Hook => 3,
        ArtifactKind::Verifier => 4,
        ArtifactKind::RuntimePatch => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::load_active_artifacts;
    use agent::types::{
        ArtifactId, ArtifactLedgerEventEnvelope, ArtifactLedgerEventKind,
        ArtifactPromotionDecision, ArtifactPromotionDecisionKind, ArtifactVersion,
        ArtifactVersionId,
    };
    use serde_json::json;
    use std::sync::Arc;
    use store::{InMemorySessionStore, SessionStore};

    #[tokio::test]
    async fn load_active_artifacts_returns_promoted_versions_in_kind_order() {
        let store = Arc::new(InMemorySessionStore::new());
        append_prompt_artifact(
            &store,
            "artifact-b",
            "version-b",
            agent::types::ArtifactKind::Skill,
        )
        .await;
        append_prompt_artifact(
            &store,
            "artifact-a",
            "version-a",
            agent::types::ArtifactKind::Prompt,
        )
        .await;

        let outcome = load_active_artifacts(&(store as Arc<dyn SessionStore>))
            .await
            .unwrap();

        assert!(outcome.warnings.is_empty());
        assert_eq!(outcome.versions.len(), 2);
        assert_eq!(outcome.versions[0].artifact_id.as_str(), "artifact-a");
        assert_eq!(
            outcome.versions[0].version.kind,
            agent::types::ArtifactKind::Prompt
        );
        assert_eq!(outcome.versions[1].artifact_id.as_str(), "artifact-b");
        assert_eq!(
            outcome.versions[1].version.kind,
            agent::types::ArtifactKind::Skill
        );
    }

    #[tokio::test]
    async fn load_active_artifacts_warns_when_active_version_is_missing_payload() {
        let store = Arc::new(InMemorySessionStore::new());
        store
            .append_artifact(ArtifactLedgerEventEnvelope::new(
                ArtifactId::from("artifact-missing"),
                ArtifactLedgerEventKind::VersionPromoted {
                    version_id: ArtifactVersionId::from("version-missing"),
                    decision: ArtifactPromotionDecision {
                        kind: ArtifactPromotionDecisionKind::Promoted,
                        reason: "ship it".to_string(),
                        rollback_version_id: None,
                    },
                },
            ))
            .await
            .unwrap();

        let outcome = load_active_artifacts(&(store as Arc<dyn SessionStore>))
            .await
            .unwrap();

        assert!(outcome.versions.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("artifact-missing"));
        assert!(outcome.warnings[0].contains("version-missing"));
    }

    async fn append_prompt_artifact(
        store: &Arc<InMemorySessionStore>,
        artifact_id: &str,
        version_id: &str,
        kind: agent::types::ArtifactKind,
    ) {
        let version_id = ArtifactVersionId::from(version_id);
        store
            .append_artifact(ArtifactLedgerEventEnvelope::new(
                ArtifactId::from(artifact_id),
                ArtifactLedgerEventKind::VersionProposed {
                    version: ArtifactVersion {
                        version_id: version_id.clone(),
                        kind,
                        label: format!("{artifact_id}-label"),
                        description: Some("active".to_string()),
                        parent_version_id: None,
                        source_signal_ids: Vec::new(),
                        source_task_ids: Vec::new(),
                        source_case_ids: Vec::new(),
                        payload: json!({"instruction":"Follow the promoted guidance."}),
                        metadata: serde_json::Value::Null,
                    },
                },
            ))
            .await
            .unwrap();
        store
            .append_artifact(ArtifactLedgerEventEnvelope::new(
                ArtifactId::from(artifact_id),
                ArtifactLedgerEventKind::VersionPromoted {
                    version_id,
                    decision: ArtifactPromotionDecision {
                        kind: ArtifactPromotionDecisionKind::Promoted,
                        reason: "ship it".to_string(),
                        rollback_version_id: None,
                    },
                },
            ))
            .await
            .unwrap();
    }
}
