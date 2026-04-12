use agent::types::{MonitorId, MonitorStatus, TaskId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMonitorSummary {
    pub monitor_id: MonitorId,
    pub task_id: Option<TaskId>,
    pub status: MonitorStatus,
    pub command: String,
    pub cwd: String,
    pub shell: String,
    pub login: bool,
    pub started_at_unix_s: u64,
    pub finished_at_unix_s: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveMonitorControlAction {
    Stopped,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMonitorControlOutcome {
    pub requested_ref: String,
    pub action: LiveMonitorControlAction,
    pub monitor: LiveMonitorSummary,
}
