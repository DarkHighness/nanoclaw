use std::collections::BTreeMap;
use std::path::Path;

use tempfile::tempdir;
use tools::{
    ExecRequest, ExecutionOrigin, ManagedPolicyProcessExecutor, ProcessExecutor, ProcessStdio,
    RuntimeScope, SandboxPolicy, ToolExecutionContext,
};

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
    command: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut process = executor.prepare(ExecRequest {
        program: "/bin/sh".to_string(),
        args: vec!["-lc".to_string(), command.to_string()],
        cwd: Some(workspace_root.to_path_buf()),
        env: BTreeMap::new(),
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
