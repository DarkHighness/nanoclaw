use super::find_socat_executable;
use super::proxy::{
    append_allow_domains_bridge_wrapper, apply_allow_domains_proxy_env,
    bind_allow_domains_proxy_bridge, resolve_allow_domains_proxy_bridge,
};
use super::seccomp::{attach_seccomp_filter, build_linux_seccomp_program, select_seccomp_profile};
use crate::policy::{
    canonicalize_filesystem_policy, canonicalize_optional_path, resolve_effective_cwd,
};
use crate::{ExecRequest, NetworkPolicy, Result, SandboxError, SandboxPolicy};
use std::path::Path;
use tokio::process::Command;

const LINUX_SYSTEM_READONLY_ROOTS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/etc",
    "/opt",
    "/nix/store",
    "/run/current-system/sw",
];

pub(crate) fn prepare_linux_bwrap_command(
    request: ExecRequest,
    bwrap_path: &Path,
) -> Result<Command> {
    let cwd = canonicalize_optional_path(request.cwd.as_deref())?;
    let filesystem = canonicalize_filesystem_policy(&request.sandbox_policy.filesystem)?;
    let effective_policy = SandboxPolicy {
        filesystem,
        ..request.sandbox_policy.clone()
    };
    let proxy_bridge = match &effective_policy.network {
        NetworkPolicy::Allowlist(allowlist) => Some(resolve_allow_domains_proxy_bridge(
            &allowlist.domains,
            &request.env,
        )?),
        _ => None,
    };
    let socat_path = if proxy_bridge.is_some() {
        Some(find_socat_executable().ok_or_else(|| {
            SandboxError::invalid_state(
                "Linux allow-domains policy requires `socat` to bridge loopback proxy traffic inside the sandbox",
            )
        })?)
    } else {
        None
    };
    let seccomp_profile = select_seccomp_profile(&effective_policy.network);
    let effective_cwd = resolve_effective_cwd(cwd, &effective_policy)?;
    let mut effective_env = request.env;
    apply_allow_domains_proxy_env(proxy_bridge.as_ref(), &mut effective_env);

    let mut command = Command::new(bwrap_path);
    command
        .arg("--die-with-parent")
        .arg("--new-session")
        .arg("--unshare-pid")
        .arg("--unshare-ipc")
        .arg("--unshare-uts")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--tmpfs")
        .arg("/tmp")
        .arg("--tmpfs")
        .arg("/var/tmp");

    if requires_network_namespace_isolation(&effective_policy.network) {
        command.arg("--unshare-net");
    }

    for root in LINUX_SYSTEM_READONLY_ROOTS {
        command.arg("--ro-bind-try").arg(root).arg(root);
    }

    // Bubblewrap applies mounts in argument order. Read-only roots land first,
    // writable roots override them where needed, and protected paths are
    // rebound read-only last so control directories such as `.git` stay
    // immutable even inside otherwise writable workspaces.
    for root in &effective_policy.filesystem.readable_roots {
        ensure_policy_mount_source_exists(root, "readable root")?;
        add_bind_mount(&mut command, "--ro-bind", root, root);
    }
    for root in &effective_policy.filesystem.writable_roots {
        ensure_policy_mount_source_exists(root, "writable root")?;
        add_bind_mount(&mut command, "--bind", root, root);
    }
    for path in &effective_policy.filesystem.protected_paths {
        if path.exists() {
            add_bind_mount(&mut command, "--ro-bind", path, path);
        }
    }

    if let Some(bridge) = proxy_bridge.as_ref() {
        bind_allow_domains_proxy_bridge(&mut command, bridge)?;
    }

    attach_seccomp_filter(&mut command, &build_linux_seccomp_program(seccomp_profile)?)?;

    if let Some(cwd) = effective_cwd.as_ref() {
        command.arg("--chdir").arg(cwd);
        command.current_dir(cwd);
    }

    command.arg("--");
    if let (Some(bridge), Some(socat_path)) = (proxy_bridge.as_ref(), socat_path.as_ref()) {
        append_allow_domains_bridge_wrapper(
            &mut command,
            bridge,
            socat_path,
            &request.program,
            &request.args,
        )?;
    } else {
        command.arg(&request.program).args(&request.args);
    }
    command
        .stdin(request.stdin.into_stdio())
        .stdout(request.stdout.into_stdio())
        .stderr(request.stderr.into_stdio())
        .kill_on_drop(request.kill_on_drop);

    if !effective_env.is_empty() {
        command.envs(effective_env);
    }

    let _ = request.origin;
    let _ = request.runtime_scope;

    Ok(command)
}

fn requires_network_namespace_isolation(network: &NetworkPolicy) -> bool {
    matches!(network, NetworkPolicy::Off | NetworkPolicy::Allowlist(_))
}

fn ensure_policy_mount_source_exists(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    Err(SandboxError::invalid_state(format!(
        "sandbox {label} {} does not exist on the host",
        path.display()
    )))
}

fn add_bind_mount(command: &mut Command, flag: &str, source: &Path, dest: &Path) {
    command.arg(flag).arg(source).arg(dest);
}
