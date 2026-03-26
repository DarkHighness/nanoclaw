use super::executor::{
    ExecRequest, NetworkPolicy, SandboxPolicy, canonicalize_filesystem_policy,
    canonicalize_optional_path, resolve_effective_cwd,
};
use crate::{Result, ToolError};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use tempfile::tempfile;
use tokio::process::Command;

const LINUX_BWRAP_CANDIDATES: &[&str] = &["bwrap", "bubblewrap"];
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

pub(super) fn find_bwrap_executable() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for candidate in LINUX_BWRAP_CANDIDATES {
            let resolved = directory.join(candidate);
            if resolved.is_file() {
                return Some(resolved);
            }
        }
    }
    None
}

pub(super) fn prepare_linux_bwrap_command(
    request: ExecRequest,
    bwrap_path: &Path,
) -> Result<Command> {
    if matches!(
        request.sandbox_policy.network,
        NetworkPolicy::AllowDomains(_)
    ) {
        return Err(ToolError::invalid_state(
            "Linux bubblewrap backend does not yet support domain-scoped network policies",
        ));
    }

    let cwd = canonicalize_optional_path(request.cwd.as_deref())?;
    let filesystem = canonicalize_filesystem_policy(&request.sandbox_policy.filesystem)?;
    let effective_policy = SandboxPolicy {
        filesystem,
        ..request.sandbox_policy.clone()
    };
    let effective_cwd = resolve_effective_cwd(cwd, &effective_policy)?;

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

    if matches!(effective_policy.network, NetworkPolicy::Off) {
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

    attach_seccomp_filter(&mut command, &build_linux_seccomp_program()?)?;

    if let Some(cwd) = effective_cwd.as_ref() {
        command.arg("--chdir").arg(cwd);
        command.current_dir(cwd);
    }

    command
        .arg("--")
        .arg(&request.program)
        .args(&request.args)
        .stdin(request.stdin.into_stdio())
        .stdout(request.stdout.into_stdio())
        .stderr(request.stderr.into_stdio())
        .kill_on_drop(request.kill_on_drop);

    if !request.env.is_empty() {
        command.envs(request.env);
    }

    let _ = request.origin;
    let _ = request.runtime_scope;

    Ok(command)
}

fn build_linux_seccomp_program() -> Result<BpfProgram> {
    let denied_syscalls = [
        libc::SYS_add_key,
        libc::SYS_bpf,
        libc::SYS_delete_module,
        libc::SYS_fanotify_init,
        libc::SYS_finit_module,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fsopen,
        libc::SYS_fspick,
        libc::SYS_init_module,
        libc::SYS_ioperm,
        libc::SYS_iopl,
        libc::SYS_kexec_load,
        libc::SYS_keyctl,
        libc::SYS_mount,
        libc::SYS_move_mount,
        libc::SYS_name_to_handle_at,
        libc::SYS_open_by_handle_at,
        libc::SYS_open_tree,
        libc::SYS_perf_event_open,
        libc::SYS_pivot_root,
        libc::SYS_ptrace,
        libc::SYS_request_key,
        libc::SYS_setns,
        libc::SYS_swapoff,
        libc::SYS_swapon,
        libc::SYS_umount2,
        libc::SYS_unshare,
        libc::SYS_userfaultfd,
    ];
    let rules = denied_syscalls
        .into_iter()
        .map(|syscall| (syscall, Vec::new()))
        .collect::<BTreeMap<_, _>>();

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        std::env::consts::ARCH.try_into().map_err(|error| {
            ToolError::invalid_state(format!(
                "unsupported Linux seccomp architecture {}: {error}",
                std::env::consts::ARCH
            ))
        })?,
    )
    .map_err(|error| {
        ToolError::invalid_state(format!("failed to build seccomp filter: {error}"))
    })?;

    filter.try_into().map_err(|error| {
        ToolError::invalid_state(format!("failed to compile seccomp filter: {error}"))
    })
}

fn attach_seccomp_filter(command: &mut Command, program: &BpfProgram) -> Result<()> {
    let mut seccomp_file = tempfile().map_err(|error| {
        ToolError::invalid_state(format!("failed to create seccomp tempfile: {error}"))
    })?;
    write_bpf_program(&mut seccomp_file, program)?;
    seccomp_file.rewind().map_err(|error| {
        ToolError::invalid_state(format!("failed to rewind seccomp tempfile: {error}"))
    })?;
    let seccomp_fd = seccomp_file.as_raw_fd();
    command.arg("--seccomp").arg(seccomp_fd.to_string());

    // Bubblewrap wants a raw inherited fd rather than a file path. Keeping the
    // tempfile inside the pre-exec closure preserves the descriptor until
    // `execve`, and clearing `FD_CLOEXEC` lets the target `bwrap` process read
    // the compiled BPF program without widening the ProcessExecutor trait.
    unsafe {
        command.pre_exec(move || {
            let _keep_alive = &seccomp_file;
            clear_close_on_exec(seccomp_fd)?;
            Ok(())
        });
    }

    Ok(())
}

fn write_bpf_program(file: &mut File, program: &BpfProgram) -> Result<()> {
    let bytes = unsafe {
        std::slice::from_raw_parts(
            program.as_ptr().cast::<u8>(),
            program.len() * size_of::<seccompiler::sock_filter>(),
        )
    };
    file.write_all(bytes).map_err(|error| {
        ToolError::invalid_state(format!("failed to write seccomp program: {error}"))
    })?;
    Ok(())
}

fn clear_close_on_exec(fd: i32) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn ensure_policy_mount_source_exists(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    Err(ToolError::invalid_state(format!(
        "sandbox {label} {} does not exist on the host",
        path.display()
    )))
}

fn add_bind_mount(command: &mut Command, flag: &str, source: &Path, dest: &Path) {
    command.arg(flag).arg(source).arg(dest);
}
