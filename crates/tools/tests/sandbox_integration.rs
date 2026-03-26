use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
#[cfg(target_os = "macos")]
use std::thread;

use tempfile::tempdir;
use tools::{
    ExecRequest, ExecutionOrigin, ManagedPolicyProcessExecutor, ProcessExecutor, ProcessStdio,
    RuntimeScope, SandboxPolicy, ToolExecutionContext, platform_sandbox_backend_available,
};

#[cfg(target_os = "linux")]
const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV: &str = "NANOCLAW_SANDBOX_PROXY_SOCKET_PATH";
#[cfg(target_os = "linux")]
const LINUX_ALLOW_DOMAINS_PROXY_SOCKET_SANDBOX_PATH_ENV: &str =
    "NANOCLAW_SANDBOX_PROXY_SOCKET_SANDBOX_PATH";
#[cfg(target_os = "linux")]
const LINUX_ALLOW_DOMAINS_PROXY_URL_ENV: &str = "NANOCLAW_SANDBOX_PROXY_URL";

fn platform_backend_is_available() -> bool {
    platform_sandbox_backend_available()
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

#[cfg(target_os = "macos")]
async fn run_direct_command(
    executor: &ManagedPolicyProcessExecutor,
    workspace_root: &Path,
    policy: SandboxPolicy,
    env: BTreeMap<String, String>,
    program: &str,
    args: Vec<String>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut process = executor.prepare(ExecRequest {
        program: program.to_string(),
        args,
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

#[cfg(target_os = "macos")]
fn spawn_http_ok_server() -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    let addr = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _peer) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .unwrap();
    });
    (addr, worker)
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

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_allow_domains_routes_http_through_local_proxy() {
    use tools::NetworkPolicy;

    if !platform_backend_is_available() {
        eprintln!("skipping sandbox integration test because no backend is available");
        return;
    }

    let workspace = tempdir().unwrap();
    let (server_addr, _worker) = spawn_http_ok_server();
    let executor = ManagedPolicyProcessExecutor::new();
    let mut policy = workspace_policy(workspace.path());
    policy.network = NetworkPolicy::AllowDomains(vec!["localhost".to_string()]);

    let output = run_direct_command(
        &executor,
        workspace.path(),
        policy,
        BTreeMap::new(),
        "/usr/bin/curl",
        vec![
            "-fsS".to_string(),
            format!("http://localhost:{}/", server_addr.port()),
        ],
    )
    .await
    .unwrap();

    assert!(
        output.status.success(),
        "macOS allow-domains curl should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_allow_domains_rejects_invalid_proxy_bridge_metadata() {
    use tools::NetworkPolicy;

    if !linux_allow_domains_runtime_is_available() {
        eprintln!("skipping sandbox integration test because no backend is available");
        return;
    }

    let workspace = tempdir().unwrap();
    let executor = ManagedPolicyProcessExecutor::new();
    let mut policy = workspace_policy(workspace.path());
    policy.network = NetworkPolicy::AllowDomains(vec!["example.com".to_string()]);
    let mut env = BTreeMap::new();
    env.insert(
        LINUX_ALLOW_DOMAINS_PROXY_SOCKET_PATH_ENV.to_string(),
        String::new(),
    );

    let result = run_shell_command(&executor, workspace.path(), policy, env, "true").await;

    let err = result.expect_err("allow-domains with invalid bridge metadata should fail");
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
