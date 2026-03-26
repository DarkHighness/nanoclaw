use crate::{Result, ToolError, ToolExecutionContext};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use types::{CallId, RunId, SessionId, TurnId};

const DEFAULT_PROTECTED_DIRS: &[&str] = &[".git", ".agent-core", ".codex"];
#[cfg(target_os = "macos")]
const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    Off,
    AllowDomains(Vec<String>),
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEscapePolicy {
    Deny,
    HostManaged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub filesystem: FilesystemPolicy,
    pub network: NetworkPolicy,
    pub host_escape: HostEscapePolicy,
    pub fail_if_unavailable: bool,
}

impl SandboxPolicy {
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            mode: SandboxMode::DangerFullAccess,
            filesystem: FilesystemPolicy::default(),
            network: NetworkPolicy::Full,
            host_escape: HostEscapePolicy::HostManaged,
            fail_if_unavailable: false,
        }
    }

    #[must_use]
    pub fn recommended_for_context(ctx: &ToolExecutionContext) -> Self {
        if !ctx.workspace_only {
            return Self::permissive();
        }

        let mut roots = vec![ctx.workspace_root.clone()];
        if let Some(worktree_root) = ctx.worktree_root.clone() {
            roots.push(worktree_root);
        }
        roots.extend(ctx.additional_roots.iter().cloned());
        let roots = dedup_paths(roots);
        let protected_paths = dedup_paths(
            roots
                .iter()
                .flat_map(|root| {
                    DEFAULT_PROTECTED_DIRS
                        .iter()
                        .map(move |name| root.join(name))
                })
                .collect(),
        );

        Self {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: roots.clone(),
                writable_roots: roots,
                protected_paths,
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            // Hosts can tighten this later once the Linux and container
            // backends land. Keeping the first rollout best-effort avoids
            // turning "no backend yet" into a surprising total loss of local
            // process execution on unsupported platforms.
            fail_if_unavailable: false,
        }
    }

    #[must_use]
    pub fn requires_enforcement(&self) -> bool {
        !matches!(self.mode, SandboxMode::DangerFullAccess)
            || !matches!(self.network, NetworkPolicy::Full)
            || !self.filesystem.readable_roots.is_empty()
            || !self.filesystem.writable_roots.is_empty()
            || !self.filesystem.protected_paths.is_empty()
    }
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        // The current substrate still launches local processes on the host by
        // default. Keeping that posture explicit avoids pretending there is a
        // hard boundary before an enforcing backend is wired in.
        Self::permissive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOrigin {
    BashTool,
    HookCommand,
    McpStdioServer { server_name: String },
    HostUtility { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RuntimeScope {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<CallId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStdio {
    Inherit,
    Null,
    Piped,
}

impl ProcessStdio {
    fn into_stdio(self) -> Stdio {
        match self {
            Self::Inherit => Stdio::inherit(),
            Self::Null => Stdio::null(),
            Self::Piped => Stdio::piped(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub stdin: ProcessStdio,
    pub stdout: ProcessStdio,
    pub stderr: ProcessStdio,
    pub kill_on_drop: bool,
    pub origin: ExecutionOrigin,
    pub runtime_scope: RuntimeScope,
    pub sandbox_policy: SandboxPolicy,
}

pub trait ProcessExecutor: Send + Sync {
    fn prepare(&self, request: ExecRequest) -> Result<Command>;
}

#[derive(Default)]
pub struct HostProcessExecutor;

impl ProcessExecutor for HostProcessExecutor {
    fn prepare(&self, request: ExecRequest) -> Result<Command> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(request.stdin.into_stdio())
            .stdout(request.stdout.into_stdio())
            .stderr(request.stderr.into_stdio())
            .kill_on_drop(request.kill_on_drop);
        if let Some(cwd) = request.cwd {
            command.current_dir(cwd);
        }
        if !request.env.is_empty() {
            command.envs(request.env);
        }

        // Phase 1 centralizes child-process construction under one boundary
        // without changing behavior. Enforcing backends will interpret
        // `sandbox_policy` here later instead of at every call site.
        let _ = request.origin;
        let _ = request.runtime_scope;
        let _ = request.sandbox_policy;

        Ok(command)
    }
}

#[derive(Default)]
pub struct ManagedPolicyProcessExecutor {
    host: HostProcessExecutor,
}

impl ManagedPolicyProcessExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProcessExecutor for ManagedPolicyProcessExecutor {
    fn prepare(&self, request: ExecRequest) -> Result<Command> {
        if !request.sandbox_policy.requires_enforcement() {
            return self.host.prepare(request);
        }

        #[cfg(target_os = "macos")]
        {
            if Path::new(MACOS_SANDBOX_EXEC).exists() {
                return prepare_macos_seatbelt_command(request);
            }
        }

        if request.sandbox_policy.fail_if_unavailable {
            Err(ToolError::invalid_state(
                "sandbox policy requires an enforcing backend, but no compatible backend is available",
            ))
        } else {
            self.host.prepare(request)
        }
    }
}

#[cfg(target_os = "macos")]
fn prepare_macos_seatbelt_command(request: ExecRequest) -> Result<Command> {
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

    let mut command = Command::new(MACOS_SANDBOX_EXEC);
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn resolve_effective_cwd(cwd: Option<PathBuf>, policy: &SandboxPolicy) -> Result<Option<PathBuf>> {
    let roots = accessible_roots(policy);
    match cwd {
        Some(cwd) if roots.is_empty() || path_is_inside_any_root(&cwd, &roots) => Ok(Some(cwd)),
        Some(cwd) => Err(ToolError::invalid_state(format!(
            "sandboxed process cwd {} is outside the configured sandbox roots",
            cwd.display()
        ))),
        None if roots.is_empty() => Ok(None),
        None => Ok(Some(
            roots
                .first()
                .expect("sandbox roots should be non-empty")
                .clone(),
        )),
    }
}

#[cfg(target_os = "macos")]
fn canonicalize_filesystem_policy(policy: &FilesystemPolicy) -> Result<FilesystemPolicy> {
    Ok(FilesystemPolicy {
        readable_roots: dedup_paths(
            policy
                .readable_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
        writable_roots: dedup_paths(
            policy
                .writable_roots
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
        protected_paths: dedup_paths(
            policy
                .protected_paths
                .iter()
                .map(|path| canonicalize_policy_path(path))
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}

#[cfg(target_os = "macos")]
fn canonicalize_optional_path(path: Option<&Path>) -> Result<Option<PathBuf>> {
    path.map(canonicalize_policy_path).transpose()
}

#[cfg(target_os = "macos")]
fn canonicalize_policy_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(|source| {
            ToolError::invalid_state(format!(
                "failed to canonicalize sandbox path {}: {source}",
                path.display()
            ))
        });
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut existing_ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !existing_ancestor.exists() {
        let Some(file_name) = existing_ancestor.file_name() else {
            return Err(ToolError::invalid_state(format!(
                "sandbox path {} has no existing ancestor",
                absolute.display()
            )));
        };
        suffix.push(file_name.to_os_string());
        existing_ancestor = existing_ancestor.parent().ok_or_else(|| {
            ToolError::invalid_state(format!("sandbox path {} has no parent", absolute.display()))
        })?;
    }

    let mut normalized = std::fs::canonicalize(existing_ancestor).map_err(|source| {
        ToolError::invalid_state(format!(
            "failed to canonicalize sandbox ancestor {}: {source}",
            existing_ancestor.display()
        ))
    })?;
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

#[cfg(target_os = "macos")]
fn accessible_roots(policy: &SandboxPolicy) -> Vec<PathBuf> {
    let mut roots = policy.filesystem.writable_roots.clone();
    roots.extend(policy.filesystem.readable_roots.iter().cloned());
    dedup_paths(roots)
}

#[cfg(target_os = "macos")]
fn path_is_inside_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

#[cfg(target_os = "macos")]
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
    ancestors.into_iter().collect()
}

#[cfg(target_os = "macos")]
fn escape_sbpl_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = BTreeSet::new();
    for path in paths {
        unique.insert(path);
    }
    unique.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ExecRequest, ExecutionOrigin, FilesystemPolicy, HostEscapePolicy,
        ManagedPolicyProcessExecutor, NetworkPolicy, ProcessExecutor, ProcessStdio, RuntimeScope,
        SandboxMode, SandboxPolicy,
    };
    use crate::ToolExecutionContext;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn recommended_policy_for_workspace_context_is_workspace_write_and_network_off() {
        let workspace = tempdir().unwrap();
        let extra = tempdir().unwrap();
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            additional_roots: vec![extra.path().to_path_buf()],
            workspace_only: true,
            ..Default::default()
        };

        let policy = SandboxPolicy::recommended_for_context(&context);

        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(policy.network, NetworkPolicy::Off);
        assert_eq!(policy.host_escape, HostEscapePolicy::Deny);
        assert!(
            policy
                .filesystem
                .writable_roots
                .contains(&workspace.path().to_path_buf())
        );
        assert!(
            policy
                .filesystem
                .writable_roots
                .contains(&extra.path().to_path_buf())
        );
        assert!(
            policy
                .filesystem
                .protected_paths
                .contains(&workspace.path().join(".git"))
        );
        assert!(
            policy
                .filesystem
                .protected_paths
                .contains(&workspace.path().join(".agent-core"))
        );
    }

    #[test]
    fn recommended_policy_is_permissive_when_workspace_only_is_disabled() {
        let workspace = tempdir().unwrap();
        let context = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            workspace_only: false,
            ..Default::default()
        };

        assert_eq!(
            SandboxPolicy::recommended_for_context(&context),
            SandboxPolicy::permissive()
        );
    }

    #[test]
    fn managed_policy_executor_leaves_permissive_requests_unsandboxed() {
        let executor = ManagedPolicyProcessExecutor::new();
        let command = executor
            .prepare(ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: None,
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::HostUtility {
                    name: "test".to_string(),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: SandboxPolicy::permissive(),
            })
            .unwrap();

        assert_eq!(command.as_std().get_program(), "/bin/echo");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_policy_executor_wraps_restrictive_requests_with_sandbox_exec() {
        let workspace = tempdir().unwrap();
        let policy = SandboxPolicy {
            mode: SandboxMode::WorkspaceWrite,
            filesystem: FilesystemPolicy {
                readable_roots: vec![workspace.path().to_path_buf()],
                writable_roots: vec![workspace.path().to_path_buf()],
                protected_paths: vec![workspace.path().join(".git")],
            },
            network: NetworkPolicy::Off,
            host_escape: HostEscapePolicy::Deny,
            fail_if_unavailable: true,
        };
        let executor = ManagedPolicyProcessExecutor::new();
        let command = executor
            .prepare(ExecRequest {
                program: "/bin/echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: Some(workspace.path().to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Null,
                stderr: ProcessStdio::Null,
                kill_on_drop: true,
                origin: ExecutionOrigin::BashTool,
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: policy,
            })
            .unwrap();

        assert_eq!(
            command.as_std().get_program(),
            std::ffi::OsStr::new(super::MACOS_SANDBOX_EXEC)
        );
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(import \"system.sb\")"));
        assert!(args[1].contains(".git"));
        assert!(
            args[1].contains(
                &workspace
                    .path()
                    .canonicalize()
                    .unwrap()
                    .display()
                    .to_string()
            )
        );
        assert_eq!(args[2], "/bin/echo");
        assert_eq!(args[3], "hello");
    }
}
