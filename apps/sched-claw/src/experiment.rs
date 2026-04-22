use crate::app_config::app_state_dir;
use crate::metrics::{
    Guardrail, MetricMap, MetricTarget, PerformancePolicy, median_metric, relative_spread_pct,
};
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
    #[serde(default)]
    pub performance_policy: PerformancePolicy,
    #[serde(default)]
    pub evaluation_policy: EvaluationPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guardrails: Vec<Guardrail>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline_runs: Vec<RecordedRun>,
    // Keep collection artifacts, analysis conclusions, and codegen intent durable
    // outside transcript text so later turns and operator tooling can reuse them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub analyses: Vec<AnalysisRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub designs: Vec<DesignRecord>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builds: Vec<CandidateBuildRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateSpec {
    pub candidate_id: String,
    pub template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_path: Option<String>,
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
pub struct CandidateBuildRecord {
    pub requested_at_unix_ms: u64,
    pub artifact_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_path: Option<String>,
    pub build: StepCommandRecord,
    pub verifier: VerifierCommandRecord,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepCommandRecord {
    pub status: CommandStatus,
    pub command: String,
    pub command_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_path: String,
    pub stderr_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerifierCommandRecord {
    pub backend: VerifierBackend,
    pub status: CommandStatus,
    pub command: String,
    pub command_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_path: String,
    pub stderr_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Success,
    Failed,
    Skipped,
}

impl CommandStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierBackend {
    BpftoolProgLoadall,
}

impl VerifierBackend {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BpftoolProgLoadall => "bpftool_prog_loadall",
        }
    }
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
pub struct EvidenceRecord {
    pub evidence_id: String,
    pub recorded_at_unix_ms: u64,
    pub kind: EvidenceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<SchedulerKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: MetricMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    PerfStat,
    PerfSched,
    PerfRecord,
    Psi,
    Schedstat,
    BpfTrace,
    Custom,
}

impl EvidenceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerfStat => "perf_stat",
            Self::PerfSched => "perf_sched",
            Self::PerfRecord => "perf_record",
            Self::Psi => "psi",
            Self::Schedstat => "schedstat",
            Self::BpfTrace => "bpf_trace",
            Self::Custom => "custom",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalysisRecord {
    pub analysis_id: String,
    pub recorded_at_unix_ms: u64,
    pub title: String,
    pub confidence: AnalysisConfidence,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inferences: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknowns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recommendations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisConfidence {
    Low,
    Medium,
    High,
}

impl AnalysisConfidence {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignRecord {
    pub design_id: String,
    pub recorded_at_unix_ms: u64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub analysis_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_levers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code_targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_timeout_ms: Option<u64>,
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
    pub performance_policy: PerformancePolicy,
    pub evaluation_policy: EvaluationPolicy,
    pub guardrails: Vec<Guardrail>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationPolicy {
    #[serde(default = "default_min_run_count")]
    pub min_baseline_runs: usize,
    #[serde(default = "default_min_run_count")]
    pub min_candidate_runs: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_primary_improvement_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_primary_relative_spread_pct: Option<f64>,
}

impl Default for EvaluationPolicy {
    fn default() -> Self {
        Self {
            min_baseline_runs: default_min_run_count(),
            min_candidate_runs: default_min_run_count(),
            min_primary_improvement_pct: None,
            max_primary_relative_spread_pct: None,
        }
    }
}

impl EvaluationPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.min_baseline_runs == 0 {
            bail!("min_baseline_runs must be at least 1");
        }
        if self.min_candidate_runs == 0 {
            bail!("min_candidate_runs must be at least 1");
        }
        if let Some(value) = self.min_primary_improvement_pct
            && (!value.is_finite() || value < 0.0)
        {
            bail!("min_primary_improvement_pct must be a finite non-negative number");
        }
        if let Some(value) = self.max_primary_relative_spread_pct
            && (!value.is_finite() || value < 0.0)
        {
            bail!("max_primary_relative_spread_pct must be a finite non-negative number");
        }
        Ok(())
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("baseline>={}", self.min_baseline_runs),
            format!("candidate>={}", self.min_candidate_runs),
        ];
        if let Some(value) = self.min_primary_improvement_pct {
            parts.push(format!("improvement>={value:.2}%"));
        }
        if let Some(value) = self.max_primary_relative_spread_pct {
            parts.push(format!("spread<={value:.2}%"));
        }
        parts.join(" / ")
    }
}

#[derive(Clone, Debug)]
pub struct ExperimentSummary {
    pub experiment_id: String,
    pub manifest_path: PathBuf,
    pub updated_at_unix_ms: u64,
    pub workload_name: String,
    pub primary_metric_name: String,
    pub baseline_run_count: usize,
    pub evidence_count: usize,
    pub analysis_count: usize,
    pub design_count: usize,
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
    pub evaluation_policy: EvaluationPolicy,
    pub baseline_run_count: usize,
    pub baseline_primary_value: Option<f64>,
    pub baseline_primary_relative_spread_pct: Option<f64>,
    pub entries: Vec<CandidateScore>,
}

#[derive(Clone, Debug)]
pub struct CandidateScore {
    pub candidate_id: String,
    pub template: String,
    pub run_count: usize,
    pub primary_candidate_value: Option<f64>,
    pub primary_improvement_pct: Option<f64>,
    pub candidate_primary_relative_spread_pct: Option<f64>,
    pub decision: CandidateDecision,
    pub breached_guardrails: Vec<GuardrailScore>,
    pub status_reasons: Vec<String>,
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
                evidence_count: manifest.evidence.len(),
                analysis_count: manifest.analyses.len(),
                design_count: manifest.designs.len(),
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
        spec.performance_policy.validate()?;
        spec.evaluation_policy.validate()?;
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
            performance_policy: spec.performance_policy,
            evaluation_policy: spec.evaluation_policy,
            guardrails: spec.guardrails,
            baseline_runs: Vec::new(),
            evidence: Vec::new(),
            analyses: Vec::new(),
            designs: Vec::new(),
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
            builds: Vec::new(),
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
                builds: Vec::new(),
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

    pub fn record_evidence(
        &self,
        reference: &str,
        record: EvidenceRecord,
    ) -> Result<ExperimentArtifact> {
        validate_identifier("evidence id", &record.evidence_id)?;
        let mut loaded = self.load(reference)?;
        if loaded
            .manifest
            .evidence
            .iter()
            .any(|entry| entry.evidence_id == record.evidence_id)
        {
            bail!(
                "evidence {} already exists in experiment {}",
                record.evidence_id,
                loaded.manifest.experiment_id
            );
        }
        if let Some(candidate_id) = record.candidate_id.as_deref() {
            require_candidate(&loaded.manifest, candidate_id)?;
        }
        let details = vec![
            ("evidence".to_string(), record.evidence_id.clone()),
            ("kind".to_string(), record.kind.as_str().to_string()),
            (
                "artifacts".to_string(),
                record.artifact_paths.len().to_string(),
            ),
        ];
        loaded.manifest.evidence.push(record);
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded evidence",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details,
        })
    }

    pub fn record_analysis(
        &self,
        reference: &str,
        record: AnalysisRecord,
    ) -> Result<ExperimentArtifact> {
        validate_identifier("analysis id", &record.analysis_id)?;
        let mut loaded = self.load(reference)?;
        if loaded
            .manifest
            .analyses
            .iter()
            .any(|entry| entry.analysis_id == record.analysis_id)
        {
            bail!(
                "analysis {} already exists in experiment {}",
                record.analysis_id,
                loaded.manifest.experiment_id
            );
        }
        for evidence_id in &record.evidence_ids {
            require_evidence(&loaded.manifest, evidence_id)?;
        }
        let details = vec![
            ("analysis".to_string(), record.analysis_id.clone()),
            (
                "confidence".to_string(),
                record.confidence.as_str().to_string(),
            ),
            (
                "evidence_refs".to_string(),
                record.evidence_ids.len().to_string(),
            ),
        ];
        loaded.manifest.analyses.push(record);
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded analysis",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details,
        })
    }

    pub fn record_design(
        &self,
        reference: &str,
        record: DesignRecord,
    ) -> Result<ExperimentArtifact> {
        validate_identifier("design id", &record.design_id)?;
        let mut loaded = self.load(reference)?;
        if loaded
            .manifest
            .designs
            .iter()
            .any(|entry| entry.design_id == record.design_id)
        {
            bail!(
                "design {} already exists in experiment {}",
                record.design_id,
                loaded.manifest.experiment_id
            );
        }
        if let Some(candidate_id) = record.candidate_id.as_deref() {
            require_candidate(&loaded.manifest, candidate_id)?;
        }
        for evidence_id in &record.evidence_ids {
            require_evidence(&loaded.manifest, evidence_id)?;
        }
        for analysis_id in &record.analysis_ids {
            require_analysis(&loaded.manifest, analysis_id)?;
        }
        let details = vec![
            ("design".to_string(), record.design_id.clone()),
            (
                "candidate".to_string(),
                record
                    .candidate_id
                    .clone()
                    .unwrap_or_else(|| "<none>".to_string()),
            ),
            (
                "policy_levers".to_string(),
                record.policy_levers.len().to_string(),
            ),
        ];
        loaded.manifest.designs.push(record);
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded design",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details,
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

    pub fn record_candidate_build(
        &self,
        reference: &str,
        candidate_id: &str,
        build: CandidateBuildRecord,
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
        let build_status = build.build.status.as_str().to_string();
        let verify_status = build.verifier.status.as_str().to_string();
        let artifact_dir = build.artifact_dir.clone();
        candidate.builds.push(build);
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "recorded build",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: vec![
                ("candidate".to_string(), candidate_id.to_string()),
                ("build".to_string(), build_status),
                ("verify".to_string(), verify_status),
                ("artifact_dir".to_string(), artifact_dir),
            ],
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

    pub fn set_evaluation_policy(
        &self,
        reference: &str,
        policy: EvaluationPolicy,
    ) -> Result<ExperimentArtifact> {
        policy.validate()?;
        let mut loaded = self.load(reference)?;
        loaded.manifest.evaluation_policy = policy.clone();
        touch_manifest(&mut loaded.manifest);
        write_manifest(&loaded.manifest_path, &loaded.manifest)?;
        Ok(ExperimentArtifact {
            action: "updated evaluation policy",
            experiment_id: loaded.manifest.experiment_id,
            manifest_path: loaded.manifest_path,
            details: vec![("policy".to_string(), policy.summary())],
        })
    }

    pub fn score(&self, reference: &str) -> Result<ExperimentScoreReport> {
        let loaded = self.load(reference)?;
        let baseline_primary_value = median_metric(
            &loaded.manifest.primary_metric.name,
            loaded.manifest.baseline_runs.iter().map(|run| &run.metrics),
        );
        let baseline_primary_relative_spread_pct = relative_spread_pct(
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
                    &loaded.manifest.evaluation_policy,
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
            evaluation_policy: loaded.manifest.evaluation_policy,
            baseline_run_count: loaded.manifest.baseline_runs.len(),
            baseline_primary_value,
            baseline_primary_relative_spread_pct,
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

fn require_candidate(manifest: &ExperimentManifest, candidate_id: &str) -> Result<()> {
    if manifest
        .candidates
        .iter()
        .any(|candidate| candidate.spec.candidate_id == candidate_id)
    {
        Ok(())
    } else {
        bail!(
            "unknown candidate {} in experiment {}",
            candidate_id,
            manifest.experiment_id
        )
    }
}

fn require_evidence(manifest: &ExperimentManifest, evidence_id: &str) -> Result<()> {
    // Cross-record references stay local to one experiment manifest so the host
    // can validate them synchronously without inventing a second index layer.
    if manifest
        .evidence
        .iter()
        .any(|evidence| evidence.evidence_id == evidence_id)
    {
        Ok(())
    } else {
        bail!(
            "unknown evidence {} in experiment {}",
            evidence_id,
            manifest.experiment_id
        )
    }
}

fn require_analysis(manifest: &ExperimentManifest, analysis_id: &str) -> Result<()> {
    if manifest
        .analyses
        .iter()
        .any(|analysis| analysis.analysis_id == analysis_id)
    {
        Ok(())
    } else {
        bail!(
            "unknown analysis {} in experiment {}",
            analysis_id,
            manifest.experiment_id
        )
    }
}

fn default_min_run_count() -> usize {
    1
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
    evaluation_policy: &EvaluationPolicy,
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
    let baseline_primary_relative_spread_pct = relative_spread_pct(
        &primary_metric.name,
        baseline_runs.iter().map(|run| &run.metrics),
    );
    let candidate_primary_relative_spread_pct = relative_spread_pct(
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
    let mut status_reasons = Vec::new();
    if baseline_runs.len() < evaluation_policy.min_baseline_runs {
        status_reasons.push(format!(
            "baseline runs {} below required {}",
            baseline_runs.len(),
            evaluation_policy.min_baseline_runs
        ));
    }
    if candidate.runs.len() < evaluation_policy.min_candidate_runs {
        status_reasons.push(format!(
            "candidate runs {} below required {}",
            candidate.runs.len(),
            evaluation_policy.min_candidate_runs
        ));
    }
    if baseline_primary_value.is_none() {
        status_reasons.push("missing baseline primary metric".to_string());
    }
    if primary_candidate_value.is_none() {
        status_reasons.push("missing candidate primary metric".to_string());
    }
    if let Some(max_spread_pct) = evaluation_policy.max_primary_relative_spread_pct {
        if let Some(value) = baseline_primary_relative_spread_pct {
            if value > max_spread_pct {
                status_reasons.push(format!(
                    "baseline primary spread {:.2}% exceeds {:.2}%",
                    value, max_spread_pct
                ));
            }
        }
        if let Some(value) = candidate_primary_relative_spread_pct {
            if value > max_spread_pct {
                status_reasons.push(format!(
                    "candidate primary spread {:.2}% exceeds {:.2}%",
                    value, max_spread_pct
                ));
            }
        }
    }
    for score in &breached_guardrails {
        if matches!(score.status, GuardrailStatus::Missing) {
            status_reasons.push(format!("missing guardrail metric {}", score.name));
        }
    }
    if let Some(min_improvement_pct) = evaluation_policy.min_primary_improvement_pct {
        if let Some(value) = primary_improvement_pct {
            if value > 0.0 && value < min_improvement_pct {
                status_reasons.push(format!(
                    "primary improvement {:.2}% below required {:.2}%",
                    value, min_improvement_pct
                ));
            }
        }
    }
    let decision = if baseline_primary_value.is_none()
        || primary_candidate_value.is_none()
        || baseline_runs.len() < evaluation_policy.min_baseline_runs
        || candidate.runs.len() < evaluation_policy.min_candidate_runs
        || breached_guardrails
            .iter()
            .any(|score| matches!(score.status, GuardrailStatus::Missing))
        || evaluation_policy
            .max_primary_relative_spread_pct
            .is_some_and(|max_spread_pct| {
                baseline_primary_relative_spread_pct.is_some_and(|value| value > max_spread_pct)
                    || candidate_primary_relative_spread_pct
                        .is_some_and(|value| value > max_spread_pct)
            }) {
        CandidateDecision::Incomplete
    } else if breached_guardrails
        .iter()
        .any(|score| matches!(score.status, GuardrailStatus::Breach))
    {
        CandidateDecision::Blocked
    } else if primary_improvement_pct.is_some_and(|value| {
        value > 0.0
            && value
                >= evaluation_policy
                    .min_primary_improvement_pct
                    .unwrap_or_default()
    }) {
        CandidateDecision::Promote
    } else {
        CandidateDecision::Revise
    };
    if matches!(decision, CandidateDecision::Revise) {
        if let Some(value) = primary_improvement_pct {
            if value <= 0.0 {
                status_reasons.push(format!("primary metric did not improve ({value:.2}%)"));
            }
        }
    }
    CandidateScore {
        candidate_id: candidate.spec.candidate_id.clone(),
        template: candidate.spec.template.clone(),
        run_count: candidate.runs.len(),
        primary_candidate_value,
        primary_improvement_pct,
        candidate_primary_relative_spread_pct,
        decision,
        breached_guardrails,
        status_reasons,
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
        AnalysisConfidence, AnalysisRecord, CandidateBuildRecord, CandidateDecision,
        CandidateRecord, CandidateSpec, CommandStatus, DeploymentRecord, DesignRecord,
        EvaluationPolicy, EvidenceKind, EvidenceRecord, ExperimentCatalog, ExperimentInitSpec,
        RecordedRun, SchedulerKind, StepCommandRecord, VerifierBackend, VerifierCommandRecord,
        experiments_dir,
    };
    use crate::metrics::{Guardrail, MetricGoal, MetricMap, MetricTarget, PerformancePolicy};
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
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
                object_path: None,
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
            builds: Vec::new(),
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
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
                    object_path: Some("sources/a.bpf.o".to_string()),
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
                    object_path: Some("sources/b.bpf.o".to_string()),
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
        assert_eq!(
            loaded.manifest.candidates[0].spec.object_path.as_deref(),
            Some("sources/b.bpf.o")
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
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
                    lease_timeout_ms: Some(1_000),
                    replace_existing: false,
                },
            )
            .unwrap();
        let loaded = catalog.load("demo").unwrap();
        assert_eq!(loaded.manifest.deployments.len(), 1);
        assert_eq!(loaded.manifest.deployments[0].daemon_pid, 1001);
    }

    #[test]
    fn records_candidate_build_history() {
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
                guardrails: Vec::new(),
            })
            .unwrap();
        catalog
            .set_candidate(
                "demo",
                CandidateSpec {
                    candidate_id: "cand-a".to_string(),
                    template: "latency_guard".to_string(),
                    source_path: Some("sources/cand-a.bpf.c".to_string()),
                    object_path: Some("sources/cand-a.bpf.o".to_string()),
                    build_command: Some("clang ...".to_string()),
                    daemon_argv: Vec::new(),
                    daemon_cwd: None,
                    daemon_env: BTreeMap::new(),
                    knobs: BTreeMap::new(),
                    notes: None,
                },
            )
            .unwrap();
        catalog
            .record_candidate_build(
                "demo",
                "cand-a",
                CandidateBuildRecord {
                    requested_at_unix_ms: 10,
                    artifact_dir: "artifacts/builds/cand-a/10".to_string(),
                    source_path: Some("sources/cand-a.bpf.c".to_string()),
                    object_path: Some("sources/cand-a.bpf.o".to_string()),
                    build: StepCommandRecord {
                        status: CommandStatus::Success,
                        command: "clang ...".to_string(),
                        command_path: "artifacts/builds/cand-a/10/build.command.txt".to_string(),
                        exit_code: Some(0),
                        duration_ms: 12,
                        stdout_path: "artifacts/builds/cand-a/10/build.stdout.log".to_string(),
                        stderr_path: "artifacts/builds/cand-a/10/build.stderr.log".to_string(),
                        summary: Some("build completed successfully".to_string()),
                    },
                    verifier: VerifierCommandRecord {
                        backend: VerifierBackend::BpftoolProgLoadall,
                        status: CommandStatus::Failed,
                        command: "bpftool ...".to_string(),
                        command_path: "artifacts/builds/cand-a/10/verify.command.txt".to_string(),
                        exit_code: Some(1),
                        duration_ms: 5,
                        stdout_path: "artifacts/builds/cand-a/10/verify.stdout.log".to_string(),
                        stderr_path: "artifacts/builds/cand-a/10/verify.stderr.log".to_string(),
                        summary: Some("libbpf: verifier rejected fake object".to_string()),
                    },
                },
            )
            .unwrap();
        let loaded = catalog.load("demo").unwrap();
        assert_eq!(loaded.manifest.candidates[0].builds.len(), 1);
        assert_eq!(
            loaded.manifest.candidates[0].builds[0].verifier.backend,
            VerifierBackend::BpftoolProgLoadall
        );
    }

    #[test]
    fn records_evidence_analysis_and_design_history() {
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
                guardrails: Vec::new(),
            })
            .unwrap();
        catalog
            .set_candidate(
                "demo",
                CandidateSpec {
                    candidate_id: "cand-a".to_string(),
                    template: "latency_guard".to_string(),
                    source_path: Some("sources/cand-a.bpf.c".to_string()),
                    object_path: Some("sources/cand-a.bpf.o".to_string()),
                    build_command: Some("clang ...".to_string()),
                    daemon_argv: Vec::new(),
                    daemon_cwd: None,
                    daemon_env: BTreeMap::new(),
                    knobs: BTreeMap::new(),
                    notes: None,
                },
            )
            .unwrap();
        catalog
            .record_evidence(
                "demo",
                EvidenceRecord {
                    evidence_id: "perf-a".to_string(),
                    recorded_at_unix_ms: 1,
                    kind: EvidenceKind::PerfStat,
                    collector: Some("perf stat -d -d".to_string()),
                    focus: Some("retiring efficiency".to_string()),
                    phase: Some("baseline".to_string()),
                    scheduler: Some(SchedulerKind::Cfs),
                    candidate_id: None,
                    artifact_paths: vec!["artifacts/evidence/perf-a.txt".to_string()],
                    metrics: MetricMap::from([("ipc".to_string(), 1.2)]),
                    summary: Some("ipc stayed below 1.3".to_string()),
                    notes: vec!["noisy host".to_string()],
                },
            )
            .unwrap();
        catalog
            .record_analysis(
                "demo",
                AnalysisRecord {
                    analysis_id: "analysis-a".to_string(),
                    recorded_at_unix_ms: 2,
                    title: "Baseline locality diagnosis".to_string(),
                    confidence: AnalysisConfidence::Medium,
                    evidence_ids: vec!["perf-a".to_string()],
                    facts: vec!["ipc is lower than expected".to_string()],
                    inferences: vec!["cross-cpu migration may be too eager".to_string()],
                    unknowns: vec!["llc miss attribution is missing".to_string()],
                    recommendations: vec!["strengthen locality bias".to_string()],
                    summary: Some("locality is a plausible lever".to_string()),
                },
            )
            .unwrap();
        catalog
            .record_design(
                "demo",
                DesignRecord {
                    design_id: "design-a".to_string(),
                    recorded_at_unix_ms: 3,
                    title: "Locality-first candidate".to_string(),
                    candidate_id: Some("cand-a".to_string()),
                    evidence_ids: vec!["perf-a".to_string()],
                    analysis_ids: vec!["analysis-a".to_string()],
                    policy_levers: vec!["prefer same-cpu wakeups".to_string()],
                    invariants: vec!["do not starve remote tasks".to_string()],
                    code_targets: vec!["pick_idle_cpu".to_string()],
                    risks: vec!["can reduce throughput".to_string()],
                    fallback_criteria: vec!["rollback if throughput regresses >5%".to_string()],
                    summary: Some("first design pass".to_string()),
                },
            )
            .unwrap();

        let loaded = catalog.load("demo").unwrap();
        assert_eq!(loaded.manifest.evidence.len(), 1);
        assert_eq!(loaded.manifest.analyses.len(), 1);
        assert_eq!(loaded.manifest.designs.len(), 1);
        assert_eq!(
            loaded.manifest.designs[0].candidate_id.as_deref(),
            Some("cand-a")
        );
    }

    #[test]
    fn score_marks_candidate_incomplete_when_evidence_policy_is_not_met() {
        let dir = tempdir().unwrap();
        let catalog = ExperimentCatalog::open(dir.path()).unwrap();
        catalog
            .init(ExperimentInitSpec {
                experiment_id: "policy-demo".to_string(),
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy {
                    min_baseline_runs: 2,
                    min_candidate_runs: 2,
                    min_primary_improvement_pct: Some(5.0),
                    max_primary_relative_spread_pct: Some(10.0),
                },
                guardrails: Vec::new(),
            })
            .unwrap();
        catalog
            .record_baseline(
                "policy-demo",
                RecordedRun {
                    label: "baseline-a".to_string(),
                    recorded_at_unix_ms: 1,
                    scheduler: SchedulerKind::Cfs,
                    artifact_dir: "artifacts/baseline-a".to_string(),
                    metrics: MetricMap::from([("latency_ms".to_string(), 10.0)]),
                    notes: None,
                },
            )
            .unwrap();
        catalog
            .set_candidate(
                "policy-demo",
                CandidateSpec {
                    candidate_id: "cand-a".to_string(),
                    template: "latency_guard".to_string(),
                    source_path: None,
                    object_path: None,
                    build_command: None,
                    daemon_argv: Vec::new(),
                    daemon_cwd: None,
                    daemon_env: BTreeMap::new(),
                    knobs: BTreeMap::new(),
                    notes: None,
                },
            )
            .unwrap();
        catalog
            .record_candidate(
                "policy-demo",
                "cand-a",
                RecordedRun {
                    label: "candidate-a".to_string(),
                    recorded_at_unix_ms: 2,
                    scheduler: SchedulerKind::SchedExt,
                    artifact_dir: "artifacts/candidate-a".to_string(),
                    metrics: MetricMap::from([("latency_ms".to_string(), 8.0)]),
                    notes: None,
                },
            )
            .unwrap();

        let report = catalog.score("policy-demo").unwrap();
        assert_eq!(report.entries[0].decision, CandidateDecision::Incomplete);
        assert!(
            report.entries[0]
                .status_reasons
                .iter()
                .any(|reason| reason.contains("below required"))
        );
    }

    #[test]
    fn set_evaluation_policy_updates_manifest() {
        let dir = tempdir().unwrap();
        let catalog = ExperimentCatalog::open(dir.path()).unwrap();
        catalog
            .init(ExperimentInitSpec {
                experiment_id: "policy-update".to_string(),
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
                performance_policy: PerformancePolicy::default(),
                evaluation_policy: EvaluationPolicy::default(),
                guardrails: Vec::new(),
            })
            .unwrap();
        catalog
            .set_evaluation_policy(
                "policy-update",
                EvaluationPolicy {
                    min_baseline_runs: 3,
                    min_candidate_runs: 4,
                    min_primary_improvement_pct: Some(2.5),
                    max_primary_relative_spread_pct: Some(9.0),
                },
            )
            .unwrap();

        let loaded = catalog.load("policy-update").unwrap();
        assert_eq!(loaded.manifest.evaluation_policy.min_baseline_runs, 3);
        assert_eq!(loaded.manifest.evaluation_policy.min_candidate_runs, 4);
        assert_eq!(
            loaded
                .manifest
                .evaluation_policy
                .min_primary_improvement_pct,
            Some(2.5)
        );
        assert_eq!(
            loaded
                .manifest
                .evaluation_policy
                .max_primary_relative_spread_pct,
            Some(9.0)
        );
    }
}
