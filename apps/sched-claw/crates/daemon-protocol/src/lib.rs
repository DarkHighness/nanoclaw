use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_STOP_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_LOG_TAIL_LINES: usize = 100;
pub const MIN_PERF_DURATION_MS: u64 = 100;
pub const MAX_PERF_DURATION_MS: u64 = 5 * 60_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonCapabilityKind {
    DeploymentControl,
    PerfStatCapture,
    PerfRecordCapture,
    SchedulerTraceCapture,
    SchedStateSnapshot,
    PressureSnapshot,
    TopologySnapshot,
}

#[derive(
    Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq, Ord, PartialOrd,
)]
#[serde(rename_all = "snake_case")]
pub enum DaemonCapabilityName {
    DeploymentControl,
    PerfStatCapture,
    PerfRecordCapture,
    SchedulerTraceCapture,
    SchedStateSnapshot,
    PressureSnapshot,
    TopologySnapshot,
}

impl DaemonCapabilityName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeploymentControl => "deployment_control",
            Self::PerfStatCapture => "perf_stat_capture",
            Self::PerfRecordCapture => "perf_record_capture",
            Self::SchedulerTraceCapture => "scheduler_trace_capture",
            Self::SchedStateSnapshot => "sched_state_snapshot",
            Self::PressureSnapshot => "pressure_snapshot",
            Self::TopologySnapshot => "topology_snapshot",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonSelectorKind {
    Pid,
    Uid,
    Gid,
    Cgroup,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
pub struct DaemonCapabilityDescriptor {
    pub name: DaemonCapabilityName,
    pub kind: DaemonCapabilityKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selector_kinds: Vec<DaemonSelectorKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    pub requires_root: bool,
}

#[must_use]
pub fn expected_daemon_capabilities() -> Vec<DaemonCapabilityDescriptor> {
    vec![
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::DeploymentControl,
            kind: DaemonCapabilityKind::DeploymentControl,
            summary: "Activate, inspect, and stop a bounded sched-ext rollout without exposing an unrestricted root shell.".to_string(),
            selector_kinds: Vec::new(),
            outputs: vec![
                "daemon status snapshot".to_string(),
                "deployment log tail".to_string(),
                "deployment exit snapshot".to_string(),
            ],
            constraints: vec![
                "argv and cwd must resolve inside allowed roots".to_string(),
                "only one active deployment at a time".to_string(),
                "optional rollout lease enforces automatic stop".to_string(),
            ],
            requires_root: true,
        },
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::PerfStatCapture,
            kind: DaemonCapabilityKind::PerfStatCapture,
            summary: "Attach bounded perf stat capture to an existing pid, uid, gid, or cgroup target.".to_string(),
            selector_kinds: perf_selector_kinds(),
            outputs: vec![
                "perf.stat.csv".to_string(),
                "perf.command.json".to_string(),
                "perf.selector.json".to_string(),
                "perf stdout and stderr logs".to_string(),
            ],
            constraints: vec![
                perf_duration_constraint(),
                "output_dir must resolve inside allowed roots".to_string(),
                "events are validated and shell expansion is not allowed".to_string(),
            ],
            requires_root: true,
        },
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::PerfRecordCapture,
            kind: DaemonCapabilityKind::PerfRecordCapture,
            summary: "Attach bounded perf record capture with optional sample frequency and call graph mode.".to_string(),
            selector_kinds: perf_selector_kinds(),
            outputs: vec![
                "perf.data".to_string(),
                "perf.command.json".to_string(),
                "perf.selector.json".to_string(),
                "perf stdout and stderr logs".to_string(),
            ],
            constraints: vec![
                perf_duration_constraint(),
                "sample_frequency_hz and call_graph are only valid for record mode".to_string(),
                "output_dir must resolve inside allowed roots".to_string(),
            ],
            requires_root: true,
        },
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::SchedulerTraceCapture,
            kind: DaemonCapabilityKind::SchedulerTraceCapture,
            summary: "Capture bounded perf sched record, timehist, and latency artifacts for scheduler ordering evidence.".to_string(),
            selector_kinds: perf_selector_kinds(),
            outputs: vec![
                "perf.sched.data".to_string(),
                "perf.sched.timehist.txt".to_string(),
                "perf.sched.latency.txt".to_string(),
                "command, selector, and stderr artifacts".to_string(),
            ],
            constraints: vec![
                perf_duration_constraint(),
                "output_dir must resolve inside allowed roots".to_string(),
                "shell execution is not permitted; only structured selectors are accepted".to_string(),
            ],
            requires_root: true,
        },
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::SchedStateSnapshot,
            kind: DaemonCapabilityKind::SchedStateSnapshot,
            summary: "Capture bounded read-only scheduler state snapshots from procfs for a pid, uid, gid, or cgroup target.".to_string(),
            selector_kinds: perf_selector_kinds(),
            outputs: vec![
                "proc.schedstat".to_string(),
                "per-pid sched, schedstat, status, and cgroup artifacts".to_string(),
                "snapshot index and selector metadata".to_string(),
            ],
            constraints: vec![
                "output_dir must resolve inside allowed roots".to_string(),
                "selectors resolve to live pids before capture".to_string(),
                "shell execution is not permitted; procfs files are copied directly".to_string(),
            ],
            requires_root: true,
        },
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::PressureSnapshot,
            kind: DaemonCapabilityKind::PressureSnapshot,
            summary: "Capture bounded read-only pressure and cgroup state snapshots for a pid, uid, gid, or cgroup target.".to_string(),
            selector_kinds: perf_selector_kinds(),
            outputs: vec![
                "proc.pressure cpu, io, and memory artifacts".to_string(),
                "per-pid cgroup membership artifacts".to_string(),
                "per-cgroup pressure, cpu.stat, and cpuset artifacts".to_string(),
                "snapshot index and selector metadata".to_string(),
            ],
            constraints: vec![
                "output_dir must resolve inside allowed roots".to_string(),
                "selectors resolve to live pids before capture".to_string(),
                "only read-only procfs and cgroupfs files are copied".to_string(),
            ],
            requires_root: true,
        },
        DaemonCapabilityDescriptor {
            name: DaemonCapabilityName::TopologySnapshot,
            kind: DaemonCapabilityKind::TopologySnapshot,
            summary: "Capture bounded read-only CPU, NUMA, SMT, and selector-scoped cpuset topology context.".to_string(),
            selector_kinds: perf_selector_kinds(),
            outputs: vec![
                "host CPU and NUMA summary artifacts".to_string(),
                "per-cpu topology summary json".to_string(),
                "optional per-pid status and cgroup cpuset artifacts".to_string(),
                "snapshot index and selector metadata".to_string(),
            ],
            constraints: vec![
                "output_dir must resolve inside allowed roots".to_string(),
                "selector is optional, but when present it must resolve to live pids".to_string(),
                "shell execution is not permitted; sysfs and procfs files are copied directly".to_string(),
            ],
            requires_root: true,
        },
    ]
}

fn perf_selector_kinds() -> Vec<DaemonSelectorKind> {
    vec![
        DaemonSelectorKind::Pid,
        DaemonSelectorKind::Uid,
        DaemonSelectorKind::Gid,
        DaemonSelectorKind::Cgroup,
    ]
}

fn perf_duration_constraint() -> String {
    format!(
        "duration_ms must stay within {}..={}",
        MIN_PERF_DURATION_MS, MAX_PERF_DURATION_MS
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SchedClawDaemonRequest {
    Status {},
    Capabilities {},
    Logs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail_lines: Option<usize>,
    },
    Invoke {
        invocation: DaemonCapabilityInvocation,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonCapabilityInvocation {
    RolloutActivate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease_timeout_ms: Option<u64>,
        #[serde(default)]
        replace_existing: bool,
    },
    RolloutStop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        graceful_timeout_ms: Option<u64>,
    },
    PerfCapture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        mode: PerfCollectionMode,
        selector: DaemonTargetSelector,
        output_dir: String,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        events: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_frequency_hz: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_graph: Option<PerfCallGraphMode>,
        #[serde(default)]
        overwrite: bool,
    },
    SchedulerTraceCapture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        selector: DaemonTargetSelector,
        output_dir: String,
        duration_ms: u64,
        #[serde(default)]
        latency_by_pid: bool,
        #[serde(default)]
        overwrite: bool,
    },
    SchedStateSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        selector: DaemonTargetSelector,
        output_dir: String,
        #[serde(default)]
        overwrite: bool,
    },
    PressureSnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        selector: DaemonTargetSelector,
        output_dir: String,
        #[serde(default)]
        overwrite: bool,
    },
    TopologySnapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<DaemonTargetSelector>,
        output_dir: String,
        #[serde(default)]
        overwrite: bool,
    },
}

impl DaemonCapabilityInvocation {
    #[must_use]
    pub const fn capability_name(&self) -> DaemonCapabilityName {
        match self {
            Self::RolloutActivate { .. } | Self::RolloutStop { .. } => {
                DaemonCapabilityName::DeploymentControl
            }
            Self::PerfCapture { mode, .. } => match mode {
                PerfCollectionMode::Stat => DaemonCapabilityName::PerfStatCapture,
                PerfCollectionMode::Record => DaemonCapabilityName::PerfRecordCapture,
            },
            Self::SchedulerTraceCapture { .. } => DaemonCapabilityName::SchedulerTraceCapture,
            Self::SchedStateSnapshot { .. } => DaemonCapabilityName::SchedStateSnapshot,
            Self::PressureSnapshot { .. } => DaemonCapabilityName::PressureSnapshot,
            Self::TopologySnapshot { .. } => DaemonCapabilityName::TopologySnapshot,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedClawDaemonResponse {
    Status {
        snapshot: DaemonStatusSnapshot,
    },
    Capabilities {
        capabilities: Vec<DaemonCapabilityDescriptor>,
    },
    Logs {
        snapshot: DaemonLogsSnapshot,
    },
    Invocation {
        result: DaemonCapabilityResult,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonCapabilityResult {
    Rollout {
        message: String,
        snapshot: DaemonStatusSnapshot,
    },
    PerfCapture {
        snapshot: PerfCollectionSnapshot,
    },
    SchedulerTraceCapture {
        snapshot: SchedCollectionSnapshot,
    },
    SchedStateCapture {
        snapshot: SchedStateSnapshot,
    },
    PressureCapture {
        snapshot: PressureSnapshot,
    },
    TopologyCapture {
        snapshot: TopologySnapshot,
    },
}

impl DaemonCapabilityResult {
    #[must_use]
    pub const fn capability_name(&self) -> DaemonCapabilityName {
        match self {
            Self::Rollout { .. } => DaemonCapabilityName::DeploymentControl,
            Self::PerfCapture { snapshot } => match snapshot.mode {
                PerfCollectionMode::Stat => DaemonCapabilityName::PerfStatCapture,
                PerfCollectionMode::Record => DaemonCapabilityName::PerfRecordCapture,
            },
            Self::SchedulerTraceCapture { .. } => DaemonCapabilityName::SchedulerTraceCapture,
            Self::SchedStateCapture { .. } => DaemonCapabilityName::SchedStateSnapshot,
            Self::PressureCapture { .. } => DaemonCapabilityName::PressureSnapshot,
            Self::TopologyCapture { .. } => DaemonCapabilityName::TopologySnapshot,
        }
    }
}

#[must_use]
pub fn find_expected_daemon_capability(
    name: DaemonCapabilityName,
) -> Option<DaemonCapabilityDescriptor> {
    expected_daemon_capabilities()
        .into_iter()
        .find(|descriptor| descriptor.name == name)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct DaemonStatusSnapshot {
    pub daemon_pid: u32,
    pub workspace_root: String,
    pub socket_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_roots: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveDeploymentSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<DeploymentExitSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ActiveDeploymentSnapshot {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub pid: u32,
    pub started_at_unix_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_unix_ms: Option<u64>,
    pub log_line_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct DeploymentExitSnapshot {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub pid: u32,
    pub started_at_unix_s: u64,
    pub ended_at_unix_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub exit_reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_unix_ms: Option<u64>,
    pub log_line_count: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct DaemonLogsSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_label: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<DaemonLogLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct DaemonLogLine {
    pub source: String,
    pub emitted_at_unix_ms: u64,
    pub line: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PerfCollectionMode {
    Stat,
    Record,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum DaemonTargetSelector {
    Pid { pids: Vec<u32> },
    Uid { uid: u32 },
    Gid { gid: u32 },
    Cgroup { path: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PerfCallGraphMode {
    FramePointer,
    Dwarf,
    Lbr,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PerfCollectionSnapshot {
    pub label: String,
    pub mode: PerfCollectionMode,
    pub selector: DaemonTargetSelector,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_pids: Vec<u32>,
    pub requested_duration_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_frequency_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_graph: Option<PerfCallGraphMode>,
    pub output_dir: String,
    pub primary_output_path: String,
    pub command_path: String,
    pub selector_path: String,
    pub stdout_path: String,
    pub stderr_path: String,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: u64,
    pub stop_reason: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub perf_argv: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct SchedCollectionSnapshot {
    pub label: String,
    pub selector: DaemonTargetSelector,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_pids: Vec<u32>,
    pub requested_duration_ms: u64,
    pub output_dir: String,
    pub data_path: String,
    pub record_command_path: String,
    pub selector_path: String,
    pub record_stdout_path: String,
    pub record_stderr_path: String,
    pub timehist_path: String,
    pub timehist_command_path: String,
    pub timehist_stderr_path: String,
    pub latency_path: String,
    pub latency_command_path: String,
    pub latency_stderr_path: String,
    pub latency_by_pid: bool,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: u64,
    pub stop_reason: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub record_argv: Vec<String>,
    pub timehist_argv: Vec<String>,
    pub latency_argv: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct SchedStateSnapshot {
    pub label: String,
    pub selector: DaemonTargetSelector,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_pids: Vec<u32>,
    pub output_dir: String,
    pub global_schedstat_path: String,
    pub selector_path: String,
    pub index_path: String,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pid_artifacts: Vec<PidSchedStateArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PidSchedStateArtifact {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sched_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedstat_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PressureSnapshot {
    pub label: String,
    pub selector: DaemonTargetSelector,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_pids: Vec<u32>,
    pub output_dir: String,
    pub selector_path: String,
    pub index_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_cpu_pressure_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_io_pressure_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_memory_pressure_path: Option<String>,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pid_memberships: Vec<PidCgroupMembershipArtifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cgroup_artifacts: Vec<CgroupSnapshotArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PidCgroupMembershipArtifact {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup_membership_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_cgroup: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CgroupSnapshotArtifact {
    pub cgroup_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pressure_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_pressure_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_pressure_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_stat_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpuset_cpus_effective_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpuset_mems_effective_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TopologySnapshot {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<DaemonTargetSelector>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_pids: Vec<u32>,
    pub output_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_path: Option<String>,
    pub index_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_online_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_possible_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_present_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smt_active_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_online_path: Option<String>,
    pub topology_summary_path: String,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pid_contexts: Vec<PidTopologyContextArtifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cgroup_contexts: Vec<CgroupSnapshotArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PidTopologyContextArtifact {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup_membership_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_cgroup: Option<String>,
}
