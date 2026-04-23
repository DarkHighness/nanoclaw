use crate::daemon_protocol::{
    DaemonCapabilityDescriptor, DaemonCapabilityName, DaemonSelectorKind,
    find_expected_daemon_capability,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonInspectionTarget {
    Projection(DaemonProjectionName),
    Capability(DaemonCapabilityName),
}

impl DaemonInspectionTarget {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Projection(name) => name.as_str(),
            Self::Capability(name) => name.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DaemonProjectionName {
    Status,
    Capabilities,
    Logs,
    Activate,
    Stop,
    CollectPerf,
    CollectSched,
    CollectState,
    CollectPressure,
    CollectTopology,
}

impl DaemonProjectionName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Capabilities => "capabilities",
            Self::Logs => "logs",
            Self::Activate => "activate",
            Self::Stop => "stop",
            Self::CollectPerf => "collect-perf",
            Self::CollectSched => "collect-sched",
            Self::CollectState => "collect-state",
            Self::CollectPressure => "collect-pressure",
            Self::CollectTopology => "collect-topology",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonProjectionKind {
    Discovery,
    Invocation,
}

impl DaemonProjectionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Invocation => "invocation",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonProjectionDescriptor {
    pub name: DaemonProjectionName,
    pub kind: DaemonProjectionKind,
    pub summary: &'static str,
    pub capabilities: Vec<DaemonCapabilityName>,
    pub selectors: Vec<DaemonSelectorKind>,
    pub examples: Vec<&'static str>,
}

#[must_use]
pub fn expected_daemon_projections() -> Vec<DaemonProjectionDescriptor> {
    vec![
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::Status,
            kind: DaemonProjectionKind::Discovery,
            summary: "Read the current daemon status snapshot without mutating the active rollout.",
            capabilities: Vec::new(),
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon status --style table"],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::Capabilities,
            kind: DaemonProjectionKind::Discovery,
            summary: "Read the daemon advertised capability catalog from the live root daemon.",
            capabilities: Vec::new(),
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon capabilities --style plain"],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::Logs,
            kind: DaemonProjectionKind::Discovery,
            summary: "Tail the active or last deployment logs from the privileged daemon.",
            capabilities: Vec::new(),
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon logs --tail-lines 50 --style plain"],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::Activate,
            kind: DaemonProjectionKind::Invocation,
            summary: "Start a bounded sched-ext rollout through the deployment-control capability.",
            capabilities: vec![DaemonCapabilityName::DeploymentControl],
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon activate --lease-seconds 30 loader --flag"],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::Stop,
            kind: DaemonProjectionKind::Invocation,
            summary: "Stop the active bounded sched-ext rollout through the deployment-control capability.",
            capabilities: vec![DaemonCapabilityName::DeploymentControl],
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon stop --graceful-timeout-ms 2000"],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::CollectPerf,
            kind: DaemonProjectionKind::Invocation,
            summary: "Run bounded perf stat or perf record capture through the structured perf capabilities.",
            capabilities: vec![
                DaemonCapabilityName::PerfStatCapture,
                DaemonCapabilityName::PerfRecordCapture,
            ],
            selectors: selectors_for_capabilities(&[
                DaemonCapabilityName::PerfStatCapture,
                DaemonCapabilityName::PerfRecordCapture,
            ]),
            examples: vec![
                "sched-claw daemon collect-perf --pid 4242 --duration-ms 1000 --output-dir artifacts/perf-a",
                "sched-claw daemon collect-perf --mode record --cgroup work.slice --duration-ms 1000 --output-dir artifacts/perf-record",
            ],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::CollectSched,
            kind: DaemonProjectionKind::Invocation,
            summary: "Run bounded scheduler trace capture through the scheduler-trace capability.",
            capabilities: vec![DaemonCapabilityName::SchedulerTraceCapture],
            selectors: selectors_for_capabilities(&[DaemonCapabilityName::SchedulerTraceCapture]),
            examples: vec![
                "sched-claw daemon collect-sched --pid 4242 --duration-ms 1000 --output-dir artifacts/sched-a",
            ],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::CollectState,
            kind: DaemonProjectionKind::Invocation,
            summary: "Capture read-only procfs scheduler state for an explicit pid, uid, gid, or cgroup selector.",
            capabilities: vec![DaemonCapabilityName::SchedStateSnapshot],
            selectors: selectors_for_capabilities(&[DaemonCapabilityName::SchedStateSnapshot]),
            examples: vec![
                "sched-claw daemon collect-state --pid 4242 --output-dir artifacts/state-a",
            ],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::CollectPressure,
            kind: DaemonProjectionKind::Invocation,
            summary: "Capture read-only PSI and cgroup pressure context for an explicit pid, uid, gid, or cgroup selector.",
            capabilities: vec![DaemonCapabilityName::PressureSnapshot],
            selectors: selectors_for_capabilities(&[DaemonCapabilityName::PressureSnapshot]),
            examples: vec![
                "sched-claw daemon collect-pressure --cgroup work.slice --output-dir artifacts/pressure-a",
            ],
        },
        DaemonProjectionDescriptor {
            name: DaemonProjectionName::CollectTopology,
            kind: DaemonProjectionKind::Invocation,
            summary: "Capture read-only CPU, NUMA, SMT, and selector-scoped cpuset topology context.",
            capabilities: vec![DaemonCapabilityName::TopologySnapshot],
            selectors: selectors_for_capabilities(&[DaemonCapabilityName::TopologySnapshot]),
            examples: vec![
                "sched-claw daemon collect-topology --output-dir artifacts/topology-a",
                "sched-claw daemon collect-topology --pid 4242 --output-dir artifacts/topology-a",
            ],
        },
    ]
}

#[must_use]
pub fn find_expected_daemon_projection(
    name: DaemonProjectionName,
) -> Option<DaemonProjectionDescriptor> {
    expected_daemon_projections()
        .into_iter()
        .find(|projection| projection.name == name)
}

#[must_use]
pub fn parse_daemon_projection_name(query: &str) -> Option<DaemonProjectionName> {
    match query.trim() {
        "status" => Some(DaemonProjectionName::Status),
        "capabilities" => Some(DaemonProjectionName::Capabilities),
        "logs" => Some(DaemonProjectionName::Logs),
        "activate" => Some(DaemonProjectionName::Activate),
        "stop" => Some(DaemonProjectionName::Stop),
        "collect-perf" => Some(DaemonProjectionName::CollectPerf),
        "collect-sched" => Some(DaemonProjectionName::CollectSched),
        "collect-state" => Some(DaemonProjectionName::CollectState),
        "collect-pressure" => Some(DaemonProjectionName::CollectPressure),
        "collect-topology" => Some(DaemonProjectionName::CollectTopology),
        _ => None,
    }
}

#[must_use]
pub fn parse_daemon_inspection_target(query: &str) -> Option<DaemonInspectionTarget> {
    if let Some(name) = parse_daemon_projection_name(query) {
        return Some(DaemonInspectionTarget::Projection(name));
    }
    match query.trim() {
        "deployment_control" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::DeploymentControl,
        )),
        "perf_stat_capture" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::PerfStatCapture,
        )),
        "perf_record_capture" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::PerfRecordCapture,
        )),
        "scheduler_trace_capture" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::SchedulerTraceCapture,
        )),
        "sched_state_snapshot" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::SchedStateSnapshot,
        )),
        "pressure_snapshot" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::PressureSnapshot,
        )),
        "topology_snapshot" => Some(DaemonInspectionTarget::Capability(
            DaemonCapabilityName::TopologySnapshot,
        )),
        _ => None,
    }
}

fn selectors_for_capabilities(capabilities: &[DaemonCapabilityName]) -> Vec<DaemonSelectorKind> {
    let mut selectors = Vec::new();
    for capability in capabilities {
        if let Some(descriptor) = find_expected_daemon_capability(*capability) {
            for selector in descriptor.selector_kinds {
                if !selectors.contains(&selector) {
                    selectors.push(selector);
                }
            }
        }
    }
    selectors
}

#[must_use]
pub fn find_projection_capabilities(
    projection: &DaemonProjectionDescriptor,
) -> Vec<DaemonCapabilityDescriptor> {
    projection
        .capabilities
        .iter()
        .filter_map(|name| find_expected_daemon_capability(*name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonInspectionTarget, DaemonProjectionKind, DaemonProjectionName,
        expected_daemon_projections, find_expected_daemon_projection, find_projection_capabilities,
        parse_daemon_inspection_target, parse_daemon_projection_name,
    };
    use crate::daemon_protocol::DaemonCapabilityName;

    #[test]
    fn projection_catalog_contains_collect_perf_mapping() {
        let projection = find_expected_daemon_projection(DaemonProjectionName::CollectPerf)
            .expect("collect-perf projection");
        assert_eq!(projection.kind, DaemonProjectionKind::Invocation);
        assert_eq!(
            projection.capabilities,
            vec![
                DaemonCapabilityName::PerfStatCapture,
                DaemonCapabilityName::PerfRecordCapture,
            ]
        );
        assert!(projection.selectors.len() >= 4);
    }

    #[test]
    fn projection_capabilities_expand_from_catalog() {
        let projection = find_expected_daemon_projection(DaemonProjectionName::CollectSched)
            .expect("collect-sched projection");
        let capabilities = find_projection_capabilities(&projection);
        assert_eq!(capabilities.len(), 1);
        assert_eq!(
            capabilities[0].name,
            DaemonCapabilityName::SchedulerTraceCapture
        );
    }

    #[test]
    fn parses_projection_names() {
        assert_eq!(
            parse_daemon_projection_name("collect-perf"),
            Some(DaemonProjectionName::CollectPerf)
        );
        assert_eq!(
            parse_daemon_projection_name("collect-topology"),
            Some(DaemonProjectionName::CollectTopology)
        );
        assert_eq!(parse_daemon_projection_name("unknown"), None);
    }

    #[test]
    fn parses_inspection_targets() {
        assert_eq!(
            parse_daemon_inspection_target("activate"),
            Some(DaemonInspectionTarget::Projection(
                DaemonProjectionName::Activate
            ))
        );
        assert_eq!(
            parse_daemon_inspection_target("perf_record_capture"),
            Some(DaemonInspectionTarget::Capability(
                DaemonCapabilityName::PerfRecordCapture
            ))
        );
        assert_eq!(
            parse_daemon_inspection_target("pressure_snapshot"),
            Some(DaemonInspectionTarget::Capability(
                DaemonCapabilityName::PressureSnapshot
            ))
        );
        assert_eq!(parse_daemon_inspection_target("unknown"), None);
    }

    #[test]
    fn projection_catalog_is_stable_and_small() {
        let projections = expected_daemon_projections();
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == DaemonProjectionName::Status)
        );
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == DaemonProjectionName::Activate)
        );
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == DaemonProjectionName::CollectPerf)
        );
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == DaemonProjectionName::CollectTopology)
        );
    }
}
