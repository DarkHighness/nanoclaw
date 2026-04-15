use super::proxy::AllowDomainsProxyConfig;
use crate::policy::{NetworkPolicy, SandboxMode, SandboxPolicy, accessible_roots};
use crate::{Result, SandboxError};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MACOS_SEATBELT_BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");
const MACOS_SYSTEM_READONLY_ROOTS: &[&str] = &[
    "/System",
    "/usr",
    "/bin",
    "/sbin",
    "/Library",
    "/etc",
    "/private/etc",
    "/var/db",
    "/private/var/db",
];

pub(super) fn build_macos_seatbelt_profile(
    policy: &SandboxPolicy,
    proxy_config: &AllowDomainsProxyConfig,
) -> Result<String> {
    let mut lines = vec![
        // `system.sb` is the stable Apple-provided base profile that keeps
        // dyld, sysctl, mach, and standard system-path access coherent. The
        // generated rules below then narrow host-visible workspace roots on top
        // of that baseline instead of trying to hand-maintain a fragile clone
        // of Apple's system allowances.
        MACOS_SEATBELT_BASE_POLICY.trim().to_string(),
    ];

    if !policy.filesystem.readable_roots.is_empty()
        || !policy.filesystem.writable_roots.is_empty()
        || !policy.filesystem.protected_paths.is_empty()
    {
        // Seatbelt evaluates real paths rather than the user-facing `/var`
        // aliases the shell often exposes, so every host path is canonicalized
        // before it is embedded into the generated profile.
        lines.push("(allow file-read-metadata)".to_string());
    }

    for ancestor in policy_path_ancestors(policy) {
        lines.push(format!(
            "(allow file-read-metadata (literal \"{}\"))",
            escape_sbpl_path(&ancestor)
        ));
    }

    match policy.mode {
        SandboxMode::DangerFullAccess => lines.push("(allow file*)".to_string()),
        SandboxMode::ReadOnly | SandboxMode::WorkspaceWrite => {
            // Common tooling needs to read curated system config and trust
            // stores outside the workspace. Linux already exposes a readonly
            // system view through bind mounts; macOS needs the same baseline or
            // networked tools fail before they ever reach the localhost proxy.
            for root in MACOS_SYSTEM_READONLY_ROOTS {
                lines.push(format!(
                    "(allow file-read* file-map-executable file-test-existence (subpath \"{}\"))",
                    escape_sbpl_path(Path::new(root))
                ));
            }
            for root in &policy.filesystem.readable_roots {
                lines.push(format!(
                    "(allow file-read* file-map-executable file-test-existence (subpath \"{}\"))",
                    escape_sbpl_path(root)
                ));
            }
            if matches!(policy.mode, SandboxMode::WorkspaceWrite) {
                for root in &policy.filesystem.writable_roots {
                    lines.push(format!(
                        "(allow file* (subpath \"{}\"))",
                        escape_sbpl_path(root)
                    ));
                }
            }
        }
    }

    for protected in &policy.filesystem.protected_paths {
        lines.push(format!(
            "(deny file-write* (subpath \"{}\"))",
            escape_sbpl_path(protected)
        ));
    }

    match &policy.network {
        NetworkPolicy::Off => {}
        NetworkPolicy::Full => lines.push("(allow network*)".to_string()),
        NetworkPolicy::Allowlist(allowlist) => {
            if allowlist.domains.is_empty() {
                return Err(SandboxError::invalid_state(
                    "domain-scoped network policy requires at least one allowed domain",
                ));
            }
            // Seatbelt cannot natively filter arbitrary hostnames. The most
            // defensible boundary it can provide here is "proxy-only egress":
            // deny generic networking and then allow loopback so a host-owned
            // localhost proxy can enforce the domain allowlist out of process.
            lines.push("(deny network*)".to_string());
            if proxy_config.loopback_ports.is_empty() {
                lines.push("(allow network* (local ip \"localhost:*\"))".to_string());
                lines.push("(allow network* (remote ip \"localhost:*\"))".to_string());
            } else {
                for port in &proxy_config.loopback_ports {
                    lines.push(format!(
                        "(allow network* (local ip \"localhost:{}\"))",
                        port
                    ));
                    lines.push(format!(
                        "(allow network* (remote ip \"localhost:{}\"))",
                        port
                    ));
                }
            }
        }
    }

    Ok(lines.join("\n"))
}

fn policy_path_ancestors(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut ancestors = BTreeSet::new();
    for path in policy
        .filesystem
        .readable_roots
        .iter()
        .chain(policy.filesystem.writable_roots.iter())
        .chain(policy.filesystem.protected_paths.iter())
    {
        for ancestor in path.ancestors() {
            ancestors.insert(ancestor.to_path_buf());
        }
    }
    // `resolve_effective_cwd` already guarantees cwd stays inside configured
    // roots, so granting metadata access to root ancestors is sufficient for
    // startup path traversal without widening real file contents beyond the
    // declared read/write roots.
    ancestors.extend(accessible_roots(policy));
    ancestors.into_iter().collect()
}

fn escape_sbpl_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}
