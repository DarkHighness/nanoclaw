//! Nanoclaw-specific verification bundle.
//!
//! These verifiers live in `meta` instead of the older generic `evals`
//! pipeline because they reason over artifact lineage, self-regression corpus
//! source refs, and isolated worktree traces rather than experiment
//! candidate/baseline scorecards.

use crate::{SelfRegressionCase, SelfRegressionCorpus, SelfRegressionSplit, WorktreeRunOutcome};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use types::{ArtifactEvaluationSummary, ArtifactVersion};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFindingSeverity {
    Warning,
    Blocking,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationFinding {
    pub code: String,
    pub severity: VerificationFindingSeverity,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationReport {
    pub passed: bool,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<VerificationFinding>,
    #[serde(default)]
    pub changed_paths: Vec<PathBuf>,
    pub validation_case_count: usize,
    pub holdout_case_count: usize,
    #[serde(default)]
    pub relevant_files: Vec<String>,
}

impl VerificationReport {
    #[must_use]
    pub fn to_artifact_evaluation_summary(&self) -> ArtifactEvaluationSummary {
        ArtifactEvaluationSummary {
            summary: self.summary.clone(),
            verifier_summary: serde_json::to_value(self)
                .unwrap_or_else(|_| json!({"summary": self.summary})),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NanoclawVerifierConfig {
    #[serde(default = "default_protected_paths")]
    pub protected_paths: Vec<PathBuf>,
    #[serde(default = "default_true")]
    pub require_changed_paths: bool,
    #[serde(default = "default_true")]
    pub require_source_cases: bool,
    #[serde(default = "default_true")]
    pub warn_on_unexpected_scope: bool,
}

impl Default for NanoclawVerifierConfig {
    fn default() -> Self {
        Self {
            protected_paths: default_protected_paths(),
            require_changed_paths: true,
            require_source_cases: true,
            warn_on_unexpected_scope: true,
        }
    }
}

#[derive(Clone, Default)]
pub struct NanoclawVerifierBundle {
    config: NanoclawVerifierConfig,
}

impl NanoclawVerifierBundle {
    #[must_use]
    pub fn new(config: NanoclawVerifierConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn verify(
        &self,
        version: &ArtifactVersion,
        corpus: &SelfRegressionCorpus,
        outcome: &WorktreeRunOutcome,
    ) -> VerificationReport {
        let case_index = case_index(corpus);
        let mut findings = Vec::new();
        let source_cases = resolve_source_cases(
            version,
            &case_index,
            self.config.require_source_cases,
            &mut findings,
        );
        let relevant_files = relevant_files(&source_cases);
        let changed_paths = outcome.trace.changed_paths.clone();

        if !outcome.succeeded {
            findings.push(VerificationFinding {
                code: "command_failed".to_string(),
                severity: VerificationFindingSeverity::Blocking,
                summary: "candidate commands failed inside isolated worktree".to_string(),
                details: json!({
                    "failed_commands": outcome.trace.commands.iter()
                        .filter(|command| command.exit_code.unwrap_or_default() != 0)
                        .map(|command| {
                            json!({
                                "argv": command.argv,
                                "exit_code": command.exit_code,
                                "stderr": command.stderr,
                            })
                        })
                        .collect::<Vec<_>>(),
                }),
            });
        }

        if self.config.require_changed_paths && changed_paths.is_empty() {
            findings.push(VerificationFinding {
                code: "empty_diff".to_string(),
                severity: VerificationFindingSeverity::Blocking,
                summary: "candidate produced no observable repository diff".to_string(),
                details: Value::Null,
            });
        }

        let protected_hits = protected_path_hits(&changed_paths, &self.config.protected_paths);
        if !protected_hits.is_empty() {
            findings.push(VerificationFinding {
                code: "protected_path_changed".to_string(),
                severity: VerificationFindingSeverity::Blocking,
                summary: "candidate touched protected approval or sandbox boundaries".to_string(),
                details: json!({
                    "changed_paths": protected_hits,
                    "protected_paths": self.config.protected_paths,
                }),
            });
        }

        if self.config.warn_on_unexpected_scope && !relevant_files.is_empty() {
            let unexpected_paths = unexpected_scope_paths(&changed_paths, &relevant_files);
            if !unexpected_paths.is_empty() {
                findings.push(VerificationFinding {
                    code: "unexpected_file_scope".to_string(),
                    severity: VerificationFindingSeverity::Warning,
                    summary: "candidate touched files outside the sourced regression cases"
                        .to_string(),
                    details: json!({
                        "changed_paths": unexpected_paths,
                        "relevant_files": relevant_files,
                    }),
                });
            }
        }

        let validation_case_count = source_cases
            .iter()
            .filter(|case| case.split == SelfRegressionSplit::Validation)
            .count();
        let holdout_case_count = source_cases
            .iter()
            .filter(|case| case.split == SelfRegressionSplit::Holdout)
            .count();
        let blocking_count = findings
            .iter()
            .filter(|finding| finding.severity == VerificationFindingSeverity::Blocking)
            .count();
        let warning_count = findings
            .iter()
            .filter(|finding| finding.severity == VerificationFindingSeverity::Warning)
            .count();
        let passed = blocking_count == 0;

        VerificationReport {
            passed,
            summary: if passed {
                format!(
                    "verification passed with {warning_count} warnings across {} changed paths",
                    changed_paths.len()
                )
            } else {
                format!(
                    "verification blocked with {blocking_count} blocking findings and {warning_count} warnings"
                )
            },
            findings,
            changed_paths,
            validation_case_count,
            holdout_case_count,
            relevant_files,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_protected_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from(".githooks"),
        PathBuf::from("crates/env"),
        PathBuf::from("crates/runtime/src/approval.rs"),
        PathBuf::from("crates/runtime/src/permissions.rs"),
        PathBuf::from("crates/sandbox"),
        PathBuf::from("apps/code-agent/src/backend/approval.rs"),
        PathBuf::from("apps/code-agent/src/backend/boot_sandbox.rs"),
    ]
}

fn case_index<'a>(corpus: &'a SelfRegressionCorpus) -> BTreeMap<&'a str, &'a SelfRegressionCase> {
    corpus
        .train
        .iter()
        .chain(corpus.validation.iter())
        .chain(corpus.holdout.iter())
        .map(|case| (case.case_id.as_str(), case))
        .collect()
}

fn resolve_source_cases<'a>(
    version: &ArtifactVersion,
    case_index: &BTreeMap<&'a str, &'a SelfRegressionCase>,
    require_source_cases: bool,
    findings: &mut Vec<VerificationFinding>,
) -> Vec<&'a SelfRegressionCase> {
    let mut cases = Vec::new();

    if version.source_case_ids.is_empty() && require_source_cases {
        findings.push(VerificationFinding {
            code: "missing_source_cases".to_string(),
            severity: VerificationFindingSeverity::Blocking,
            summary: "artifact version does not cite any self-regression source cases".to_string(),
            details: Value::Null,
        });
        return cases;
    }

    for case_id in &version.source_case_ids {
        match case_index.get(case_id.as_str()) {
            Some(case) => cases.push(*case),
            None => findings.push(VerificationFinding {
                code: "unknown_source_case".to_string(),
                severity: VerificationFindingSeverity::Blocking,
                summary: format!("artifact version cites unknown self-regression case `{case_id}`"),
                details: json!({ "case_id": case_id }),
            }),
        }
    }

    cases
}

fn relevant_files(cases: &[&SelfRegressionCase]) -> Vec<String> {
    let mut files = BTreeSet::new();
    for case in cases {
        for path in &case.task.relevant_files {
            files.insert(path.clone());
        }
    }
    files.into_iter().collect()
}

fn protected_path_hits(changed_paths: &[PathBuf], protected_paths: &[PathBuf]) -> Vec<String> {
    changed_paths
        .iter()
        .filter(|changed| {
            protected_paths
                .iter()
                .any(|protected| path_matches_scope(changed, protected))
        })
        .map(|path| path.display().to_string())
        .collect()
}

fn unexpected_scope_paths(changed_paths: &[PathBuf], relevant_files: &[String]) -> Vec<String> {
    changed_paths
        .iter()
        .filter(|changed| {
            !relevant_files
                .iter()
                .any(|hint| path_matches_scope(changed, Path::new(hint)))
        })
        .map(|path| path.display().to_string())
        .collect()
}

fn path_matches_scope(changed: &Path, scope: &Path) -> bool {
    changed == scope || changed.starts_with(scope)
}

#[cfg(test)]
mod tests {
    use super::{NanoclawVerifierBundle, VerificationFindingSeverity, default_protected_paths};
    use crate::{
        SelfImproveTask, SelfImproveTaskKind, SelfImproveTaskPriority, SelfRegressionCase,
        SelfRegressionCorpus, SelfRegressionSplit, WorktreeCommandStatus, WorktreeCommandTrace,
        WorktreeRunOutcome, WorktreeRunTrace,
    };
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use types::{ArtifactId, ArtifactKind, ArtifactVersion, ArtifactVersionId, SessionId};

    fn version(source_case_ids: Vec<&str>) -> ArtifactVersion {
        ArtifactVersion {
            version_id: ArtifactVersionId::from("artifact-runtime-v2"),
            kind: ArtifactKind::RuntimePatch,
            label: "runtime-v2".to_string(),
            description: Some("tighten runtime error handling".to_string()),
            parent_version_id: Some(ArtifactVersionId::from("artifact-runtime-v1")),
            source_signal_ids: vec!["signal-1".into()],
            source_task_ids: vec!["task-1".to_string()],
            source_case_ids: source_case_ids.into_iter().map(str::to_string).collect(),
            payload: json!({"patch":"candidate"}),
            metadata: json!({"owner":"nanoclaw"}),
        }
    }

    fn corpus() -> SelfRegressionCorpus {
        let case = SelfRegressionCase {
            case_id: "case-1".to_string(),
            split: SelfRegressionSplit::Validation,
            task: SelfImproveTask {
                task_id: "task-1".to_string(),
                kind: SelfImproveTaskKind::RuntimeBugfix,
                priority: SelfImproveTaskPriority::High,
                summary: "fix runtime bug".to_string(),
                objective: "stabilize failure path".to_string(),
                expected_outcome: "runtime survives the turn".to_string(),
                session_id: SessionId::from("session-1"),
                agent_session_id: "agent-1".into(),
                turn_id: Some("turn-1".into()),
                source_signal_ids: vec!["signal-1".into()],
                source_event_ids: vec![],
                source_signal_kinds: vec![],
                relevant_files: vec!["crates/runtime/src/runtime.rs".to_string()],
                tool_name: None,
                source_task_id: None,
                details: vec![],
            },
            focus_events: vec![],
            focus_transcript: vec![],
            agent_session_transcript: vec![],
            last_user_prompt: Some("fix the runtime".to_string()),
        };

        SelfRegressionCorpus {
            train: vec![],
            validation: vec![case],
            holdout: vec![],
        }
    }

    fn outcome(changed_paths: Vec<&str>, succeeded: bool) -> WorktreeRunOutcome {
        WorktreeRunOutcome {
            succeeded,
            trace: WorktreeRunTrace {
                artifact_id: ArtifactId::from("artifact-runtime"),
                version_id: ArtifactVersionId::from("artifact-runtime-v2"),
                artifact_kind: ArtifactKind::RuntimePatch,
                baseline_ref: "HEAD".to_string(),
                worktree_path: PathBuf::from("/tmp/worktree"),
                changed_paths: changed_paths.into_iter().map(PathBuf::from).collect(),
                mutations: vec![],
                commands: vec![WorktreeCommandTrace {
                    argv: vec!["cargo".to_string(), "test".to_string()],
                    status: if succeeded {
                        WorktreeCommandStatus::Succeeded
                    } else {
                        WorktreeCommandStatus::Failed
                    },
                    exit_code: if succeeded { Some(0) } else { Some(1) },
                    stdout: String::new(),
                    stderr: if succeeded {
                        String::new()
                    } else {
                        "test failure".to_string()
                    },
                }],
                git_diff: "diff --git a/...".to_string(),
                cleanup_performed: true,
            },
        }
    }

    #[test]
    fn verifier_blocks_failed_commands_and_protected_paths() {
        let report = NanoclawVerifierBundle::default().verify(
            &version(vec!["case-1"]),
            &corpus(),
            &outcome(
                vec![
                    "crates/runtime/src/approval.rs",
                    "crates/runtime/src/runtime.rs",
                ],
                false,
            ),
        );

        assert!(!report.passed);
        assert!(report.findings.iter().any(|finding| {
            finding.code == "command_failed"
                && finding.severity == VerificationFindingSeverity::Blocking
        }));
        assert!(report.findings.iter().any(|finding| {
            finding.code == "protected_path_changed"
                && finding.severity == VerificationFindingSeverity::Blocking
        }));
    }

    #[test]
    fn verifier_warns_on_unexpected_scope_and_serializes_summary() {
        let report = NanoclawVerifierBundle::default().verify(
            &version(vec!["case-1"]),
            &corpus(),
            &outcome(
                vec!["crates/runtime/src/runtime.rs", "crates/meta/src/miner.rs"],
                true,
            ),
        );

        assert!(report.passed);
        assert_eq!(report.validation_case_count, 1);
        assert!(report.findings.iter().any(|finding| {
            finding.code == "unexpected_file_scope"
                && finding.severity == VerificationFindingSeverity::Warning
        }));
        let summary = report.to_artifact_evaluation_summary();
        assert!(summary.summary.contains("verification passed"));
        assert_ne!(summary.verifier_summary, Value::Null);
    }

    #[test]
    fn verifier_blocks_unknown_source_cases() {
        let report = NanoclawVerifierBundle::default().verify(
            &version(vec!["case-missing"]),
            &corpus(),
            &outcome(vec!["crates/runtime/src/runtime.rs"], true),
        );

        assert!(!report.passed);
        assert!(report.findings.iter().any(|finding| {
            finding.code == "unknown_source_case"
                && finding.severity == VerificationFindingSeverity::Blocking
        }));
        assert!(
            default_protected_paths()
                .iter()
                .any(|path| path == &PathBuf::from("crates/runtime/src/approval.rs"))
        );
    }
}
