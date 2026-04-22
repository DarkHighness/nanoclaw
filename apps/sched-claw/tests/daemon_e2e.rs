use sched_claw::app_config::DaemonClientConfig;
use sched_claw::daemon_client::SchedExtDaemonClient;
use sched_claw::daemon_protocol::SchedExtDaemonRequest;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{Duration, Instant, sleep};

#[tokio::test]
async fn daemon_round_trips_activate_logs_and_stop() {
    let harness = DaemonHarness::start().await;
    let script = harness.write_executable(
        "loop.sh",
        r#"#!/bin/sh
trap 'echo trapped; exit 0' TERM INT
echo "started:$*"
i=0
while [ "$i" -lt 50 ]; do
  echo "tick:$i"
  i=$((i + 1))
  sleep 0.1
done
echo "completed"
"#,
    );

    let initial = harness.client.status().await.unwrap();
    assert!(initial.active.is_none());
    assert!(initial.last_exit.is_none());

    let response = harness
        .client
        .send(&SchedExtDaemonRequest::Activate {
            label: Some("loop-test".to_string()),
            argv: vec![script.display().to_string(), "--demo".to_string()],
            cwd: Some(harness.workspace_root().display().to_string()),
            env: Default::default(),
            lease_timeout_ms: None,
            replace_existing: false,
        })
        .await
        .unwrap();
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["kind"], "ack");
    assert_eq!(value["snapshot"]["active"]["label"], "loop-test");

    wait_until(Duration::from_secs(5), || async {
        let logs = harness.client.logs(Some(64)).await.unwrap();
        logs.lines
            .iter()
            .any(|line| line.line.contains("started:--demo"))
    })
    .await;

    let stopped = harness
        .client
        .send(&SchedExtDaemonRequest::Stop {
            graceful_timeout_ms: Some(2_000),
        })
        .await
        .unwrap();
    let stopped = serde_json::to_value(stopped).unwrap();
    assert!(stopped["snapshot"]["active"].is_null());
    assert_eq!(stopped["snapshot"]["last_exit"]["label"], "loop-test");

    let logs = harness.client.logs(Some(128)).await.unwrap();
    assert!(logs.lines.iter().any(|line| line.line.contains("tick:")));

    harness.shutdown().await;
}

#[tokio::test]
async fn daemon_reaps_completed_process_on_next_request() {
    let harness = DaemonHarness::start().await;
    let script = harness.write_executable(
        "exit-fast.sh",
        r#"#!/bin/sh
echo "fast-exit"
exit 0
"#,
    );

    harness
        .client
        .send(&SchedExtDaemonRequest::Activate {
            label: Some("fast-exit".to_string()),
            argv: vec![script.display().to_string()],
            cwd: Some(harness.workspace_root().display().to_string()),
            env: Default::default(),
            lease_timeout_ms: None,
            replace_existing: false,
        })
        .await
        .unwrap();

    wait_until(Duration::from_secs(5), || async {
        let status = harness.client.status().await.unwrap();
        status.active.is_none() && status.last_exit.is_some()
    })
    .await;

    let status = harness.client.status().await.unwrap();
    let last_exit = status.last_exit.unwrap();
    assert_eq!(last_exit.label, "fast-exit");
    assert_eq!(last_exit.exit_code, Some(0));
    let logs = harness.client.logs(Some(32)).await.unwrap();
    assert!(logs.lines.iter().any(|line| line.line == "fast-exit"));

    harness.shutdown().await;
}

#[tokio::test]
async fn daemon_stops_active_process_when_lease_expires() {
    let harness = DaemonHarness::start().await;
    let script = harness.write_executable(
        "lease-loop.sh",
        r#"#!/bin/sh
trap 'echo trapped; exit 0' TERM INT
echo "lease-start"
while true; do
  sleep 0.1
done
"#,
    );

    harness
        .client
        .send(&SchedExtDaemonRequest::Activate {
            label: Some("lease-test".to_string()),
            argv: vec![script.display().to_string()],
            cwd: Some(harness.workspace_root().display().to_string()),
            env: Default::default(),
            lease_timeout_ms: Some(500),
            replace_existing: false,
        })
        .await
        .unwrap();

    wait_until(Duration::from_secs(5), || async {
        let status = harness.client.status().await.unwrap();
        status.active.is_none()
            && status
                .last_exit
                .as_ref()
                .is_some_and(|snapshot| snapshot.exit_reason == "lease_expired")
    })
    .await;

    let status = harness.client.status().await.unwrap();
    let last_exit = status.last_exit.unwrap();
    assert_eq!(last_exit.label, "lease-test");
    assert_eq!(last_exit.exit_reason, "lease_expired");
    assert_eq!(last_exit.lease_timeout_ms, Some(500));

    harness.shutdown().await;
}

struct DaemonHarness {
    _workspace: TempDir,
    socket_path: PathBuf,
    child: Child,
    client: SchedExtDaemonClient,
}

impl DaemonHarness {
    async fn start() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        agent::AgentWorkspaceLayout::new(workspace.path())
            .ensure_standard_layout()
            .unwrap();
        let socket_path = workspace
            .path()
            .join(".nanoclaw/apps/sched-claw/test-daemon.sock");
        let daemon_log = workspace.path().join("daemon.log");
        let bin = std::env::var("CARGO_BIN_EXE_sched-claw-daemon")
            .expect("sched-claw-daemon test binary path");

        let child = Command::new(bin)
            .arg("serve")
            .arg("--workspace-root")
            .arg(workspace.path())
            .arg("--socket")
            .arg(&socket_path)
            .arg("--allow-root")
            .arg(workspace.path())
            .stdout(std::fs::File::create(&daemon_log).unwrap())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();

        let client = SchedExtDaemonClient::new(DaemonClientConfig {
            socket_path: socket_path.clone(),
            request_timeout_ms: 5_000,
        });
        wait_until(Duration::from_secs(5), || async {
            socket_path.exists() && client.status().await.is_ok()
        })
        .await;

        Self {
            _workspace: workspace,
            socket_path,
            child,
            client,
        }
    }

    fn workspace_root(&self) -> &Path {
        self._workspace.path()
    }

    fn write_executable(&self, relative: &str, body: &str) -> PathBuf {
        let path = self.workspace_root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    async fn shutdown(mut self) {
        let _ = self
            .client
            .send(&SchedExtDaemonRequest::Stop {
                graceful_timeout_ms: Some(1_000),
            })
            .await;
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

async fn wait_until<F, Fut>(timeout_window: Duration, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout_window;
    loop {
        if condition().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "condition did not become true within {:?}",
            timeout_window
        );
        sleep(Duration::from_millis(50)).await;
    }
}
