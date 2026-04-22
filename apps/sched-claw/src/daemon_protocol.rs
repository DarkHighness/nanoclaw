use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_STOP_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_LOG_TAIL_LINES: usize = 100;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SchedExtDaemonRequest {
    Status {},
    Activate {
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
    Stop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        graceful_timeout_ms: Option<u64>,
    },
    Logs {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail_lines: Option<usize>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedExtDaemonResponse {
    Status {
        snapshot: DaemonStatusSnapshot,
    },
    Logs {
        snapshot: DaemonLogsSnapshot,
    },
    Ack {
        message: String,
        snapshot: DaemonStatusSnapshot,
    },
    Error {
        message: String,
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
