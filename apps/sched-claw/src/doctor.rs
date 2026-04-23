use crate::app_config::{SchedClawConfig, app_state_dir};
use crate::daemon_client::SchedExtDaemonClient;
use agent_env::vars;
use anyhow::Result;
use nanoclaw_config::ProviderKind;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

const RECOMMENDED_KERNEL_MAJOR: u64 = 6;
const RECOMMENDED_KERNEL_MINOR: u64 = 12;

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
    pub helper_script_count: usize,
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

#[derive(Clone, Debug)]
struct LoadedKernelConfig {
    source_path: PathBuf,
    contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct KernelRelease {
    raw: String,
    major: u64,
    minor: u64,
    patch: u64,
}

pub async fn collect_doctor_report(
    workspace_root: &Path,
    config: &SchedClawConfig,
) -> Result<DoctorReport> {
    let mut checks = Vec::new();
    let path_value = config.env_map.get_raw("PATH");
    let kernel_release = detect_kernel_release();
    let kernel_config = load_kernel_config(kernel_release.as_ref().map(|value| value.raw.as_str()));

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
    checks.push(helper_script_check(
        workspace_root.join("apps/sched-claw/skills/sched-perf-collection/scripts/collect_perf.sh"),
        "collection",
        "perf collection helper",
        true,
        "used by scheduler evidence collection skills for reproducible perf capture",
    ));
    checks.push(helper_script_check(
        workspace_root
            .join("apps/sched-claw/skills/sched-perf-analysis/scripts/bootstrap_uv_env.sh"),
        "analysis",
        "uv analysis bootstrap",
        true,
        "used to prepare a reproducible Python analysis environment",
    ));
    checks.push(helper_script_check(
        workspace_root
            .join("apps/sched-claw/skills/sched-perf-analysis/scripts/analyze_perf_csv.py"),
        "analysis",
        "perf csv analysis helper",
        true,
        "used for pandas or polars summaries and matplotlib plots",
    ));
    checks.push(helper_script_check(
        workspace_root
            .join("apps/sched-claw/skills/sched-perf-analysis/scripts/render_perf_report.sh"),
        "analysis",
        "perf report rendering helper",
        false,
        "used to turn perf.data captures into perf report or perf script artifacts",
    ));
    checks.push(helper_script_check(
        workspace_root.join(
            "apps/sched-claw/skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh",
        ),
        "codegen",
        "sched-ext code scaffold helper",
        false,
        "used to seed candidate directories and build stubs without host-owned workflow code",
    ));
    checks.push(daemon_check(config).await);
    checks.push(kernel_release_check(kernel_release.as_ref()));
    checks.push(kernel_config_source_check(kernel_config.as_ref()));
    checks.push(kernel_config_option_check(kernel_config.as_ref()));
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
        Some("mount cgroup v2 or avoid cgroup-targeted captures on this host".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "toolchain",
        "clang",
        true,
        "required for sched-ext candidate builds",
        Some("install clang and keep it on PATH for sched-ext candidate builds".to_string()),
    ));
    checks.push(command_check(
        path_value,
        "toolchain",
        "bpftool",
        true,
        "required for verifier probes and libbpf log capture",
        Some(
            "install bpftool and keep it on PATH for verifier probes and candidate builds"
                .to_string(),
        ),
    ));
    checks.push(command_check(
        path_value,
        "analysis",
        "uv",
        false,
        "used by skill helper scripts to provision pandas, polars, and matplotlib environments",
        Some(
            "install uv if you want the built-in analysis environment bootstrap helpers"
                .to_string(),
        ),
    ));
    checks.push(command_check(
        path_value,
        "analysis",
        "python3",
        false,
        "used by skill helper scripts for metrics analysis and plotting",
        Some("install python3 if you want the built-in analysis helper scripts".to_string()),
    ));
    checks.push(uv_bootstrap_check(path_value, workspace_root));
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
    checks.push(perf_event_paranoid_check());

    Ok(DoctorReport {
        workspace_root: workspace_root.to_path_buf(),
        app_state_dir: app_state_dir(workspace_root),
        daemon_socket: config.daemon.socket_path.clone(),
        provider: provider_label(&config.primary_profile.model.provider).to_string(),
        model_alias: config.primary_profile.model.alias.clone(),
        model_name: config.primary_profile.model.model.clone(),
        helper_script_count: count_helper_scripts(&workspace_root.join("apps/sched-claw/skills")),
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

fn helper_script_check(
    path: PathBuf,
    category: &'static str,
    name: &'static str,
    required: bool,
    detail: &'static str,
) -> DoctorCheck {
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => DoctorCheck {
            category,
            name,
            status: DoctorStatus::Pass,
            detail: format!("{detail}: {}", path.display()),
            remediation: None,
        },
        _ if required => DoctorCheck {
            category,
            name,
            status: DoctorStatus::Fail,
            detail: format!("required helper is missing: {}", path.display()),
            remediation: Some(format!("restore {}", path.display())),
        },
        _ => DoctorCheck {
            category,
            name,
            status: DoctorStatus::Warn,
            detail: format!("optional helper is missing: {}", path.display()),
            remediation: Some(format!(
                "restore {} if you want that helper flow",
                path.display()
            )),
        },
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

fn kernel_release_check(release: Option<&KernelRelease>) -> DoctorCheck {
    let Some(release) = release else {
        return DoctorCheck {
            category: "kernel",
            name: "kernel release",
            status: DoctorStatus::Warn,
            detail: "failed to detect kernel release via uname -r".to_string(),
            remediation: Some(
                "verify that the host exposes uname -r and record the tested kernel version"
                    .to_string(),
            ),
        };
    };

    let meets_baseline =
        (release.major, release.minor) >= (RECOMMENDED_KERNEL_MAJOR, RECOMMENDED_KERNEL_MINOR);
    DoctorCheck {
        category: "kernel",
        name: "kernel release",
        status: if meets_baseline {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Warn
        },
        detail: format!(
            "{} ({}) the harness baseline {}.{}+",
            release.raw,
            if meets_baseline {
                "meets"
            } else {
                "is below"
            },
            RECOMMENDED_KERNEL_MAJOR,
            RECOMMENDED_KERNEL_MINOR
        ),
        remediation: (!meets_baseline).then_some(
            "use a newer kernel or confirm that your distro backports the required sched-ext surface before rollout"
                .to_string(),
        ),
    }
}

fn kernel_config_source_check(config: Option<&LoadedKernelConfig>) -> DoctorCheck {
    match config {
        Some(config) => DoctorCheck {
            category: "kernel",
            name: "kernel config source",
            status: DoctorStatus::Pass,
            detail: format!("loaded from {}", config.source_path.display()),
            remediation: None,
        },
        None => DoctorCheck {
            category: "kernel",
            name: "kernel config source",
            status: DoctorStatus::Warn,
            detail: "could not load /boot/config-<release> or /proc/config.gz".to_string(),
            remediation: Some(
                "install a readable kernel config or expose /proc/config.gz so doctor can verify sched-ext prerequisites"
                    .to_string(),
            ),
        },
    }
}

fn kernel_config_option_check(config: Option<&LoadedKernelConfig>) -> DoctorCheck {
    let Some(config) = config else {
        return DoctorCheck {
            category: "kernel",
            name: "kernel config prerequisites",
            status: DoctorStatus::Warn,
            detail: "kernel config not readable; sched-ext prerequisites could not be verified"
                .to_string(),
            remediation: Some(
                "make the current kernel config readable and rerun sched-claw doctor".to_string(),
            ),
        };
    };

    let required = [
        ("CONFIG_BPF", "y"),
        ("CONFIG_BPF_SYSCALL", "y"),
        ("CONFIG_DEBUG_INFO_BTF", "y"),
        ("CONFIG_CGROUPS", "y"),
        ("CONFIG_CGROUP_SCHED", "y"),
        ("CONFIG_SCHED_CLASS_EXT", "y"),
    ];

    let mut failures = Vec::new();
    let mut observed = Vec::new();
    for (key, expected) in required {
        let value = kernel_config_value(&config.contents, key).unwrap_or("<missing>");
        observed.push(format!("{key}={value}"));
        if value != expected {
            failures.push(format!("{key} expected {expected} got {value}"));
        }
    }

    DoctorCheck {
        category: "kernel",
        name: "kernel config prerequisites",
        status: if failures.is_empty() {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        detail: if failures.is_empty() {
            observed.join(", ")
        } else {
            format!("{}; observed {}", failures.join("; "), observed.join(", "))
        },
        remediation: (!failures.is_empty()).then_some(
            "use a kernel build with sched-ext, BPF, BTF, and cgroup scheduling enabled"
                .to_string(),
        ),
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

fn uv_bootstrap_check(path_env: Option<&str>, workspace_root: &Path) -> DoctorCheck {
    let Some(uv_path) = find_command_on_path("uv", path_env) else {
        return DoctorCheck {
            category: "analysis",
            name: "uv helper compatibility",
            status: DoctorStatus::Fail,
            detail: "uv is unavailable, so the analysis helper environment cannot be provisioned"
                .to_string(),
            remediation: Some(
                "install uv and rerun sched-claw doctor before using pandas, polars, or matplotlib helpers"
                    .to_string(),
            ),
        };
    };
    let Some(python_path) = find_command_on_path("python3", path_env) else {
        return DoctorCheck {
            category: "analysis",
            name: "uv helper compatibility",
            status: DoctorStatus::Fail,
            detail:
                "python3 is unavailable, so the uv-managed analysis environment cannot be created"
                    .to_string(),
            remediation: Some(
                "install python3 and rerun sched-claw doctor before using analysis helpers"
                    .to_string(),
            ),
        };
    };

    let requirements =
        workspace_root.join("apps/sched-claw/skills/sched-perf-analysis/scripts/requirements.txt");
    if !requirements.is_file() {
        return DoctorCheck {
            category: "analysis",
            name: "uv helper compatibility",
            status: DoctorStatus::Fail,
            detail: format!(
                "analysis requirements file is missing at {}",
                requirements.display()
            ),
            remediation: Some(format!("restore {}", requirements.display())),
        };
    }

    let probe_root = std::env::temp_dir().join(format!(
        "sched-claw-doctor-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default()
    ));
    let venv_dir = probe_root.join(".venv");
    let python_in_venv = venv_dir.join("bin/python");
    let uv_version =
        command_version_line(&uv_path, &["--version"]).unwrap_or_else(|| "uv unknown".to_string());
    let python_version = command_version_line(&python_path, &["--version"])
        .unwrap_or_else(|| "python3 unknown".to_string());

    let venv_output = ProcessCommand::new(&uv_path)
        .args(["venv", "--python"])
        .arg(&python_path)
        .arg(&venv_dir)
        .output();

    let check = match venv_output {
        Ok(output) if output.status.success() => {
            let dry_run = ProcessCommand::new(&uv_path)
                .arg("pip")
                .arg("install")
                .arg("--dry-run")
                .arg("--strict")
                .arg("--python")
                .arg(&python_in_venv)
                .arg("-r")
                .arg(&requirements)
                .output();
            match dry_run {
                Ok(output) if output.status.success() => DoctorCheck {
                    category: "analysis",
                    name: "uv helper compatibility",
                    status: DoctorStatus::Pass,
                    detail: format!(
                        "{} + {} resolved {}",
                        uv_version,
                        python_version,
                        requirements.display()
                    ),
                    remediation: None,
                },
                Ok(output) => DoctorCheck {
                    category: "analysis",
                    name: "uv helper compatibility",
                    status: DoctorStatus::Fail,
                    detail: format!(
                        "uv could create a venv but failed to resolve helper requirements: {}",
                        summarize_output(&output.stdout, &output.stderr)
                    ),
                    remediation: Some(
                        "run bootstrap_uv_env.sh manually and fix Python or package resolution issues before using analysis helpers"
                            .to_string(),
                    ),
                },
                Err(error) => DoctorCheck {
                    category: "analysis",
                    name: "uv helper compatibility",
                    status: DoctorStatus::Fail,
                    detail: format!("failed to run uv dry-run dependency probe: {error}"),
                    remediation: Some(
                        "verify that uv can execute pip install --dry-run for the analysis requirements"
                            .to_string(),
                    ),
                },
            }
        }
        Ok(output) => DoctorCheck {
            category: "analysis",
            name: "uv helper compatibility",
            status: DoctorStatus::Fail,
            detail: format!(
                "uv failed to create a virtual environment: {}",
                summarize_output(&output.stdout, &output.stderr)
            ),
            remediation: Some(
                "verify uv, python3, and filesystem permissions before using the analysis helper scripts"
                    .to_string(),
            ),
        },
        Err(error) => DoctorCheck {
            category: "analysis",
            name: "uv helper compatibility",
            status: DoctorStatus::Fail,
            detail: format!("failed to run uv venv probe: {error}"),
            remediation: Some(
                "verify that uv is executable and can create a virtual environment on this host"
                    .to_string(),
            ),
        },
    };

    let _ = fs::remove_dir_all(&probe_root);
    check
}

fn perf_event_paranoid_check() -> DoctorCheck {
    let path = Path::new("/proc/sys/kernel/perf_event_paranoid");
    let Ok(raw) = fs::read_to_string(path) else {
        return DoctorCheck {
            category: "analysis",
            name: "perf_event_paranoid",
            status: DoctorStatus::Warn,
            detail: format!("could not read {}", path.display()),
            remediation: Some(
                "inspect /proc/sys/kernel/perf_event_paranoid if non-root perf capture behaves unexpectedly"
                    .to_string(),
            ),
        };
    };
    let trimmed = raw.trim();
    let Ok(value) = trimmed.parse::<i32>() else {
        return DoctorCheck {
            category: "analysis",
            name: "perf_event_paranoid",
            status: DoctorStatus::Warn,
            detail: format!("unexpected value `{trimmed}` in {}", path.display()),
            remediation: Some(
                "set a sane perf_event_paranoid value if non-root perf capture needs to work"
                    .to_string(),
            ),
        };
    };
    let (status, detail, remediation) = if value <= 2 {
        (
            DoctorStatus::Pass,
            format!("{value} (compatible with many non-root perf stat or perf record flows)"),
            None,
        )
    } else {
        (
            DoctorStatus::Warn,
            format!("{value} (non-root perf capture may be restricted; use the privileged daemon collect_perf path when needed)"),
            Some(
                "lower perf_event_paranoid for unprivileged collection, or keep using the daemon collect_perf action for bounded privileged capture"
                    .to_string(),
            ),
        )
    };
    DoctorCheck {
        category: "analysis",
        name: "perf_event_paranoid",
        status,
        detail,
        remediation,
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

fn count_helper_scripts(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| fs::read_dir(entry.path().join("scripts")).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| matches!(value, "sh" | "py"))
                .unwrap_or(false)
        })
        .count()
}

fn provider_label(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
    }
}

fn load_kernel_config(release: Option<&str>) -> Option<LoadedKernelConfig> {
    if let Some(release) = release {
        let boot_path = PathBuf::from(format!("/boot/config-{release}"));
        if let Ok(contents) = fs::read_to_string(&boot_path) {
            return Some(LoadedKernelConfig {
                source_path: boot_path,
                contents,
            });
        }
    }

    if Path::new("/proc/config.gz").is_file() {
        for command in ["gzip", "zcat"] {
            let Ok(output) = ProcessCommand::new(command)
                .args(["-dc", "/proc/config.gz"])
                .output()
            else {
                continue;
            };
            if output.status.success() {
                return Some(LoadedKernelConfig {
                    source_path: PathBuf::from("/proc/config.gz"),
                    contents: String::from_utf8_lossy(&output.stdout).into_owned(),
                });
            }
        }
    }
    None
}

fn kernel_config_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents.lines().find_map(|line| {
        let (line_key, value) = line.split_once('=')?;
        (line_key == key).then_some(value)
    })
}

fn detect_kernel_release() -> Option<KernelRelease> {
    let output = ProcessCommand::new("uname").arg("-r").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_kernel_release(&raw)
}

fn parse_kernel_release(raw: &str) -> Option<KernelRelease> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            numbers.push(current.parse::<u64>().ok()?);
            current.clear();
            if numbers.len() == 3 {
                break;
            }
        }
    }
    if !current.is_empty() && numbers.len() < 3 {
        numbers.push(current.parse::<u64>().ok()?);
    }
    if numbers.len() < 2 {
        return None;
    }
    Some(KernelRelease {
        raw: raw.to_string(),
        major: numbers[0],
        minor: numbers[1],
        patch: numbers.get(2).copied().unwrap_or_default(),
    })
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

fn command_version_line(command: &Path, args: &[&str]) -> Option<String> {
    let output = ProcessCommand::new(command).args(args).output().ok()?;
    let text = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

fn summarize_output(stdout: &[u8], stderr: &[u8]) -> String {
    for source in [stderr, stdout] {
        if let Some(line) = String::from_utf8_lossy(source)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            return line.to_string();
        }
    }
    "<no output>".to_string()
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
        DoctorCheck, DoctorReport, DoctorStatus, count_helper_scripts, count_skills,
        find_command_on_path, is_executable_file, kernel_config_value, parse_kernel_release,
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
    fn counts_helper_scripts_under_skill_dirs() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("one/scripts")).unwrap();
        fs::create_dir_all(dir.path().join("two/scripts")).unwrap();
        fs::write(dir.path().join("one/scripts/a.sh"), "#!/bin/sh\n").unwrap();
        fs::write(
            dir.path().join("two/scripts/b.py"),
            "#!/usr/bin/env python3\n",
        )
        .unwrap();
        fs::write(dir.path().join("two/scripts/c.txt"), "ignored").unwrap();

        assert_eq!(count_helper_scripts(dir.path()), 2);
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
            helper_script_count: 4,
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

    #[test]
    fn parses_kernel_release_with_distro_suffix() {
        let parsed = parse_kernel_release("6.14.0-1006-intel").unwrap();
        assert_eq!(parsed.major, 6);
        assert_eq!(parsed.minor, 14);
        assert_eq!(parsed.patch, 0);
    }

    #[test]
    fn extracts_kernel_config_values() {
        let config = "CONFIG_BPF=y\nCONFIG_SCHED_CLASS_EXT=m\n";
        assert_eq!(kernel_config_value(config, "CONFIG_BPF"), Some("y"));
        assert_eq!(
            kernel_config_value(config, "CONFIG_SCHED_CLASS_EXT"),
            Some("m")
        );
        assert_eq!(kernel_config_value(config, "CONFIG_DEBUG_INFO_BTF"), None);
    }
}
