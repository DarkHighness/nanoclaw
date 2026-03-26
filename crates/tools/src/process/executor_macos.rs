use super::executor::{
    ExecRequest, NetworkPolicy, SandboxMode, SandboxPolicy, accessible_roots,
    canonicalize_filesystem_policy, canonicalize_optional_path, resolve_effective_cwd,
};
use crate::{Result, ToolError};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(super) const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub(super) fn sandbox_exec_path() -> Option<PathBuf> {
    Path::new(MACOS_SANDBOX_EXEC)
        .exists()
        .then(|| PathBuf::from(MACOS_SANDBOX_EXEC))
}

pub(super) fn prepare_macos_seatbelt_command(
    request: ExecRequest,
    sandbox_exec_path: &Path,
) -> Result<Command> {
    if matches!(
        request.sandbox_policy.network,
        NetworkPolicy::AllowDomains(_)
    ) {
        return Err(ToolError::invalid_state(
            "macOS Seatbelt backend does not yet support domain-scoped network policies",
        ));
    }

    let cwd = canonicalize_optional_path(request.cwd.as_deref())?;
    let filesystem = canonicalize_filesystem_policy(&request.sandbox_policy.filesystem)?;
    let effective_policy = SandboxPolicy {
        filesystem,
        ..request.sandbox_policy.clone()
    };
    let effective_cwd = resolve_effective_cwd(cwd, &effective_policy)?;
    let profile = build_macos_seatbelt_profile(&effective_policy)?;

    let mut command = Command::new(sandbox_exec_path);
    command
        .arg("-p")
        .arg(profile)
        .arg(&request.program)
        .args(&request.args)
        .stdin(request.stdin.into_stdio())
        .stdout(request.stdout.into_stdio())
        .stderr(request.stderr.into_stdio())
        .kill_on_drop(request.kill_on_drop);

    if let Some(cwd) = effective_cwd {
        command.current_dir(cwd);
    }
    if !request.env.is_empty() {
        command.envs(request.env);
    }

    let _ = request.origin;
    let _ = request.runtime_scope;

    Ok(command)
}

fn build_macos_seatbelt_profile(policy: &SandboxPolicy) -> Result<String> {
    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        // `system.sb` is the stable Apple-provided base profile that keeps
        // dyld, sysctl, mach, and standard system-path access coherent. The
        // generated rules below then narrow host-visible workspace roots on top
        // of that baseline instead of trying to hand-maintain a fragile clone
        // of Apple's system allowances.
        "(import \"system.sb\")".to_string(),
        "(allow process*)".to_string(),
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

    if matches!(policy.network, NetworkPolicy::Full) {
        lines.push("(allow network*)".to_string());
    }

    Ok(lines.join(" "))
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
