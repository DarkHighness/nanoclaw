use crate::{
    ArtifactArchive, NanoclawVerifierBundle, NanoclawVerifierConfig, SelfRegressionCorpus,
    VerificationFindingSeverity, VerificationReport, WorktreeCommandSpec, WorktreeMutation,
    WorktreeRunOutcome, WorktreeRunPlan, WorktreeRunner, WorktreeRunnerError,
};
use std::path::PathBuf;
use std::sync::Arc;
use store::SessionStore;
use thiserror::Error;
use types::{
    ArtifactId, ArtifactPromotionDecision, ArtifactPromotionDecisionKind, ArtifactVersion,
};

#[derive(Debug, Error)]
pub enum ProposalError {
    #[error(transparent)]
    Store(#[from] store::SessionStoreError),
    #[error(transparent)]
    Worktree(#[from] WorktreeRunnerError),
}

pub type ProposalResult<T> = std::result::Result<T, ProposalError>;

#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactProposalPlan {
    pub repo_root: PathBuf,
    pub artifact_id: ArtifactId,
    pub version: ArtifactVersion,
    pub baseline_ref: String,
    pub corpus: SelfRegressionCorpus,
    pub mutations: Vec<WorktreeMutation>,
    pub commands: Vec<WorktreeCommandSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromotionProposal {
    pub ready: bool,
    pub reason: String,
    pub blocking_finding_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactProposalOutcome {
    pub artifact_id: ArtifactId,
    pub version: ArtifactVersion,
    pub run: WorktreeRunOutcome,
    pub verification: VerificationReport,
    pub proposal: PromotionProposal,
}

#[derive(Clone)]
pub struct ArtifactProposalRunner<S: SessionStore + ?Sized> {
    archive: ArtifactArchive<S>,
    verifier: NanoclawVerifierBundle,
    worktree_runner: WorktreeRunner,
}

impl<S: SessionStore + ?Sized> ArtifactProposalRunner<S> {
    #[must_use]
    pub fn new(store: Arc<S>, verifier_config: NanoclawVerifierConfig) -> Self {
        Self {
            archive: ArtifactArchive::new(store),
            verifier: NanoclawVerifierBundle::new(verifier_config),
            worktree_runner: WorktreeRunner::new(),
        }
    }

    pub async fn run(&self, plan: ArtifactProposalPlan) -> ProposalResult<ArtifactProposalOutcome> {
        let artifact_id = plan.artifact_id.clone();
        let version = plan.version.clone();
        self.archive
            .propose_version(artifact_id.clone(), version.clone())
            .await?;

        let run = match self
            .worktree_runner
            .run(WorktreeRunPlan {
                repo_root: plan.repo_root,
                artifact_id: artifact_id.clone(),
                version: version.clone(),
                baseline_ref: plan.baseline_ref,
                mutations: plan.mutations,
                commands: plan.commands,
            })
            .await
        {
            Ok(run) => run,
            Err(error) => {
                self.archive
                    .record_decision(
                        &artifact_id,
                        version.version_id.clone(),
                        ArtifactPromotionDecision {
                            kind: ArtifactPromotionDecisionKind::Rejected,
                            reason: format!("worktree execution failed: {error}"),
                            rollback_version_id: version.parent_version_id.clone(),
                        },
                    )
                    .await?;
                return Err(error.into());
            }
        };

        let verification = self.verifier.verify(&version, &plan.corpus, &run);
        self.archive
            .record_evaluation(
                &artifact_id,
                version.version_id.clone(),
                verification.to_artifact_evaluation_summary(),
            )
            .await?;

        let blocking_finding_count = verification
            .findings
            .iter()
            .filter(|finding| finding.severity == VerificationFindingSeverity::Blocking)
            .count();
        let proposal = if verification.passed {
            // Passing verification only means the candidate is safe enough for
            // operator review. Promotion remains a separate decision so runtime
            // self-improvement never hot-swaps itself after one background run.
            PromotionProposal {
                ready: true,
                reason: "candidate cleared the current verifier bundle".to_string(),
                blocking_finding_count,
            }
        } else {
            let reason = format!(
                "verifier rejected candidate with {blocking_finding_count} blocking findings"
            );
            self.archive
                .record_decision(
                    &artifact_id,
                    version.version_id.clone(),
                    ArtifactPromotionDecision {
                        kind: ArtifactPromotionDecisionKind::Rejected,
                        reason: reason.clone(),
                        rollback_version_id: version.parent_version_id.clone(),
                    },
                )
                .await?;
            PromotionProposal {
                ready: false,
                reason,
                blocking_finding_count,
            }
        };

        Ok(ArtifactProposalOutcome {
            artifact_id,
            version,
            run,
            verification,
            proposal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactProposalPlan, ArtifactProposalRunner};
    use crate::{
        NanoclawVerifierConfig, SelfImproveTask, SelfImproveTaskKind, SelfImproveTaskPriority,
        SelfRegressionCase, SelfRegressionCorpus, SelfRegressionSplit, WorktreeCommandSpec,
        WorktreeMutation,
    };
    use nanoclaw_test_support::run_current_thread_test;
    use std::path::Path;
    use std::process::Command as StdCommand;
    use std::sync::Arc;
    use store::InMemorySessionStore;
    use tempfile::tempdir;
    use types::{
        AgentSessionId, ArtifactId, ArtifactKind, ArtifactPromotionDecisionKind, ArtifactVersion,
        ArtifactVersionId, SessionId,
    };

    fn init_git_repo(path: &Path) {
        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "ci@example.com"]);
        run_git(path, &["config", "user.name", "CI"]);
    }

    fn seed_repo_file(path: &Path, relative_path: &str, content: &str) {
        let target = path.join(relative_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(target, content).unwrap();
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", "initial"]);
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = StdCommand::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn corpus(relevant_file: &str, split: SelfRegressionSplit) -> SelfRegressionCorpus {
        let case = SelfRegressionCase {
            case_id: "case-1".to_string(),
            split,
            task: SelfImproveTask {
                task_id: "task-1".to_string(),
                kind: SelfImproveTaskKind::RuntimeBugfix,
                priority: SelfImproveTaskPriority::High,
                summary: "repair runtime".to_string(),
                objective: "stop the failure".to_string(),
                expected_outcome: "candidate remains stable".to_string(),
                session_id: SessionId::from("session-1"),
                agent_session_id: AgentSessionId::from("agent-1"),
                turn_id: Some("turn-1".into()),
                source_signal_ids: vec!["signal-1".into()],
                source_event_ids: vec![],
                source_signal_kinds: vec![],
                relevant_files: vec![relevant_file.to_string()],
                tool_name: None,
                source_task_id: None,
                details: vec![],
            },
            focus_events: vec![],
            focus_transcript: vec![],
            agent_session_transcript: vec![],
            last_user_prompt: Some("repair the bug".to_string()),
        };

        SelfRegressionCorpus {
            train: vec![],
            validation: vec![case],
            holdout: vec![],
        }
    }

    fn version(kind: ArtifactKind) -> ArtifactVersion {
        ArtifactVersion {
            version_id: ArtifactVersionId::from("artifact-v2"),
            kind,
            label: "artifact-v2".to_string(),
            description: Some("candidate".to_string()),
            parent_version_id: Some(ArtifactVersionId::from("artifact-v1")),
            source_signal_ids: vec!["signal-1".into()],
            source_task_ids: vec!["task-1".to_string()],
            source_case_ids: vec!["case-1".to_string()],
            payload: serde_json::json!({"candidate":"v2"}),
            metadata: serde_json::json!({"owner":"nanoclaw"}),
        }
    }

    #[test]
    fn proposal_runner_keeps_passed_candidate_pending_promotion() {
        run_current_thread_test(async {
            let dir = tempdir().unwrap();
            init_git_repo(dir.path());
            seed_repo_file(dir.path(), "prompt.txt", "baseline\n");

            let store = Arc::new(InMemorySessionStore::new());
            let runner =
                ArtifactProposalRunner::new(store.clone(), NanoclawVerifierConfig::default());
            let outcome = runner
                .run(ArtifactProposalPlan {
                    repo_root: dir.path().to_path_buf(),
                    artifact_id: ArtifactId::from("artifact-prompt"),
                    version: version(ArtifactKind::Prompt),
                    baseline_ref: "HEAD".to_string(),
                    corpus: corpus("prompt.txt", SelfRegressionSplit::Validation),
                    mutations: vec![WorktreeMutation::WriteFile {
                        relative_path: "prompt.txt".into(),
                        content: "candidate\n".to_string(),
                    }],
                    commands: vec![WorktreeCommandSpec {
                        argv: vec![
                            "sh".to_string(),
                            "-lc".to_string(),
                            "test \"$(cat prompt.txt)\" = candidate".to_string(),
                        ],
                        env: Default::default(),
                    }],
                })
                .await
                .unwrap();

            assert!(outcome.verification.passed);
            assert!(outcome.proposal.ready);

            let summary = crate::ArtifactArchive::new(store.clone())
                .summary(&ArtifactId::from("artifact-prompt"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(summary.version_count, 1);
            assert_eq!(summary.last_decision, None);
        });
    }

    #[test]
    fn proposal_runner_rejects_blocked_candidate_in_ledger() {
        run_current_thread_test(async {
            let dir = tempdir().unwrap();
            init_git_repo(dir.path());
            seed_repo_file(dir.path(), "crates/runtime/src/approval.rs", "baseline\n");

            let store = Arc::new(InMemorySessionStore::new());
            let runner =
                ArtifactProposalRunner::new(store.clone(), NanoclawVerifierConfig::default());
            let outcome = runner
                .run(ArtifactProposalPlan {
                    repo_root: dir.path().to_path_buf(),
                    artifact_id: ArtifactId::from("artifact-runtime"),
                    version: version(ArtifactKind::RuntimePatch),
                    baseline_ref: "HEAD".to_string(),
                    corpus: corpus(
                        "crates/runtime/src/runtime.rs",
                        SelfRegressionSplit::Validation,
                    ),
                    mutations: vec![WorktreeMutation::WriteFile {
                        relative_path: "crates/runtime/src/approval.rs".into(),
                        content: "candidate\n".to_string(),
                    }],
                    commands: vec![WorktreeCommandSpec {
                        argv: vec![
                            "sh".to_string(),
                            "-lc".to_string(),
                            "test \"$(cat crates/runtime/src/approval.rs)\" = candidate"
                                .to_string(),
                        ],
                        env: Default::default(),
                    }],
                })
                .await
                .unwrap();

            assert!(!outcome.verification.passed);
            assert!(!outcome.proposal.ready);

            let summary = crate::ArtifactArchive::new(store.clone())
                .summary(&ArtifactId::from("artifact-runtime"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                summary.last_decision,
                Some(ArtifactPromotionDecisionKind::Rejected)
            );
        });
    }
}
