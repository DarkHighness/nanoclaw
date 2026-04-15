use crate::policy::NetworkPolicy;
use crate::{Result, SandboxError};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use tempfile::tempfile;
use tokio::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinuxSeccompProfile {
    FullNetwork,
    NoNetwork,
    ProxyOnlyUnixSockets,
}

pub(crate) fn select_seccomp_profile(network: &NetworkPolicy) -> LinuxSeccompProfile {
    match network {
        NetworkPolicy::Full => LinuxSeccompProfile::FullNetwork,
        NetworkPolicy::Off => LinuxSeccompProfile::NoNetwork,
        NetworkPolicy::Allowlist(_) => LinuxSeccompProfile::ProxyOnlyUnixSockets,
    }
}

pub(crate) fn build_linux_seccomp_program(profile: LinuxSeccompProfile) -> Result<BpfProgram> {
    let mut rules = BTreeMap::new();
    add_unconditional_allow_rules(&mut rules, &baseline_allowed_syscalls());

    match profile {
        LinuxSeccompProfile::FullNetwork => {
            add_unconditional_allow_rules(&mut rules, &network_allowed_syscalls())
        }
        LinuxSeccompProfile::NoNetwork => {}
        LinuxSeccompProfile::ProxyOnlyUnixSockets => {
            add_unconditional_allow_rules(&mut rules, &proxy_flow_syscalls());
            add_proxy_bridge_socket_rules(&mut rules, libc::SYS_socket)?;
            add_proxy_bridge_socket_rules(&mut rules, libc::SYS_socketpair)?;
        }
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        std::env::consts::ARCH.try_into().map_err(|error| {
            SandboxError::invalid_state(format!(
                "unsupported Linux seccomp architecture {}: {error}",
                std::env::consts::ARCH
            ))
        })?,
    )
    .map_err(|error| {
        SandboxError::invalid_state(format!("failed to build seccomp filter: {error}"))
    })?;

    filter.try_into().map_err(|error| {
        SandboxError::invalid_state(format!("failed to compile seccomp filter: {error}"))
    })
}

pub(crate) fn attach_seccomp_filter(command: &mut Command, program: &BpfProgram) -> Result<()> {
    let mut seccomp_file = tempfile().map_err(|error| {
        SandboxError::invalid_state(format!("failed to create seccomp tempfile: {error}"))
    })?;
    write_bpf_program(&mut seccomp_file, program)?;
    seccomp_file.rewind().map_err(|error| {
        SandboxError::invalid_state(format!("failed to rewind seccomp tempfile: {error}"))
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

fn baseline_allowed_syscalls() -> Vec<i64> {
    let mut syscalls = vec![
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_openat,
        libc::SYS_close,
        libc::SYS_statx,
        libc::SYS_newfstatat,
        libc::SYS_fstat,
        libc::SYS_lseek,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_ioctl,
        libc::SYS_access,
        libc::SYS_faccessat,
        libc::SYS_pipe,
        libc::SYS_pipe2,
        libc::SYS_dup,
        libc::SYS_dup2,
        libc::SYS_dup3,
        libc::SYS_fcntl,
        libc::SYS_getdents64,
        libc::SYS_getcwd,
        libc::SYS_chdir,
        libc::SYS_fchdir,
        libc::SYS_readlink,
        libc::SYS_readlinkat,
        libc::SYS_unlinkat,
        libc::SYS_mkdirat,
        libc::SYS_renameat,
        libc::SYS_renameat2,
        libc::SYS_linkat,
        libc::SYS_symlinkat,
        libc::SYS_umask,
        libc::SYS_ftruncate,
        libc::SYS_fsync,
        libc::SYS_fdatasync,
        libc::SYS_fallocate,
        libc::SYS_clock_gettime,
        libc::SYS_clock_getres,
        libc::SYS_nanosleep,
        libc::SYS_clock_nanosleep,
        libc::SYS_getrandom,
        libc::SYS_uname,
        libc::SYS_getpid,
        libc::SYS_getppid,
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_prlimit64,
        libc::SYS_clone,
        libc::SYS_clone3,
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_wait4,
        libc::SYS_waitid,
        libc::SYS_futex,
        libc::SYS_sched_yield,
        libc::SYS_sched_getaffinity,
        libc::SYS_prctl,
        libc::SYS_getrusage,
        libc::SYS_kill,
        libc::SYS_tgkill,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_poll,
        libc::SYS_ppoll,
        libc::SYS_select,
        libc::SYS_pselect6,
        libc::SYS_epoll_create1,
        libc::SYS_epoll_ctl,
        libc::SYS_epoll_wait,
        libc::SYS_epoll_pwait,
        libc::SYS_eventfd2,
    ];
    #[cfg(target_arch = "x86_64")]
    {
        // x86_64 dynamic loader initialization requires TLS arch setup.
        syscalls.push(libc::SYS_arch_prctl);
    }
    syscalls
}

fn network_allowed_syscalls() -> Vec<i64> {
    vec![
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_shutdown,
    ]
}

fn proxy_flow_syscalls() -> Vec<i64> {
    // `socket` and `socketpair` are constrained to AF_UNIX by argument-matched
    // rules. The remaining calls operate on already-created descriptors.
    vec![
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_shutdown,
    ]
}

fn add_unconditional_allow_rules(rules: &mut BTreeMap<i64, Vec<SeccompRule>>, syscalls: &[i64]) {
    for syscall in syscalls {
        rules.entry(*syscall).or_default();
    }
}

fn add_proxy_bridge_socket_rules(
    rules: &mut BTreeMap<i64, Vec<SeccompRule>>,
    syscall: i64,
) -> Result<()> {
    let mut socket_rules = Vec::new();
    for family in [libc::AF_UNIX, libc::AF_INET, libc::AF_INET6] {
        let domain_condition =
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, family as u64)
                .map_err(|error| {
                    SandboxError::invalid_state(format!(
                        "failed to build seccomp socket-domain condition: {error}"
                    ))
                })?;
        let rule = SeccompRule::new(vec![domain_condition]).map_err(|error| {
            SandboxError::invalid_state(format!(
                "failed to build seccomp socket-domain rule: {error}"
            ))
        })?;
        socket_rules.push(rule);
    }
    rules.insert(syscall, socket_rules);
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
        SandboxError::invalid_state(format!("failed to write seccomp program: {error}"))
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
