use std::collections::BTreeMap;
use std::path::Path;

use tempfile::tempdir;
use tools::{
    ExecRequest, ExecutionOrigin, ManagedPolicyProcessExecutor, ProcessExecutor, ProcessStdio,
    RuntimeScope, SandboxPolicy, ToolExecutionContext,
};

#[cfg(target_os = "linux")]
const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV: &str = "NANOCLAW_SANDBOX_PROXY_SOCKET_PATH";
#[cfg(target_os = "linux")]
const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV: &str =
    "NANOCLAW_SANDBOX_PROXY_SOCKET_SANDBOX_PATH";
#[cfg(target_os = "linux")]
const LINUX_ALLOW_DOMAINS_PROXY_URL_ENV: &str = "NANOCLAW_SANDBOX_PROXY_URL";

fn platform_backend_is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new("/usr/bin/sandbox-exec").exists()
    }
    #[cfg(target_os = "linux")]
    {
        [
            "/usr/bin/bwrap",
            "/bin/bwrap",
            "/usr/bin/bubblewrap",
            "/bin/bubblewrap",
        ]
        .iter()
        .any(|path| Path::new(path).exists())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn linux_allow_domains_runtime_is_available() -> bool {
    platform_backend_is_available()
        && ["/usr/bin/socat", "/bin/socat"]
            .iter()
            .any(|path| Path::new(path).exists())
}

fn workspace_policy(workspace_root: &Path) -> SandboxPolicy {
    let tool_context = ToolExecutionContext {
        workspace_root: workspace_root.to_path_buf(),
        worktree_root: Some(workspace_root.to_path_buf()),
        workspace_only: true,
        ..Default::default()
    };
    SandboxPolicy::recommended_for_context(&tool_context).with_fail_if_unavailable(true)
}

async fn run_shell_command(
    executor: &ManagedPolicyProcessExecutor,
    workspace_root: &Path,
    policy: SandboxPolicy,
    env: BTreeMap<String, String>,
    command: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut process = executor.prepare(ExecRequest {
        program: "/bin/sh".to_string(),
        args: vec!["-lc".to_string(), command.to_string()],
        cwd: Some(workspace_root.to_path_buf()),
        env,
        stdin: ProcessStdio::Null,
        stdout: ProcessStdio::Piped,
        stderr: ProcessStdio::Piped,
        kill_on_drop: true,
        origin: ExecutionOrigin::HostUtility {
            name: "sandbox-integration-test".to_string(),
        },
        runtime_scope: RuntimeScope::default(),
        sandbox_policy: policy,
    })?;
    Ok(process.output().await?)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn sandbox_backend_allows_workspace_writes() {
    if !platform_backend_is_available() {
        eprintln!("skipping sandbox integration test because no backend is available");
        return;
    }

    let workspace = tempdir().unwrap();
    let executor = ManagedPolicyProcessExecutor::new();
    let output = run_shell_command(
        &executor,
        workspace.path(),
        workspace_policy(workspace.path()),
        BTreeMap::new(),
        "touch allowed.txt",
    )
    .await
    .unwrap();

    assert!(
        output.status.success(),
        "sandboxed workspace write should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(workspace.path().join("allowed.txt").exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn sandbox_backend_blocks_protected_workspace_paths() {
    if !platform_backend_is_available() {
        eprintln!("skipping sandbox integration test because no backend is available");
        return;
    }

    let workspace = tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    let executor = ManagedPolicyProcessExecutor::new();
    let output = run_shell_command(
        &executor,
        workspace.path(),
        workspace_policy(workspace.path()),
        BTreeMap::new(),
        "touch .git/blocked.txt",
    )
    .await
    .unwrap();

    assert!(
        !output.status.success(),
        "sandboxed protected-path write should fail"
    );
    assert!(!workspace.path().join(".git/blocked.txt").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_allow_domains_requires_proxy_bridge_metadata() {
    use tools::NetworkPolicy;

    if !linux_allow_domains_runtime_is_available() {
        eprintln!("skipping sandbox integration test because no backend is available");
        return;
    }

    let workspace = tempdir().unwrap();
    let executor = ManagedPolicyProcessExecutor::new();
    let mut policy = workspace_policy(workspace.path());
    policy.network = NetworkPolicy::AllowDomains(vec!["example.com".to_string()]);

    let result =
        run_shell_command(&executor, workspace.path(), policy, BTreeMap::new(), "true").await;

    let err = result.expect_err("allow-domains without bridge metadata should fail");
    assert!(
        err.to_string()
            .contains(LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV),
        "unexpected error: {err}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_allow_domains_accepts_unix_socket_proxy_bridge_metadata() {
    use std::os::unix::net::UnixListener;
    use tools::NetworkPolicy;

    if !linux_allow_domains_runtime_is_available() {
        eprintln!("skipping sandbox integration test because no backend is available");
        return;
    }

    let workspace = tempdir().unwrap();
    let proxy_dir = tempdir().unwrap();
    let host_socket_path = proxy_dir.path().join("proxy.sock");
    let _listener = UnixListener::bind(&host_socket_path).unwrap();

    let mut policy = workspace_policy(workspace.path());
    policy.network = NetworkPolicy::AllowDomains(vec!["example.com".to_string()]);

    let mut env = BTreeMap::new();
    env.insert(
        LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV.to_string(),
        host_socket_path.display().to_string(),
    );
    env.insert(
        LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV.to_string(),
        host_socket_path.display().to_string(),
    );
    env.insert(
        LINUX_ALLOW_DOMAINS_PROXY_URL_ENV.to_string(),
        "socks5h://127.0.0.1:18080".to_string(),
    );

    let executor = ManagedPolicyProcessExecutor::new();
    let output = run_shell_command(
        &executor,
        workspace.path(),
        policy,
        env,
        "touch allowdomains.txt",
    )
    .await
    .unwrap();

    assert!(
        output.status.success(),
        "allow-domains command should run with bridge metadata: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(workspace.path().join("allowdomains.txt").exists());
}
