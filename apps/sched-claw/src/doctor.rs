use crate::app_config::{SchedClawConfig, app_state_dir};
use crate::candidate_templates::template_specs;
use crate::daemon_client::SchedExtDaemonClient;
use agent_env::vars;
use anyhow::Result;
use nanoclaw_config::ProviderKind;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl DoctorStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorCheck {
    pub category: &'static str,
    pub name: &'static str,
    pub status: DoctorStatus,
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DoctorCounts {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorReport {
    pub workspace_root: PathBuf,
    pub app_state_dir: PathBuf,
    pub daemon_socket: PathBuf,
    pub provider: String,
    pub model_alias: String,
    pub model_name: String,
    pub template_count: usize,
    pub configured_skill_roots: Vec<PathBuf>,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    #[must_use]
    pub fn counts(&self) -> DoctorCounts {
        let mut counts = DoctorCounts::default();
        for check in &self.checks {
            match check.status {
                DoctorStatus::Pass => counts.pass += 1,
                DoctorStatus::Warn => counts.warn += 1,
                DoctorStatus::Fail => counts.fail += 1,
            }
        }
        counts
    }

    #[must_use]
    pub fn overall_status(&self) -> DoctorStatus {
        self.checks
            .iter()
            .map(|check| check.status)
            .max()
            .unwrap_or(DoctorStatus::Pass)
    }
}

pub async fn collect_doctor_report(
    workspace_root: &Path,
    config: &SchedClawConfig,
) -> Result<DoctorReport> {
    let mut checks = Vec::new();
    checks.push(provider_key_check(config));
    checks.push(skill_source_check(
        workspace_root.join("apps/sched-claw/skills"),
        true,
        "builtin sched-claw skills",
    ));
    checks.push(skill_source_check(
        workspace_root.join("apps/code-agent/skills"),
        false,
        "shared code-agent skills",
    ));
    checks.push(template_catalog_check());
    checks.push(daemon_check(config).await);
    checks.push(path_presence_check(
        "kernel",
        "BTF vmlinux",
        Path::new("/sys/kernel/btf/vmlinux"),
        true,
        "required for libbpf CO-RE verification on most hosts",
        Some("install or boot a kernel that exposes /sys/kernel/btf/vmlinux".to_string()),
    ));
    checks.push(path_presence_check(
        "kernel",
        "cgroup v2 controllers",
        Path::new("/sys/fs/cgroup/cgroup.controllers"),
        false,
        "used when workload contracts or sched-ext candidates target cgroups",
        Some("mount cgroup v2 or avoid cgroup-targeted experiments on this host".to_string()),
    ));

    let path_value = config.env_map.get_raw("PATH");
    checks.push(command_check(
        path_value,
        "toolchain",
        "clang",
        true,
        "required for sched-ext candidate builds",
        Some("install clang and keep it on PATH for experiment build runs".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "toolchain",
        "bpftool",
        true,
        "required for verifier probes and libbpf log capture",
        Some("install bpftool and keep it on PATH for experiment build runs".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "analysis",
        "perf",
        false,
        "used for scheduler triage and IPC/CPI proxy metrics",
        Some(
            "install linux perf tools if you need host-side profiling or proxy metrics".to_string(),
        ),
    ));
    checks.push(command_check(
        path_value,
        "demo",
        "cmake",
        false,
        "required by the LLVM/clang demo workload launcher",
        Some("install cmake to use the LLVM/clang autotune demo".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "demo",
        "ninja",
        false,
        "required by the LLVM/clang demo workload launcher",
        Some("install ninja to use the LLVM/clang autotune demo".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "demo",
        "sysbench",
        false,
        "required by the MySQL/sysbench demo workload launcher",
        Some("install sysbench to use the MySQL/sysbench autotune demo".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "demo",
        "docker",
        false,
        "default MySQL demo mode uses dockerized MySQL",
        Some("install docker or switch the MySQL demo launcher to --mode host".to_string()),
    ));
    checks.push(executable_file_check(
        "demo",
        "LLVM demo wrapper",
        workspace_root.join("apps/sched-claw/scripts/demos/llvm-clang-autotune.sh"),
        true,
    ));
    checks.push(executable_file_check(
        "demo",
        "LLVM workload launcher",
        workspace_root.join("apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh"),
        true,
    ));
    checks.push(executable_file_check(
        "demo",
        "MySQL demo wrapper",
        workspace_root.join("apps/sched-claw/scripts/demos/mysql-sysbench-autotune.sh"),
        true,
    ));
    checks.push(executable_file_check(
        "demo",
        "MySQL workload launcher",
        workspace_root.join("apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh"),
        true,
    ));

    Ok(DoctorReport {
        workspace_root: workspace_root.to_path_buf(),
        app_state_dir: app_state_dir(workspace_root),
        daemon_socket: config.daemon.socket_path.clone(),
        provider: provider_label(&config.primary_profile.model.provider).to_string(),
        model_alias: config.primary_profile.model.alias.clone(),
        model_name: config.primary_profile.model.model.clone(),
        template_count: template_specs().len(),
        configured_skill_roots: config.skill_roots.clone(),
        checks,
    })
}

fn provider_key_check(config: &SchedClawConfig) -> DoctorCheck {
    let (required_var, provider_label) = match config.primary_profile.model.provider {
        ProviderKind::OpenAi => (vars::OPENAI_API_KEY, "openai"),
        ProviderKind::Anthropic => (vars::ANTHROPIC_API_KEY, "anthropic"),
    };
    match config.env_map.get_non_empty_var(required_var) {
        Some(_) => DoctorCheck {
            category: "runtime",
            name: "selected provider credentials",
            status: DoctorStatus::Pass,
            detail: format!(
                "{} is configured for {}:{}",
                required_var.key, provider_label, config.primary_profile.model.model
            ),
            remediation: None,
        },
        None => DoctorCheck {
            category: "runtime",
            name: "selected provider credentials",
            status: DoctorStatus::Fail,
            detail: format!(
                "{} is missing for {}:{}",
                required_var.key, provider_label, config.primary_profile.model.model
            ),
            remediation: Some(format!(
                "set {} in the workspace root .env or the parent shell before running sched-claw exec",
                required_var.key
            )),
        },
    }
}

fn skill_source_check(root: PathBuf, required: bool, label: &'static str) -> DoctorCheck {
    let count = count_skills(&root);
    match count {
        Some(0) | None if required => DoctorCheck {
            category: "skills",
            name: label,
            status: DoctorStatus::Fail,
            detail: format!("{} is missing under {}", label, root.display()),
            remediation: Some(format!(
                "restore the repository skill bundle at {}",
                root.display()
            )),
        },
        Some(0) | None => DoctorCheck {
            category: "skills",
            name: label,
            status: DoctorStatus::Warn,
            detail: format!("{} is not available under {}", label, root.display()),
            remediation: Some(
                "keep apps/code-agent/skills available if you want the shared Linux perf SOP bundle"
                    .to_string(),
            ),
        },
        Some(count) => DoctorCheck {
            category: "skills",
            name: label,
            status: DoctorStatus::Pass,
            detail: format!("{} skill directories found under {}", count, root.display()),
            remediation: None,
        },
    }
}

fn template_catalog_check() -> DoctorCheck {
    let count = template_specs().len();
    if count == 0 {
        DoctorCheck {
            category: "templates",
            name: "sched-ext template catalog",
            status: DoctorStatus::Fail,
            detail: "no local sched-ext templates are registered".to_string(),
            remediation: Some(
                "restore template_specs() entries before running candidate materialization"
                    .to_string(),
            ),
        }
    } else {
        DoctorCheck {
            category: "templates",
            name: "sched-ext template catalog",
            status: DoctorStatus::Pass,
            detail: format!("{count} sched-ext templates are available"),
            remediation: None,
        }
    }
}

async fn daemon_check(config: &SchedClawConfig) -> DoctorCheck {
    if !config.daemon.socket_path.exists() {
        return DoctorCheck {
            category: "daemon",
            name: "privileged sched-ext daemon",
            status: DoctorStatus::Fail,
            detail: format!(
                "daemon socket does not exist at {}",
                config.daemon.socket_path.display()
            ),
            remediation: Some(
                "start apps/sched-claw/scripts/start-root-daemon.sh or launch sched-claw-daemon manually"
                    .to_string(),
            ),
        };
    }

    let client = SchedExtDaemonClient::new(config.daemon.clone());
    match client.status().await {
        Ok(snapshot) => DoctorCheck {
            category: "daemon",
            name: "privileged sched-ext daemon",
            status: DoctorStatus::Pass,
            detail: format!(
                "reachable at {} (daemon_pid={}, active={})",
                config.daemon.socket_path.display(),
                snapshot.daemon_pid,
                if snapshot.active.is_some() { "yes" } else { "no" }
            ),
            remediation: None,
        },
        Err(error) => DoctorCheck {
            category: "daemon",
            name: "privileged sched-ext daemon",
            status: DoctorStatus::Fail,
            detail: format!(
                "socket exists at {} but status failed: {error}",
                config.daemon.socket_path.display()
            ),
            remediation: Some(
                "restart the root daemon and verify that the non-root client can connect to the socket"
                    .to_string(),
            ),
        },
    }
}

fn path_presence_check(
    category: &'static str,
    name: &'static str,
    path: &Path,
    required: bool,
    success_detail: &'static str,
    remediation: Option<String>,
) -> DoctorCheck {
    if path.exists() {
        DoctorCheck {
            category,
            name,
            status: DoctorStatus::Pass,
            detail: format!("present at {} ({success_detail})", path.display()),
            remediation: None,
        }
    } else {
        DoctorCheck {
            category,
            name,
            status: if required {
                DoctorStatus::Fail
            } else {
                DoctorStatus::Warn
            },
            detail: format!("missing at {}", path.display()),
            remediation,
        }
    }
}

fn command_check(
    path_env: Option<&str>,
    category: &'static str,
    command: &'static str,
    required: bool,
    success_detail: &'static str,
    remediation: Option<String>,
) -> DoctorCheck {
    match find_command_on_path(command, path_env) {
        Some(path) => DoctorCheck {
            category,
            name: command,
            status: DoctorStatus::Pass,
            detail: format!("found at {} ({success_detail})", path.display()),
            remediation: None,
        },
        None => DoctorCheck {
            category,
            name: command,
            status: if required {
                DoctorStatus::Fail
            } else {
                DoctorStatus::Warn
            },
            detail: format!("{command} is not available on PATH"),
            remediation,
        },
    }
}

fn executable_file_check(
    category: &'static str,
    name: &'static str,
    path: PathBuf,
    required: bool,
) -> DoctorCheck {
    if is_executable_file(&path) {
        DoctorCheck {
            category,
            name,
            status: DoctorStatus::Pass,
            detail: format!("ready at {}", path.display()),
            remediation: None,
        }
    } else {
        DoctorCheck {
            category,
            name,
            status: if required {
                DoctorStatus::Fail
            } else {
                DoctorStatus::Warn
            },
            detail: format!("missing or not executable at {}", path.display()),
            remediation: Some(format!(
                "restore executable permissions on {}",
                path.display()
            )),
        }
    }
}

fn count_skills(root: &Path) -> Option<usize> {
    let entries = fs::read_dir(root).ok()?;
    let count = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .count();
    Some(count)
}

fn provider_label(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    }
}

fn find_command_on_path(command: &str, path_env: Option<&str>) -> Option<PathBuf> {
    let candidate = PathBuf::from(command);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return is_executable_file(&candidate).then_some(candidate);
    }

    let path_env = path_env?;
    std::env::split_paths(path_env)
        .map(|dir| dir.join(command))
        .find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    is_executable_mode(&metadata)
}

#[cfg(unix)]
fn is_executable_mode(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_mode(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{
        DoctorCheck, DoctorReport, DoctorStatus, count_skills, find_command_on_path,
        is_executable_file,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn counts_skill_directories_with_skill_markdown() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("one")).unwrap();
        fs::create_dir_all(dir.path().join("two")).unwrap();
        fs::write(dir.path().join("one/SKILL.md"), "body").unwrap();
        fs::write(dir.path().join("two/SKILL.md"), "body").unwrap();

        assert_eq!(count_skills(dir.path()), Some(2));
    }

    #[test]
    fn finds_command_on_custom_path() {
        let dir = tempdir().unwrap();
        let tool = dir.path().join("mock-tool");
        fs::write(&tool, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&tool).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&tool, permissions).unwrap();
        }

        assert_eq!(
            find_command_on_path("mock-tool", Some(dir.path().to_str().unwrap())),
            Some(tool)
        );
    }

    #[test]
    fn report_counts_and_overall_status_follow_check_severity() {
        let report = DoctorReport {
            workspace_root: "/repo".into(),
            app_state_dir: "/repo/.nanoclaw/apps/sched-claw".into(),
            daemon_socket: "/repo/.nanoclaw/apps/sched-claw/sched-claw.sock".into(),
            provider: "openai".to_string(),
            model_alias: "gpt_5_4_default".to_string(),
            model_name: "gpt-5.4".to_string(),
            template_count: 4,
            configured_skill_roots: Vec::new(),
            checks: vec![
                DoctorCheck {
                    category: "runtime",
                    name: "provider",
                    status: DoctorStatus::Pass,
                    detail: "ok".to_string(),
                    remediation: None,
                },
                DoctorCheck {
                    category: "daemon",
                    name: "daemon",
                    status: DoctorStatus::Warn,
                    detail: "warn".to_string(),
                    remediation: None,
                },
                DoctorCheck {
                    category: "toolchain",
                    name: "clang",
                    status: DoctorStatus::Fail,
                    detail: "missing".to_string(),
                    remediation: None,
                },
            ],
        };

        assert_eq!(report.counts().pass, 1);
        assert_eq!(report.counts().warn, 1);
        assert_eq!(report.counts().fail, 1);
        assert_eq!(report.overall_status(), DoctorStatus::Fail);
    }

    #[test]
    fn executable_file_check_uses_mode_bits() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("plain.txt");
        fs::write(&file, "content").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&file).unwrap().permissions();
            permissions.set_mode(0o644);
            fs::set_permissions(&file, permissions).unwrap();
            assert!(!is_executable_file(&file));
        }
    }
}
