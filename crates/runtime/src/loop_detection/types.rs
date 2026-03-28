use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct LoopDetectionConfig {
    pub enabled: bool,
    pub history_size: usize,
    pub warning_threshold: usize,
    pub critical_threshold: usize,
    pub global_circuit_breaker_threshold: usize,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            history_size: 24,
            warning_threshold: 4,
            critical_threshold: 6,
            global_circuit_breaker_threshold: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopSignalSeverity {
    Warning,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopSignal {
    pub severity: LoopSignalSeverity,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub(super) struct ToolLoopEntry {
    pub(super) pattern: String,
    pub(super) outcome: String,
}

pub(super) type ToolLoopHistory = VecDeque<ToolLoopEntry>;
