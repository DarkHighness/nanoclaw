use anyhow::{Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn shell_helper_scripts_have_valid_bash_syntax() -> Result<()> {
    let scripts = [
        repo_root().join("skills/sched-perf-collection/scripts/collect_perf.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/bootstrap_uv_env.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/render_perf_report.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_design_brief.sh"),
    ];

    for script in scripts {
        let status = Command::new("bash")
            .arg("-n")
            .arg(&script)
            .status()
            .with_context(|| format!("failed to run bash -n on {}", script.display()))?;
        assert!(status.success(), "bash -n failed for {}", script.display());
    }
    Ok(())
}

#[test]
fn python_helper_scripts_compile() -> Result<()> {
    let scripts = [
        repo_root().join("skills/sched-perf-analysis/scripts/analyze_perf_csv.py"),
        repo_root().join("skills/sched-perf-analysis/scripts/compose_perf_evidence.py"),
        repo_root().join("skills/sched-perf-analysis/scripts/summarize_metrics.py"),
        repo_root().join("skills/sched-ext-run-evaluation/scripts/compare_trials.py"),
    ];

    for script in scripts {
        let status = Command::new("python3")
            .args(["-m", "py_compile"])
            .arg(&script)
            .status()
            .with_context(|| format!("failed to compile {}", script.display()))?;
        assert!(
            status.success(),
            "py_compile failed for {}",
            script.display()
        );
    }
    Ok(())
}

#[test]
fn scaffold_helper_creates_candidate_layout() -> Result<()> {
    let dir = tempdir()?;
    let script =
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh");
    let output_dir = dir.path().join("cand-a");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output_dir.to_str().unwrap(),
            "--candidate-id",
            "cand-a",
            "--experiment-id",
            "demo",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "scaffold script failed");
    assert!(output_dir.join("cand-a.bpf.c").is_file());
    assert!(output_dir.join("build.sh").is_file());
    assert!(output_dir.join("README.md").is_file());
    Ok(())
}

#[test]
fn compose_perf_evidence_helper_writes_markdown() -> Result<()> {
    let dir = tempdir()?;
    let capture_dir = dir.path().join("capture");
    std::fs::create_dir_all(&capture_dir)?;
    std::fs::write(
        capture_dir.join("perf.command.json"),
        "[\"perf\",\"stat\",\"-p\",\"42\"]\n",
    )?;
    std::fs::write(
        capture_dir.join("perf.selector.json"),
        "{\"selector\":{\"target\":\"pid\",\"pids\":[42]},\"resolved_pids\":[42]}\n",
    )?;
    std::fs::write(
        capture_dir.join("perf.stat.csv"),
        "1000,,cycles,1.0,100.00,,\n2000,,instructions,1.0,100.00,,\n",
    )?;
    let output = dir.path().join("evidence.md");
    let script = repo_root().join("skills/sched-perf-analysis/scripts/compose_perf_evidence.py");

    let status = Command::new("python3")
        .arg(&script)
        .args([
            "--capture-dir",
            capture_dir.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--fact",
            "IPC fell after migration spikes",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "compose helper failed");

    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("## Direct Facts"));
    assert!(rendered.contains("`cycles` = `1000.0`"));
    assert!(rendered.contains("IPC fell after migration spikes"));
    Ok(())
}

#[test]
fn design_brief_helper_writes_markdown() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("design.md");
    let script = repo_root().join("skills/sched-ext-codegen/scripts/scaffold_design_brief.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--candidate-id",
            "cand-a",
            "--source-target",
            "candidates/cand-a/cand-a.bpf.c",
            "--evidence",
            "evidence/perf-a.md",
            "--analysis",
            "analysis/a.md",
            "--lever",
            "stronger locality bias",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "design brief helper failed");

    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("# sched-ext design brief: cand-a"));
    assert!(rendered.contains("evidence/perf-a.md"));
    assert!(rendered.contains("stronger locality bias"));
    Ok(())
}

#[test]
fn collect_perf_helper_supports_daemon_driver() -> Result<()> {
    let dir = tempdir()?;
    let workspace_root = dir.path();
    let socket_path = workspace_root.join(".nanoclaw/apps/sched-claw/test.sock");
    let daemon_log = workspace_root.join("daemon.log");
    let perf_path = workspace_root.join("perf");
    std::fs::write(
        &perf_path,
        r#"#!/bin/sh
set -eu
mode=""
output=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    stat|record)
      mode="$1"
      shift
      ;;
    -o)
      output="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
emit() {
  mkdir -p "$(dirname "$output")"
  if [ "$mode" = "stat" ]; then
    cat >"$output" <<'OUT'
1000,,cycles,1.0,100.00,,
OUT
  else
    printf 'PERF DATA\n' >"$output"
  fi
}
trap 'emit; exit 0' INT TERM
while true; do
  sleep 0.05
done
"#,
    )?;
    let mut permissions = std::fs::metadata(&perf_path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&perf_path, permissions)?;

    let busy_path = workspace_root.join("busy.sh");
    std::fs::write(
        &busy_path,
        "#!/bin/sh\ntrap 'exit 0' TERM INT\nwhile true; do\n  :\ndone\n",
    )?;
    let mut permissions = std::fs::metadata(&busy_path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&busy_path, permissions)?;

    let mut busy_child = Command::new(&busy_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start busy target")?;

    let daemon_bin =
        std::env::var("CARGO_BIN_EXE_sched-claw-daemon").context("missing sched-claw-daemon")?;
    let sched_claw_bin = std::env::var("CARGO_BIN_EXE_sched-claw").context("missing sched-claw")?;
    let mut daemon_child = Command::new(daemon_bin)
        .arg("serve")
        .arg("--workspace-root")
        .arg(workspace_root)
        .arg("--socket")
        .arg(&socket_path)
        .arg("--allow-root")
        .arg(workspace_root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                workspace_root.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .stdout(std::fs::File::create(&daemon_log)?)
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start daemon")?;

    wait_until(Duration::from_secs(5), || socket_path.exists())?;

    let script = repo_root().join("skills/sched-perf-collection/scripts/collect_perf.sh");
    let output_dir = workspace_root.join("artifacts/perf-daemon");
    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--driver",
            "daemon",
            "--sched-claw-bin",
            &sched_claw_bin,
            "--daemon-socket",
            socket_path.to_str().unwrap(),
            "--output",
            output_dir.to_str().unwrap(),
            "--pid",
            &busy_child.id().to_string(),
            "--timeout",
            "1",
            "--event",
            "cycles",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;

    let _ = busy_child.kill();
    let _ = busy_child.wait();
    let _ = daemon_child.kill();
    let _ = daemon_child.wait();

    assert!(status.success(), "daemon driver helper failed");
    assert!(output_dir.join("perf.stat.csv").is_file());
    assert!(output_dir.join("perf.command.json").is_file());
    assert!(output_dir.join("collector.command.txt").is_file());
    Ok(())
}

fn wait_until(timeout_window: Duration, mut condition: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + timeout_window;
    loop {
        if condition() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "condition did not become true within {:?}",
            timeout_window
        );
        sleep(Duration::from_millis(50));
    }
}
