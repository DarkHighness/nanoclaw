mod profile;
mod proxy;

pub(crate) use proxy::has_allow_domains_proxy_config;

use crate::Result;
use crate::policy::{
    ExecRequest, NetworkPolicy, SandboxPolicy, canonicalize_filesystem_policy,
    canonicalize_optional_path, resolve_effective_cwd,
};
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(crate) const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub(crate) fn sandbox_exec_path() -> Option<PathBuf> {
    Path::new(MACOS_SANDBOX_EXEC)
        .exists()
        .then(|| PathBuf::from(MACOS_SANDBOX_EXEC))
}

pub(crate) fn prepare_macos_seatbelt_command(
    mut request: ExecRequest,
    sandbox_exec_path: &Path,
) -> Result<Command> {
    let proxy_config = match &request.sandbox_policy.network {
        NetworkPolicy::Allowlist(allowlist) => {
            proxy::configure_allow_domains_proxy_env(&mut request.env, &allowlist.domains)?
        }
        _ => proxy::AllowDomainsProxyConfig::default(),
    };

    let cwd = canonicalize_optional_path(request.cwd.as_deref())?;
    let filesystem = canonicalize_filesystem_policy(&request.sandbox_policy.filesystem)?;
    let effective_policy = SandboxPolicy {
        filesystem,
        ..request.sandbox_policy.clone()
    };
    let effective_cwd = resolve_effective_cwd(cwd, &effective_policy)?;
    let profile = profile::build_macos_seatbelt_profile(&effective_policy, &proxy_config)?;

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
