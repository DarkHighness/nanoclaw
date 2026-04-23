use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn shell_helper_scripts_have_valid_bash_syntax() -> Result<()> {
    let scripts = [
        repo_root().join("skills/sched-perf-collection/scripts/collect_perf.sh"),
        repo_root().join("skills/sched-perf-analysis/scripts/bootstrap_uv_env.sh"),
        repo_root().join("skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh"),
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
