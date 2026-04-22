use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadContract {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_criteria: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostFingerprint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

impl HostFingerprint {
    #[must_use]
    pub fn capture() -> Self {
        Self {
            kernel_release: read_trimmed("/proc/sys/kernel/osrelease"),
            cpu_model: read_cpu_model(),
            arch: Some(std::env::consts::ARCH.to_string()),
        }
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_cpu_model() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    raw.lines()
        .find_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.trim() == "model name")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::HostFingerprint;

    #[test]
    fn host_fingerprint_always_reports_arch() {
        let fingerprint = HostFingerprint::capture();
        assert!(fingerprint.arch.is_some());
    }
}
