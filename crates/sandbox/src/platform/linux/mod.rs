mod bwrap;
mod proxy;
mod seccomp;

pub(crate) use bwrap::prepare_linux_bwrap_command;
pub(crate) use proxy::{
    LINUX_ALLOW_DOMAINS_PROXY_BRIDGE_PORT, LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV,
    LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV, LINUX_ALLOW_DOMAINS_PROXY_URL_ENV,
};

use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio as StdStdio};
use std::sync::OnceLock;

const LINUX_BWRAP_CANDIDATES: &[&str] = &["bwrap", "bubblewrap"];
const LINUX_SOCAT_CANDIDATES: &[&str] = &["socat"];

static LINUX_BWRAP_STATUS: OnceLock<LinuxBubblewrapStatus> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinuxBubblewrapStatus {
    Available(PathBuf),
    Unavailable { reason: String },
}

pub(crate) fn find_bwrap_executable() -> Option<PathBuf> {
    match linux_bwrap_status() {
        LinuxBubblewrapStatus::Available(path) => Some(path),
        LinuxBubblewrapStatus::Unavailable { .. } => None,
    }
}

pub(crate) fn linux_bwrap_status() -> LinuxBubblewrapStatus {
    LINUX_BWRAP_STATUS.get_or_init(detect_bwrap_status).clone()
}

pub(super) fn find_socat_executable() -> Option<PathBuf> {
    find_first_usable_executable(LINUX_SOCAT_CANDIDATES, |_| true)
}

fn find_first_usable_executable(
    candidates: &[&str],
    is_usable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for candidate in candidates {
            let resolved = directory.join(candidate);
            if resolved.is_file() && is_usable(&resolved) {
                return Some(resolved);
            }
        }
    }
    None
}

fn detect_bwrap_status() -> LinuxBubblewrapStatus {
    let Some(path) = std::env::var_os("PATH") else {
        return LinuxBubblewrapStatus::Unavailable {
            reason: "PATH is unset, so no `bwrap` executable can be discovered".to_string(),
        };
    };
    let mut first_failure = None;
    for directory in std::env::split_paths(&path) {
        for candidate in LINUX_BWRAP_CANDIDATES {
            let resolved = directory.join(candidate);
            if !resolved.is_file() {
                continue;
            }
            match probe_bwrap_runtime(&resolved) {
                Ok(()) => return LinuxBubblewrapStatus::Available(resolved),
                Err(reason) => {
                    if first_failure.is_none() {
                        first_failure = Some(format!(
                            "`{}` exists but cannot create an unprivileged sandbox: {reason}",
                            resolved.display()
                        ));
                    }
                }
            }
        }
    }
    LinuxBubblewrapStatus::Unavailable {
        reason: first_failure.unwrap_or_else(|| {
            "no `bwrap` or `bubblewrap` executable was found in PATH".to_string()
        }),
    }
}

fn probe_bwrap_runtime(bwrap_path: &Path) -> std::result::Result<(), String> {
    // Linux distributions often ship `bwrap` even when the surrounding
    // container/kernel forbids unprivileged user namespaces. Treating binary
    // presence as availability makes fail-closed policies report a later child
    // process crash instead of "no backend is available", so probe the actual
    // namespace boundary once before selecting bubblewrap.
    match StdCommand::new(bwrap_path)
        .arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--")
        .arg("/bin/sh")
        .arg("-lc")
        .arg("true")
        .stdin(StdStdio::null())
        .stdout(StdStdio::piped())
        .stderr(StdStdio::piped())
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("exit status {}", output.status)
            };
            Err(detail)
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::probe_bwrap_runtime;

    #[test]
    fn bwrap_probe_accepts_zero_exit_status() {
        assert!(probe_bwrap_runtime(std::path::Path::new("/bin/true")).is_ok());
    }

    #[test]
    fn bwrap_probe_rejects_non_zero_exit_status() {
        assert!(probe_bwrap_runtime(std::path::Path::new("/bin/false")).is_err());
    }
}
