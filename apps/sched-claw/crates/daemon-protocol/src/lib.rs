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
    pub name: String,
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
            name: "deployment_control".to_string(),
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
            name: "perf_stat_capture".to_string(),
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
            name: "perf_record_capture".to_string(),
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
            name: "scheduler_trace_capture".to_string(),
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
        selector: PerfTargetSelector,
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
        selector: PerfTargetSelector,
        output_dir: String,
        duration_ms: u64,
        #[serde(default)]
        latency_by_pid: bool,
        #[serde(default)]
        overwrite: bool,
    },
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
pub enum PerfTargetSelector {
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
    pub selector: PerfTargetSelector,
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
    pub selector: PerfTargetSelector,
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
