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
        repo_root().join("skills/sched-perf-collection/scripts/collect_sched_timeline.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/bootstrap_uv_env.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/render_perf_report.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_design_brief.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_edit_checklist.sh"),
        repo_root().join("skills/sched-policy-mapping/scripts/scaffold_policy_mapping.sh"),
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
        repo_root().join("skills/sched-perf-analysis/scripts/compose_sched_trace_evidence.py"),
        repo_root().join("skills/sched-perf-analysis/scripts/summarize_sched_latency.py"),
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
    std::fs::write(
        capture_dir.join("perf.report.txt"),
        "# header\n  60.00% busy [kernel] [k] pick_next_task\n  30.00% busy [kernel] [k] ttwu_do_wakeup\n",
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
    assert!(rendered.contains("## Derived Proxy Metrics"));
    assert!(rendered.contains("`ipc` = `2.000000`"));
    assert!(rendered.contains("## Hotspots"));
    assert!(rendered.contains("pick_next_task"));
    assert!(rendered.contains("IPC fell after migration spikes"));
    Ok(())
}

#[test]
fn analyze_perf_csv_derives_proxy_metrics() -> Result<()> {
    let dir = tempdir()?;
    let csv_path = dir.path().join("perf.stat.csv");
    std::fs::write(
        &csv_path,
        "1000,,cycles,1.0,100.00,,\n2500,,instructions,1.0,100.00,,\n10,,branches,1.0,100.00,,\n1,,branch-misses,1.0,100.00,,\n",
    )?;
    let markdown_path = dir.path().join("summary.md");
    let env_path = dir.path().join("summary.env");
    let script = repo_root().join("skills/sched-perf-analysis/scripts/analyze_perf_csv.py");

    let status = Command::new("python3")
        .arg(&script)
        .arg(&csv_path)
        .args([
            "--derive-proxies",
            "--out-markdown",
            markdown_path.to_str().unwrap(),
            "--out-env",
            env_path.to_str().unwrap(),
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "analyze helper failed");

    let markdown = std::fs::read_to_string(markdown_path)?;
    let env = std::fs::read_to_string(env_path)?;
    assert!(markdown.contains("## Derived Proxy Metrics"));
    assert!(markdown.contains("| perf.stat.csv | ipc | 2.500000 |"));
    assert!(env.contains("IPC=2.5000000000"));
    assert!(env.contains("CPI=0.4000000000"));
    assert!(env.contains("BRANCH_MISS_RATE=0.1000000000"));
    Ok(())
}

#[test]
fn summarize_sched_latency_parses_top_offenders() -> Result<()> {
    let dir = tempdir()?;
    let input = dir.path().join("perf.sched.latency.txt");
    std::fs::write(
        &input,
        "# header\nTask | Runtime ms | Switches | Average delay ms | Maximum delay ms |\nclang-1 | 120.0 | 40 | 2.5 | 9.0 |\nclang-2 | 80.0 | 20 | 1.5 | 4.0 |\n",
    )?;
    let markdown = dir.path().join("sched-latency.md");
    let json_path = dir.path().join("sched-latency.json");
    let script = repo_root().join("skills/sched-perf-analysis/scripts/summarize_sched_latency.py");

    let status = Command::new("python3")
        .arg(&script)
        .args([
            "--input",
            input.to_str().unwrap(),
            "--output",
            markdown.to_str().unwrap(),
            "--out-json",
            json_path.to_str().unwrap(),
            "--top",
            "1",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "sched latency summary helper failed");

    let rendered = std::fs::read_to_string(markdown)?;
    assert!(rendered.contains("# perf sched latency summary"));
    assert!(rendered.contains("| clang-1 | 120.000 | 40 | 2.500 | 9.000 |"));

    let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json_path)?)?;
    assert_eq!(parsed["rows"].as_array().unwrap().len(), 2);
    assert_eq!(parsed["top_offenders"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["top_offenders"][0]["task"], "clang-1");
    Ok(())
}

#[test]
fn compose_sched_trace_evidence_writes_markdown() -> Result<()> {
    let dir = tempdir()?;
    let capture_dir = dir.path().join("sched-capture");
    std::fs::create_dir_all(&capture_dir)?;
    std::fs::write(capture_dir.join("perf.sched.data"), "PERF SCHED DATA\n")?;
    std::fs::write(
        capture_dir.join("perf.sched.latency.txt"),
        "Task | Runtime ms | Switches | Average delay ms | Maximum delay ms |\nclang-1 | 120.0 | 40 | 2.5 | 9.0 |\n",
    )?;
    std::fs::write(
        capture_dir.join("perf.sched.timehist.txt"),
        "0.000 [001] clang-1 wait=0.100 sch_delay=0.200 run=1.000\n",
    )?;
    let output = dir.path().join("sched-evidence.md");
    let json_path = dir.path().join("sched-evidence.json");
    let script =
        repo_root().join("skills/sched-perf-analysis/scripts/compose_sched_trace_evidence.py");

    let status = Command::new("python3")
        .arg(&script)
        .args([
            "--capture-dir",
            capture_dir.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--out-json",
            json_path.to_str().unwrap(),
            "--fact",
            "clang-1 shows the highest wakeup delay",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(
        status.success(),
        "compose sched trace evidence helper failed"
    );

    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("## Top Delayed Tasks"));
    assert!(rendered.contains("clang-1"));
    assert!(rendered.contains("## Timehist Excerpt"));
    assert!(rendered.contains("sch_delay=0.200"));
    assert!(rendered.contains("clang-1 shows the highest wakeup delay"));

    let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json_path)?)?;
    assert_eq!(parsed["top_offenders"].as_array().unwrap().len(), 1);
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
fn edit_checklist_helper_writes_markdown() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("edit-checklist.md");
    let script = repo_root().join("skills/sched-ext-codegen/scripts/scaffold_edit_checklist.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--candidate-id",
            "cand-a",
            "--design-brief",
            "candidates/cand-a/design-brief.md",
            "--source-target",
            "candidates/cand-a/cand-a.bpf.c",
            "--touchpoint",
            "adjust wakeup CPU selection path",
            "--hook",
            "enqueue",
            "--hook",
            "dispatch",
            "--map",
            "per-task latency class map",
            "--dsq",
            "split latency-sensitive work into a dedicated DSQ",
            "--guard",
            "rollback if throughput drops more than 5%",
            "--build-command",
            "./build.sh",
            "--verify-command",
            "bpftool -d -L prog loadall cand-a.bpf.o /tmp/cand-a",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "edit checklist helper failed");

    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("# sched-ext edit checklist: cand-a"));
    assert!(rendered.contains("adjust wakeup CPU selection path"));
    assert!(rendered.contains("split latency-sensitive work into a dedicated DSQ"));
    assert!(rendered.contains("rollback if throughput drops more than 5%"));
    assert!(rendered.contains("bpftool -d -L prog loadall cand-a.bpf.o /tmp/cand-a"));
    Ok(())
}

#[test]
fn policy_mapping_helper_writes_markdown() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("policy-mapping.md");
    let script = repo_root().join("skills/sched-policy-mapping/scripts/scaffold_policy_mapping.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--objective",
            "minimize p99 latency without sacrificing throughput",
            "--evidence",
            "artifacts/evidence/sched.md",
            "--analysis",
            "artifacts/analysis/triage.md",
            "--lever",
            "favor same-cpu wakeups for the latency class",
            "--invariant",
            "background throughput must not drop more than 5%",
            "--question",
            "does migration churn fall after the wakeup change?",
            "--invalidate",
            "rollback if p95 latency or throughput regresses",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "policy mapping helper failed");

    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("# scheduler policy mapping"));
    assert!(rendered.contains("minimize p99 latency without sacrificing throughput"));
    assert!(rendered.contains("favor same-cpu wakeups for the latency class"));
    assert!(rendered.contains("does migration churn fall after the wakeup change?"));
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

#[test]
fn collect_sched_timeline_helper_supports_daemon_driver() -> Result<()> {
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
submode=""
input=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    sched)
      mode="sched"
      submode="$2"
      shift 2
      ;;
    stat|record)
      mode="$1"
      shift
      ;;
    -o|-i)
      if [ "$1" = "-o" ]; then
        output="$2"
      else
        input="$2"
      fi
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
emit() {
  mkdir -p "$(dirname "$output")"
  if [ "$mode" = "sched" ]; then
    printf 'PERF SCHED DATA\n' >"$output"
  elif [ "$mode" = "stat" ]; then
    cat >"$output" <<'OUT'
1000,,cycles,1.0,100.00,,
OUT
  else
    printf 'PERF DATA\n' >"$output"
  fi
}
if [ "$mode" = "sched" ] && [ "$submode" = "timehist" ]; then
  cat <<'OUT'
0.000 [001] worker wait=0.100 sch_delay=0.200 run=1.000
OUT
  exit 0
fi
if [ "$mode" = "sched" ] && [ "$submode" = "latency" ]; then
  cat <<'OUT'
Task | Runtime ms | Switches | Average delay ms | Maximum delay ms |
worker | 20.0 | 10 | 1.5 | 4.5 |
OUT
  exit 0
fi
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

    let script = repo_root().join("skills/sched-perf-collection/scripts/collect_sched_timeline.sh");
    let output_dir = workspace_root.join("artifacts/sched-daemon");
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
            "--latency-by-pid",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;

    let _ = busy_child.kill();
    let _ = busy_child.wait();
    let _ = daemon_child.kill();
    let _ = daemon_child.wait();

    assert!(status.success(), "daemon scheduler driver helper failed");
    assert!(output_dir.join("perf.sched.data").is_file());
    assert!(output_dir.join("perf.sched.timehist.txt").is_file());
    assert!(output_dir.join("perf.sched.latency.txt").is_file());
    assert!(
        output_dir
            .join("perf.sched.timehist.command.json")
            .is_file()
    );
    assert!(output_dir.join("perf.sched.latency.command.json").is_file());
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
