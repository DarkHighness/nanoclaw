use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadContract {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<WorkloadTarget>,
    // Legacy script launch fields remain for backward-compatible manifest loads.
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkloadTarget {
    Script {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    Pid {
        pid: u32,
    },
    Uid {
        uid: u32,
    },
    Gid {
        gid: u32,
    },
    Cgroup {
        path: String,
    },
}

impl WorkloadContract {
    #[must_use]
    pub fn effective_target(&self) -> WorkloadTarget {
        self.target
            .clone()
            .unwrap_or_else(|| WorkloadTarget::Script {
                cwd: self.cwd.clone(),
                argv: self.argv.clone(),
                env: self.env.clone(),
            })
    }
}

impl WorkloadTarget {
    #[must_use]
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Script { .. } => "script",
            Self::Pid { .. } => "pid",
            Self::Uid { .. } => "uid",
            Self::Gid { .. } => "gid",
            Self::Cgroup { .. } => "cgroup",
        }
    }

    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Script { cwd, argv, env } => format!(
                "script cwd={} argv={} env={}",
                cwd.clone().unwrap_or_else(|| "<none>".to_string()),
                if argv.is_empty() {
                    "<none>".to_string()
                } else {
                    argv.join(" ")
                },
                if env.is_empty() {
                    "<none>".to_string()
                } else {
                    env.iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            Self::Pid { pid } => format!("pid={pid}"),
            Self::Uid { uid } => format!("uid={uid}"),
            Self::Gid { gid } => format!("gid={gid}"),
            Self::Cgroup { path } => format!("cgroup={path}"),
        }
    }
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
    use super::{HostFingerprint, WorkloadContract, WorkloadTarget};
    use std::collections::BTreeMap;

    #[test]
    fn host_fingerprint_always_reports_arch() {
        let fingerprint = HostFingerprint::capture();
        assert!(fingerprint.arch.is_some());
    }

    #[test]
    fn effective_target_defaults_to_legacy_script_fields() {
        let workload = WorkloadContract {
            name: "bench".to_string(),
            cwd: Some("/tmp".to_string()),
            argv: vec!["./run.sh".to_string()],
            env: BTreeMap::from([("MODE".to_string(), "smoke".to_string())]),
            ..Default::default()
        };
        assert_eq!(
            workload.effective_target(),
            WorkloadTarget::Script {
                cwd: Some("/tmp".to_string()),
                argv: vec!["./run.sh".to_string()],
                env: BTreeMap::from([("MODE".to_string(), "smoke".to_string())]),
            }
        );
    }

    #[test]
    fn target_summary_covers_cgroup() {
        let target = WorkloadTarget::Cgroup {
            path: "/sys/fs/cgroup/demo.slice".to_string(),
        };
        assert_eq!(target.kind_label(), "cgroup");
        assert!(target.summary().contains("demo.slice"));
    }
}
