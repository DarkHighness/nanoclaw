use crate::daemon_protocol::{
    DaemonCapabilityDescriptor, DaemonCapabilityName, DaemonSelectorKind,
    find_expected_daemon_capability,
};

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
    pub name: &'static str,
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
            name: "status",
            kind: DaemonProjectionKind::Discovery,
            summary: "Read the current daemon status snapshot without mutating the active rollout.",
            capabilities: Vec::new(),
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon status --style table"],
        },
        DaemonProjectionDescriptor {
            name: "capabilities",
            kind: DaemonProjectionKind::Discovery,
            summary: "Read the daemon advertised capability catalog from the live root daemon.",
            capabilities: Vec::new(),
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon capabilities --style plain"],
        },
        DaemonProjectionDescriptor {
            name: "logs",
            kind: DaemonProjectionKind::Discovery,
            summary: "Tail the active or last deployment logs from the privileged daemon.",
            capabilities: Vec::new(),
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon logs --tail-lines 50 --style plain"],
        },
        DaemonProjectionDescriptor {
            name: "activate",
            kind: DaemonProjectionKind::Invocation,
            summary: "Start a bounded sched-ext rollout through the deployment-control capability.",
            capabilities: vec![DaemonCapabilityName::DeploymentControl],
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon activate --lease-seconds 30 loader --flag"],
        },
        DaemonProjectionDescriptor {
            name: "stop",
            kind: DaemonProjectionKind::Invocation,
            summary: "Stop the active bounded sched-ext rollout through the deployment-control capability.",
            capabilities: vec![DaemonCapabilityName::DeploymentControl],
            selectors: Vec::new(),
            examples: vec!["sched-claw daemon stop --graceful-timeout-ms 2000"],
        },
        DaemonProjectionDescriptor {
            name: "collect-perf",
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
            name: "collect-sched",
            kind: DaemonProjectionKind::Invocation,
            summary: "Run bounded scheduler trace capture through the scheduler-trace capability.",
            capabilities: vec![DaemonCapabilityName::SchedulerTraceCapture],
            selectors: selectors_for_capabilities(&[DaemonCapabilityName::SchedulerTraceCapture]),
            examples: vec![
                "sched-claw daemon collect-sched --pid 4242 --duration-ms 1000 --output-dir artifacts/sched-a",
            ],
        },
    ]
}

#[must_use]
pub fn find_daemon_projection(query: &str) -> Option<DaemonProjectionDescriptor> {
    let normalized = query.trim();
    if normalized.is_empty() {
        return None;
    }
    expected_daemon_projections()
        .into_iter()
        .find(|projection| projection.name == normalized)
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
        DaemonProjectionKind, expected_daemon_projections, find_daemon_projection,
        find_projection_capabilities,
    };
    use crate::daemon_protocol::DaemonCapabilityName;

    #[test]
    fn projection_catalog_contains_collect_perf_mapping() {
        let projection = find_daemon_projection("collect-perf").expect("collect-perf projection");
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
        let projection = find_daemon_projection("collect-sched").expect("collect-sched projection");
        let capabilities = find_projection_capabilities(&projection);
        assert_eq!(capabilities.len(), 1);
        assert_eq!(
            capabilities[0].name,
            DaemonCapabilityName::SchedulerTraceCapture
        );
    }

    #[test]
    fn projection_catalog_is_stable_and_small() {
        let projections = expected_daemon_projections();
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == "status")
        );
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == "activate")
        );
        assert!(
            projections
                .iter()
                .any(|projection| projection.name == "collect-perf")
        );
    }
}
