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
        repo_root().join("skills/sched-perf-collection/scripts/collect_sched_state.sh"),
        repo_root().join("skills/sched-perf-collection/scripts/collect_pressure_snapshot.sh"),
        repo_root().join("skills/sched-perf-collection/scripts/collect_topology_snapshot.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/bootstrap_uv_env.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/render_perf_report.sh"),
        repo_root().join("skills/sched-workload-contract/scripts/scaffold_workload_contract.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_design_brief.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_edit_checklist.sh"),
        repo_root()
            .join("skills/sched-ext-build-verify/scripts/capture_build_verifier_artifacts.sh"),
        repo_root().join("skills/sched-ext-rollout-safety/scripts/scaffold_rollout_plan.sh"),
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
        repo_root().join("skills/sched-workload-contract/scripts/validate_workload_contract.py"),
        repo_root().join("skills/sched-ext-build-verify/scripts/summarize_build_verifier.py"),
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
fn collect_sched_state_helper_captures_proc_artifacts() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("state");
    let script = repo_root().join("skills/sched-perf-collection/scripts/collect_sched_state.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--pid",
            &std::process::id().to_string(),
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "sched state helper failed");
    assert!(output.join("proc.schedstat").is_file());
    assert!(output.join("collector.command.txt").is_file());
    assert!(output.join("selector.txt").is_file());
    Ok(())
}

#[test]
fn collect_pressure_snapshot_helper_captures_pressure_artifacts() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("pressure");
    let script =
        repo_root().join("skills/sched-perf-collection/scripts/collect_pressure_snapshot.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--pid",
            &std::process::id().to_string(),
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "pressure helper failed");
    assert!(output.join("proc.pressure.cpu").is_file());
    assert!(output.join("collector.command.txt").is_file());
    assert!(output.join("selector.txt").is_file());
    Ok(())
}

#[test]
fn collect_topology_snapshot_helper_captures_topology_artifacts() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("topology");
    let script =
        repo_root().join("skills/sched-perf-collection/scripts/collect_topology_snapshot.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args(["--output", output.to_str().unwrap()])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "topology helper failed");
    assert!(output.join("sys.cpu.online").is_file() || output.join("sys.cpu.possible").is_file());
    assert!(output.join("collector.command.txt").is_file());
    assert!(output.join("selector.txt").is_file());
    Ok(())
}

#[test]
fn scaffold_workload_contract_helper_writes_toml() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("contract.toml");
    let script =
        repo_root().join("skills/sched-workload-contract/scripts/scaffold_workload_contract.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--workload",
            "llvm",
            "--selector-kind",
            "script",
            "--selector-value",
            "scripts/workloads/run-llvm-clang-build.sh",
            "--primary-metric",
            "wall_time_s",
            "--primary-goal",
            "minimize",
            "--basis",
            "direct",
            "--guardrail",
            "throughput:maximize:5",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "workload contract helper failed");
    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("name = \"llvm\""));
    assert!(rendered.contains("selector_kind = \"script\""));
    assert!(rendered.contains("guardrails = [\"throughput:maximize:5\"]"));
    Ok(())
}

#[test]
fn validate_workload_contract_helper_accepts_valid_contract() -> Result<()> {
    let dir = tempdir()?;
    let contract = dir.path().join("contract.toml");
    std::fs::write(
        &contract,
        r#"
name = "llvm"
selector_kind = "script"
selector_value = "scripts/workloads/run-llvm-clang-build.sh"
primary_metric = "build_seconds"
primary_goal = "minimize"
performance_basis = "direct"
guardrails = ["throughput:maximize:5"]
proxy_metrics = ["ipc:maximize"]
"#,
    )?;
    let json_path = dir.path().join("contract.json");
    let markdown = dir.path().join("contract.md");
    let script =
        repo_root().join("skills/sched-workload-contract/scripts/validate_workload_contract.py");

    let status = Command::new("python3")
        .arg(&script)
        .args([
            contract.to_str().unwrap(),
            "--out-json",
            json_path.to_str().unwrap(),
            "--out-markdown",
            markdown.to_str().unwrap(),
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(
        status.success(),
        "workload contract validation helper failed"
    );

    let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json_path)?)?;
    assert_eq!(parsed["status"], "valid");
    let rendered = std::fs::read_to_string(markdown)?;
    assert!(rendered.contains("# workload contract validation"));
    assert!(rendered.contains("selector: `script:scripts/workloads/run-llvm-clang-build.sh`"));
    Ok(())
}

#[test]
fn capture_build_verifier_artifacts_helper_captures_status() -> Result<()> {
    let dir = tempdir()?;
    let script = repo_root()
        .join("skills/sched-ext-build-verify/scripts/capture_build_verifier_artifacts.sh");
    let artifacts = dir.path().join("artifacts");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--artifact-dir",
            artifacts.to_str().unwrap(),
            "--source",
            "cand-a.bpf.c",
            "--object",
            "cand-a.bpf.o",
            "--build-command",
            "printf build-ok",
            "--verify-command",
            "printf verify-ok",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "build verifier helper failed");
    assert!(artifacts.join("build.command.txt").is_file());
    assert!(artifacts.join("build.stdout.log").is_file());
    assert!(artifacts.join("verify.command.txt").is_file());
    let summary = std::fs::read_to_string(artifacts.join("summary.env"))?;
    assert!(summary.contains("build_status=0"));
    assert!(summary.contains("verify_status=0"));
    Ok(())
}

#[test]
fn summarize_build_verifier_helper_classifies_failure() -> Result<()> {
    let dir = tempdir()?;
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts)?;
    std::fs::write(
        artifacts.join("context.txt"),
        "source=cand-a.bpf.c\nobject=cand-a.bpf.o\n",
    )?;
    std::fs::write(
        artifacts.join("summary.env"),
        "build_status=0\nverify_status=1\n",
    )?;
    std::fs::write(
        artifacts.join("verify.stderr.log"),
        "libbpf: failed to load object file\nverifier rejected program due to invalid access\n",
    )?;
    let json_path = dir.path().join("summary.json");
    let markdown = dir.path().join("summary.md");
    let script =
        repo_root().join("skills/sched-ext-build-verify/scripts/summarize_build_verifier.py");

    let status = Command::new("python3")
        .arg(&script)
        .args([
            artifacts.to_str().unwrap(),
            "--out-json",
            json_path.to_str().unwrap(),
            "--out-markdown",
            markdown.to_str().unwrap(),
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "build verifier summary helper failed");

    let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json_path)?)?;
    assert_eq!(parsed["overall_status"], "failed");
    assert_eq!(parsed["classification"], "libbpf-load-failure");
    let rendered = std::fs::read_to_string(markdown)?;
    assert!(rendered.contains("## Verify Excerpt"));
    assert!(rendered.contains("libbpf"));
    Ok(())
}

#[test]
fn scaffold_rollout_plan_helper_writes_markdown() -> Result<()> {
    let dir = tempdir()?;
    let output = dir.path().join("rollout.md");
    let script =
        repo_root().join("skills/sched-ext-rollout-safety/scripts/scaffold_rollout_plan.sh");

    let status = Command::new("bash")
        .arg(&script)
        .args([
            "--output",
            output.to_str().unwrap(),
            "--candidate",
            "cand-a",
            "--lease-seconds",
            "30",
            "--rollback-trigger",
            "p95 latency > baseline + 10%",
            "--guardrail",
            "throughput must not regress",
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "rollout plan helper failed");
    let rendered = std::fs::read_to_string(output)?;
    assert!(rendered.contains("# rollout plan: cand-a"));
    assert!(rendered.contains("lease: `30s`"));
    assert!(rendered.contains("throughput must not regress"));
    Ok(())
}

#[test]
fn compare_trials_helper_supports_direct_metrics_files() -> Result<()> {
    let dir = tempdir()?;
    let baseline_a = dir.path().join("baseline-a.env");
    let baseline_b = dir.path().join("baseline-b.env");
    let candidate_a = dir.path().join("candidate-a.env");
    let candidate_b = dir.path().join("candidate-b.env");
    std::fs::write(&baseline_a, "build_seconds=10\n")?;
    std::fs::write(&baseline_b, "build_seconds=10\n")?;
    std::fs::write(&candidate_a, "build_seconds=8\n")?;
    std::fs::write(&candidate_b, "build_seconds=8\n")?;
    let json_path = dir.path().join("compare.json");
    let markdown = dir.path().join("compare.md");
    let script = repo_root().join("skills/sched-ext-run-evaluation/scripts/compare_trials.py");

    let status = Command::new("python3")
        .arg(&script)
        .args([
            "--candidate-id",
            "cand-a",
            "--baseline-file",
            baseline_a.to_str().unwrap(),
            "--baseline-file",
            baseline_b.to_str().unwrap(),
            "--candidate-file",
            candidate_a.to_str().unwrap(),
            "--candidate-file",
            candidate_b.to_str().unwrap(),
            "--metric",
            "build_seconds",
            "--goal",
            "minimize",
            "--out-json",
            json_path.to_str().unwrap(),
            "--out-markdown",
            markdown.to_str().unwrap(),
        ])
        .status()
        .with_context(|| format!("failed to run {}", script.display()))?;
    assert!(status.success(), "compare trials helper failed");

    let parsed: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(json_path)?)?;
    assert_eq!(parsed["mode"], "direct-files");
    assert_eq!(parsed["baseline_count"], 2);
    assert_eq!(parsed["candidate_count"], 2);
    assert_eq!(parsed["improvement_pct"], 20.0);
    let rendered = std::fs::read_to_string(markdown)?;
    assert!(rendered.contains("# trial comparison: cand-a"));
    assert!(rendered.contains("improvement_pct: `20.00`"));
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

    let mut busy_child = Command::new("/bin/sh")
        .arg(&busy_path)
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

    let mut busy_child = Command::new("/bin/sh")
        .arg(&busy_path)
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
