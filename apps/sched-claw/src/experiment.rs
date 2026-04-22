use crate::app_config::app_state_dir;
use crate::metrics::{Guardrail, MetricMap, MetricTarget, median_metric};
use crate::workload::{HostFingerprint, WorkloadContract};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const EXPERIMENT_VERSION: u32 = 1;
const MANIFEST_FILE_NAME: &str = "experiment.toml";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentManifest {
    pub version: u32,
    pub experiment_id: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub host: HostFingerprint,
    pub workload: WorkloadContract,
    pub primary_metric: MetricTarget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guardrails: Vec<Guardrail>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline_runs: Vec<RecordedRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<CandidateRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deployments: Vec<DeploymentRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub spec: CandidateSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<RecordedRun>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateSpec {
    pub candidate_id: String,
    pub template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daemon_argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub daemon_env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub knobs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedRun {
    pub label: String,
    pub recorded_at_unix_ms: u64,
    pub scheduler: SchedulerKind,
    pub artifact_dir: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: MetricMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeploymentRecord {
    pub candidate_id: String,
    pub requested_at_unix_ms: u64,
    pub label: String,
    pub daemon_pid: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default)]
    pub replace_existing: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerKind {
    Cfs,
    SchedExt,
}

impl SchedulerKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cfs => "cfs",
            Self::SchedExt => "sched_ext",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExperimentInitSpec {
    pub experiment_id: String,
    pub workload: WorkloadContract,
    pub primary_metric: MetricTarget,
    pub guardrails: Vec<Guardrail>,
}

#[derive(Clone, Debug)]
pub struct ExperimentSummary {
    pub experiment_id: String,
    pub manifest_path: PathBuf,
    pub updated_at_unix_ms: u64,
    pub workload_name: String,
    pub primary_metric_name: String,
    pub baseline_run_count: usize,
    pub candidate_count: usize,
    pub deployment_count: usize,
}

#[derive(Clone, Debug)]
pub struct LoadedExperiment {
    pub manifest_path: PathBuf,
    pub manifest: ExperimentManifest,
}

#[derive(Clone, Debug)]
pub struct ExperimentArtifact {
    pub action: &'static str,
    pub experiment_id: String,
    pub manifest_path: PathBuf,
    pub details: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct ExperimentScoreReport {
    pub experiment_id: String,
    pub manifest_path: PathBuf,
    pub primary_metric: MetricTarget,
    pub baseline_run_count: usize,
    pub baseline_primary_value: Option<f64>,
    pub entries: Vec<CandidateScore>,
}

#[derive(Clone, Debug)]
pub struct CandidateScore {
    pub candidate_id: String,
    pub template: String,
    pub run_count: usize,
    pub primary_candidate_value: Option<f64>,
    pub primary_improvement_pct: Option<f64>,
    pub decision: CandidateDecision,
    pub breached_guardrails: Vec<GuardrailScore>,
}

#[derive(Clone, Debug)]
pub struct GuardrailScore {
    pub name: String,
    pub baseline_value: Option<f64>,
    pub candidate_value: Option<f64>,
    pub improvement_pct: Option<f64>,
    pub max_regression_pct: f64,
    pub status: GuardrailStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardrailStatus {
    Pass,
    Breach,
    Missing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateDecision {
    Promote,
    Revise,
    Blocked,
    Incomplete,
}

impl CandidateDecision {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Promote => "promote",
            Self::Revise => "revise",
            Self::Blocked => "blocked",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Clone)]
pub struct ExperimentCatalog {
    workspace_root: PathBuf,
    experiments_root: PathBuf,
}

impl ExperimentCatalog {
    pub fn open(workspace_root: &Path) -> Result<Self> {
        let experiments_root = experiments_dir(workspace_root);
        std::fs::create_dir_all(&experiments_root).with_context(|| {
            format!(
                "failed to create sched-claw experiments root {}",
                experiments_root.display()
            )
        })?;
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            experiments_root,
        })
    }

    pub fn list(&self) -> Result<Vec<ExperimentSummary>> {
        let mut summaries = Vec::new();
        for entry in std::fs::read_dir(&self.experiments_root).with_context(|| {
            format!(
                "failed to read experiments root {}",
                self.experiments_root.display()
            )
        })? {
            let entry = entry?;
            let path = entry.path();
            let manifest_path = if path.is_dir() {
                path.join(MANIFEST_FILE_NAME)
            } else {
                path
            };
            if !manifest_path.is_file() {
                continue;
            }
            let manifest = read_manifest(&manifest_path)?;
            summaries.push(ExperimentSummary {
                experiment_id: manifest.experiment_id.clone(),
                manifest_path,
                updated_at_unix_ms: manifest.updated_at_unix_ms,
                workload_name: manifest.workload.name.clone(),
                primary_metric_name: manifest.primary_metric.name.clone(),
                baseline_run_count: manifest.baseline_runs.len(),
                candidate_count: manifest.candidates.len(),
                deployment_count: manifest.deployments.len(),
            });
        }
        summaries.sort_by(|left, right| {
            right
                .updated_at_unix_ms
                .cmp(&left.updated_at_unix_ms)
                .then_with(|| left.experiment_id.cmp(&right.experiment_id))
        });
        Ok(summaries)
    }

    pub fn init(&self, spec: ExperimentInitSpec) -> Result<ExperimentArtifact> {
        validate_identifier("experiment id", &spec.experiment_id)?;
        let manifest_path = self.manifest_path_for_id(&spec.experiment_id);
        if manifest_path.exists() {
            bail!(
                "experiment {} already exists at {}",
                spec.experiment_id,
                manifest_path.display()
            );
        }
        let now = now_unix_ms();
        let manifest = ExperimentManifest {
            version: EXPERIMENT_VERSION,
            experiment_id: spec.experiment_id.clone(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            host: HostFingerprint::capture(),
            workload: spec.workload,
            primary_metric: spec.primary_metric,
            guardrails: spec.guardrails,
            baseline_runs: Vec::new(),
            candidates: Vec::new(),
            deployments: Vec::new(),
        };
        write_manifest(&manifest_path, &manifest)?;
        std::fs::create_dir_all(self.artifacts_dir_for_id(&spec.experiment_id)).with_context(
            || {
                format!(
                    "failed to create artifacts directory for experiment {}",
                    spec.experiment_id
                )
            },
        )?;
        Ok(ExperimentArtifact {
            action: "initialized",
            experiment_id: spec.experiment_id,
            manifest_path,
            details: Vec::new(),
        })
    }

    pub fn load(&self, reference: &str) -> Result<LoadedExperiment> {
        let manifest_path = self.resolve_reference(reference)?;
        Ok(LoadedExperiment {
            manifest: read_manifest(&manifest_path)?,
            manifest_path,
        })
    }

    pub fn add_candidate(
        &self,
        reference: &str,
        spec: CandidateSpec,
    ) -> Result<ExperimentArtifact> {
        validate_identifier("candidate id", &spec.candidate_id)?;
        let mut loaded = self.load(reference)?;
        if loaded
            .manifest
            .candidates
            .iter()
            .any(|candidate| candidate.spec.candidate_id == spec.candidate_id)
        {
            bail!(
                "candidate {} already exists in experiment {}",
                spec.candidate_id,
                loaded.manifest.experiment_id
            );
        }
        loaded.manifest.candidates.push(CandidateRecord {
            spec,
            runs: Vec::new(),
        });
        let candidate_id = candidate_id(&loaded.manifest).to_string();
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "added candidate",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: vec![("candidate".to_string(), candidate_id)],
        })
    }

    pub fn set_candidate(
        &self,
        reference: &str,
        spec: CandidateSpec,
    ) -> Result<ExperimentArtifact> {
        validate_identifier("candidate id", &spec.candidate_id)?;
        let mut loaded = self.load(reference)?;
        let action = if let Some(existing) = loaded
            .manifest
            .candidates
            .iter_mut()
            .find(|candidate| candidate.spec.candidate_id == spec.candidate_id)
        {
            existing.spec = spec.clone();
            "updated candidate"
        } else {
            loaded.manifest.candidates.push(CandidateRecord {
                spec: spec.clone(),
                runs: Vec::new(),
            });
            "added candidate"
        };
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action,
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: vec![
                ("candidate".to_string(), spec.candidate_id),
                ("template".to_string(), spec.template),
            ],
        })
    }

    pub fn record_baseline(&self, reference: &str, run: RecordedRun) -> Result<ExperimentArtifact> {
        let mut loaded = self.load(reference)?;
        loaded.manifest.baseline_runs.push(run);
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded baseline",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: Vec::new(),
        })
    }

    pub fn record_candidate(
        &self,
        reference: &str,
        candidate_id: &str,
        run: RecordedRun,
    ) -> Result<ExperimentArtifact> {
        let mut loaded = self.load(reference)?;
        let candidate = loaded
            .manifest
            .candidates
            .iter_mut()
            .find(|candidate| candidate.spec.candidate_id == candidate_id)
            .ok_or_else(|| {
                anyhow!(
                    "unknown candidate {} in experiment {}",
                    candidate_id,
                    loaded.manifest.experiment_id
                )
            })?;
        candidate.runs.push(run);
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded candidate",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: vec![("candidate".to_string(), candidate_id.to_string())],
        })
    }

    pub fn record_deployment(
        &self,
        reference: &str,
        record: DeploymentRecord,
    ) -> Result<ExperimentArtifact> {
        let mut loaded = self.load(reference)?;
        loaded.manifest.deployments.push(record.clone());
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded deployment",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: vec![
                ("candidate".to_string(), record.candidate_id),
                ("label".to_string(), record.label),
                ("daemon_pid".to_string(), record.daemon_pid.to_string()),
            ],
        })
    }

    pub fn score(&self, reference: &str) -> Result<ExperimentScoreReport> {
        let loaded = self.load(reference)?;
        let baseline_primary_value = median_metric(
            &loaded.manifest.primary_metric.name,
            loaded.manifest.baseline_runs.iter().map(|run| &run.metrics),
        );
        let entries = loaded
            .manifest
            .candidates
            .iter()
            .map(|candidate| {
                score_candidate(
                    &loaded.manifest.primary_metric,
                    &loaded.manifest.guardrails,
                    &loaded.manifest.baseline_runs,
                    candidate,
                )
            })
            .collect::<Vec<_>>();
        Ok(ExperimentScoreReport {
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            primary_metric: loaded.manifest.primary_metric,
            baseline_run_count: loaded.manifest.baseline_runs.len(),
            baseline_primary_value,
            entries,
        })
    }

    pub fn manifest_path_for_id(&self, experiment_id: &str) -> PathBuf {
        self.experiments_root
            .join(experiment_id)
            .join(MANIFEST_FILE_NAME)
    }

    pub fn artifacts_dir_for_id(&self, experiment_id: &str) -> PathBuf {
        self.experiments_root.join(experiment_id).join("artifacts")
    }

    fn resolve_reference(&self, reference: &str) -> Result<PathBuf> {
        let reference = reference.trim();
        if reference.is_empty() {
            bail!("experiment reference cannot be empty");
        }
        if reference == "last" {
            return self
                .list()?
                .into_iter()
                .next()
                .map(|summary| summary.manifest_path)
                .ok_or_else(|| anyhow!("no persisted experiments found"));
        }

        let candidate = PathBuf::from(reference);
        let direct = if candidate.is_absolute() {
            candidate
        } else {
            self.workspace_root.join(&candidate)
        };
        if direct.is_file() {
            return Ok(direct);
        }
        if direct.is_dir() {
            let manifest_path = direct.join(MANIFEST_FILE_NAME);
            if manifest_path.is_file() {
                return Ok(manifest_path);
            }
        }

        let manifest_path = self.manifest_path_for_id(reference);
        if manifest_path.is_file() {
            return Ok(manifest_path);
        }
        bail!("unknown experiment id or manifest path: {reference}");
    }
}

fn candidate_id(manifest: &ExperimentManifest) -> &str {
    manifest
        .candidates
        .last()
        .map(|candidate| candidate.spec.candidate_id.as_str())
        .unwrap_or("<unknown>")
}

pub fn experiments_dir(workspace_root: &Path) -> PathBuf {
    app_state_dir(workspace_root).join("experiments")
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn validate_identifier(kind: &str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{kind} cannot be empty");
    }
    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        Ok(())
    } else {
        bail!("{kind} must use only ASCII alphanumeric characters, '-', '_' or '.'")
    }
}

fn touch_manifest(manifest: &mut ExperimentManifest) {
    manifest.updated_at_unix_ms = now_unix_ms();
}

fn read_manifest(path: &Path) -> Result<ExperimentManifest> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read experiment manifest {}", path.display()))?;
    let manifest = toml::from_str::<ExperimentManifest>(&raw)
        .with_context(|| format!("failed to parse experiment manifest {}", path.display()))?;
    if manifest.version != EXPERIMENT_VERSION {
        bail!(
            "unsupported experiment manifest version {} in {}",
            manifest.version,
            path.display()
        );
    }
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &ExperimentManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered =
        toml::to_string_pretty(manifest).context("failed to render experiment manifest as TOML")?;
    std::fs::write(path, rendered)
        .with_context(|| format!("failed to write experiment manifest {}", path.display()))?;
    Ok(())
}

fn score_candidate(
    primary_metric: &MetricTarget,
    guardrails: &[Guardrail],
    baseline_runs: &[RecordedRun],
    candidate: &CandidateRecord,
) -> CandidateScore {
    let baseline_primary_value = median_metric(
        &primary_metric.name,
        baseline_runs.iter().map(|run| &run.metrics),
    );
    let primary_candidate_value = median_metric(
        &primary_metric.name,
        candidate.runs.iter().map(|run| &run.metrics),
    );
    let primary_improvement_pct = baseline_primary_value.and_then(|baseline| {
        primary_candidate_value
            .and_then(|value| primary_metric.goal.improvement_pct(baseline, value))
    });
    let breached_guardrails = guardrails
        .iter()
        .map(|guardrail| score_guardrail(guardrail, baseline_runs, &candidate.runs))
        .collect::<Vec<_>>();
    let decision = if baseline_primary_value.is_none() || primary_candidate_value.is_none() {
        CandidateDecision::Incomplete
    } else if breached_guardrails
        .iter()
        .any(|score| matches!(score.status, GuardrailStatus::Breach))
    {
        CandidateDecision::Blocked
    } else if primary_improvement_pct.is_some_and(|value| value > 0.0) {
        CandidateDecision::Promote
    } else {
        CandidateDecision::Revise
    };
    CandidateScore {
        candidate_id: candidate.spec.candidate_id.clone(),
        template: candidate.spec.template.clone(),
        run_count: candidate.runs.len(),
        primary_candidate_value,
        primary_improvement_pct,
        decision,
        breached_guardrails,
    }
}

fn score_guardrail(
    guardrail: &Guardrail,
    baseline_runs: &[RecordedRun],
    candidate_runs: &[RecordedRun],
) -> GuardrailScore {
    let baseline_value = median_metric(
        &guardrail.name,
        baseline_runs.iter().map(|run| &run.metrics),
    );
    let candidate_value = median_metric(
        &guardrail.name,
        candidate_runs.iter().map(|run| &run.metrics),
    );
    let improvement_pct = baseline_value.and_then(|baseline| {
        candidate_value.and_then(|value| guardrail.goal.improvement_pct(baseline, value))
    });
    let status = match improvement_pct {
        Some(value) if value < -guardrail.max_regression_pct => GuardrailStatus::Breach,
        Some(_) => GuardrailStatus::Pass,
        None => GuardrailStatus::Missing,
    };
    GuardrailScore {
        name: guardrail.name.clone(),
        baseline_value,
        candidate_value,
        improvement_pct,
        max_regression_pct: guardrail.max_regression_pct,
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateDecision, CandidateRecord, CandidateSpec, DeploymentRecord, ExperimentCatalog,
        ExperimentInitSpec, RecordedRun, SchedulerKind, experiments_dir,
    };
    use crate::metrics::{Guardrail, MetricGoal, MetricMap, MetricTarget};
    use crate::workload::WorkloadContract;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn init_and_resolve_last_experiment() {
        let dir = tempdir().unwrap();
        let catalog = ExperimentCatalog::open(dir.path()).unwrap();
        let artifact = catalog
            .init(ExperimentInitSpec {
                experiment_id: "demo".to_string(),
                workload: WorkloadContract {
                    name: "bench".to_string(),
                    ..Default::default()
                },
                primary_metric: MetricTarget {
                    name: "latency_ms".to_string(),
                    goal: MetricGoal::Minimize,
                    unit: Some("ms".to_string()),
                    notes: None,
                },
                guardrails: Vec::new(),
            })
            .unwrap();
        assert!(artifact.manifest_path.is_file());

        let loaded = catalog.load("last").unwrap();
        assert_eq!(loaded.manifest.experiment_id, "demo");
        assert_eq!(
            experiments_dir(dir.path()),
            dir.path().join(".nanoclaw/apps/sched-claw/experiments")
        );
    }

    #[test]
    fn score_blocks_candidates_that_breach_guardrails() {
        let dir = tempdir().unwrap();
        let catalog = ExperimentCatalog::open(dir.path()).unwrap();
        catalog
            .init(ExperimentInitSpec {
                experiment_id: "score-demo".to_string(),
                workload: WorkloadContract {
                    name: "bench".to_string(),
                    ..Default::default()
                },
                primary_metric: MetricTarget {
                    name: "latency_ms".to_string(),
                    goal: MetricGoal::Minimize,
                    unit: Some("ms".to_string()),
                    notes: None,
                },
                guardrails: vec![Guardrail {
                    name: "throughput".to_string(),
                    goal: MetricGoal::Maximize,
                    max_regression_pct: 5.0,
                    notes: None,
                }],
            })
            .unwrap();
        catalog
            .record_baseline(
                "score-demo",
                RecordedRun {
                    label: "baseline".to_string(),
                    recorded_at_unix_ms: 1,
                    scheduler: SchedulerKind::Cfs,
                    artifact_dir: "artifacts/baseline".to_string(),
                    metrics: MetricMap::from([
                        ("latency_ms".to_string(), 10.0),
                        ("throughput".to_string(), 100.0),
                    ]),
                    notes: None,
                },
            )
            .unwrap();
        let loaded = catalog.load("score-demo").unwrap();
        let mut manifest = loaded.manifest;
        manifest.candidates.push(CandidateRecord {
            spec: CandidateSpec {
                candidate_id: "cand-a".to_string(),
                template: "locality".to_string(),
                source_path: None,
                build_command: None,
                daemon_argv: Vec::new(),
                daemon_cwd: None,
                daemon_env: BTreeMap::new(),
                knobs: BTreeMap::new(),
                notes: None,
            },
            runs: vec![RecordedRun {
                label: "candidate".to_string(),
                recorded_at_unix_ms: 2,
                scheduler: SchedulerKind::SchedExt,
                artifact_dir: "artifacts/candidate".to_string(),
                metrics: MetricMap::from([
                    ("latency_ms".to_string(), 8.0),
                    ("throughput".to_string(), 90.0),
                ]),
                notes: None,
            }],
        });
        super::write_manifest(&catalog.manifest_path_for_id("score-demo"), &manifest).unwrap();

        let report = catalog.score("score-demo").unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].decision, CandidateDecision::Blocked);
    }

    #[test]
    fn set_candidate_replaces_existing_spec() {
        let dir = tempdir().unwrap();
        let catalog = ExperimentCatalog::open(dir.path()).unwrap();
        catalog
            .init(ExperimentInitSpec {
                experiment_id: "demo".to_string(),
                workload: WorkloadContract {
                    name: "bench".to_string(),
                    ..Default::default()
                },
                primary_metric: MetricTarget {
                    name: "latency_ms".to_string(),
                    goal: MetricGoal::Minimize,
                    unit: Some("ms".to_string()),
                    notes: None,
                },
                guardrails: Vec::new(),
            })
            .unwrap();
        catalog
            .set_candidate(
                "demo",
                CandidateSpec {
                    candidate_id: "cand-a".to_string(),
                    template: "latency_guard".to_string(),
                    source_path: Some("sources/a.bpf.c".to_string()),
                    build_command: None,
                    daemon_argv: vec!["loader".to_string()],
                    daemon_cwd: None,
                    daemon_env: BTreeMap::new(),
                    knobs: BTreeMap::from([("slice_us".to_string(), "1000".to_string())]),
                    notes: None,
                },
            )
            .unwrap();
        catalog
            .set_candidate(
                "demo",
                CandidateSpec {
                    candidate_id: "cand-a".to_string(),
                    template: "dsq_locality".to_string(),
                    source_path: Some("sources/b.bpf.c".to_string()),
                    build_command: Some("clang ...".to_string()),
                    daemon_argv: vec!["loader".to_string(), "sources/b.bpf.c".to_string()],
                    daemon_cwd: Some("/tmp".to_string()),
                    daemon_env: BTreeMap::from([("MODE".to_string(), "candidate".to_string())]),
                    knobs: BTreeMap::new(),
                    notes: Some("updated".to_string()),
                },
            )
            .unwrap();
        let loaded = catalog.load("demo").unwrap();
        assert_eq!(loaded.manifest.candidates.len(), 1);
        assert_eq!(loaded.manifest.candidates[0].spec.template, "dsq_locality");
        assert_eq!(
            loaded.manifest.candidates[0].spec.source_path.as_deref(),
            Some("sources/b.bpf.c")
        );
    }

    #[test]
    fn records_deployment_history() {
        let dir = tempdir().unwrap();
        let catalog = ExperimentCatalog::open(dir.path()).unwrap();
        catalog
            .init(ExperimentInitSpec {
                experiment_id: "demo".to_string(),
                workload: WorkloadContract {
                    name: "bench".to_string(),
                    ..Default::default()
                },
                primary_metric: MetricTarget {
                    name: "latency_ms".to_string(),
                    goal: MetricGoal::Minimize,
                    unit: Some("ms".to_string()),
                    notes: None,
                },
                guardrails: Vec::new(),
            })
            .unwrap();
        catalog
            .record_deployment(
                "demo",
                DeploymentRecord {
                    candidate_id: "cand-a".to_string(),
                    requested_at_unix_ms: 42,
                    label: "demo:cand-a".to_string(),
                    daemon_pid: 1001,
                    argv: vec!["loader".to_string()],
                    cwd: Some("/tmp".to_string()),
                    env: BTreeMap::new(),
                    source_path: Some("sources/cand-a.bpf.c".to_string()),
                    replace_existing: false,
                },
            )
            .unwrap();
        let loaded = catalog.load("demo").unwrap();
        assert_eq!(loaded.manifest.deployments.len(), 1);
        assert_eq!(loaded.manifest.deployments[0].daemon_pid, 1001);
    }
}
