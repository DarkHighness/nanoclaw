use std::process::Command;

use tempfile::tempdir;

fn manifest_dir() -> &'static str {
    env!("CARGO_MANIFEST_DIR")
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(manifest_dir())
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn llvm_workload_script_supports_dry_run() {
    let repo_root = repo_root();
    let script = repo_root.join("apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh");
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("llvm")).unwrap();
    std::fs::write(
        dir.path().join("llvm/CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\n",
    )
    .unwrap();

    let output = Command::new("bash")
        .arg(script)
        .arg("--llvm-src")
        .arg(dir.path())
        .arg("--dry-run")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("cmake"));
    assert!(stdout.contains("--build"));
}

#[test]
fn mysql_workload_script_supports_dry_run() {
    let repo_root = repo_root();
    let script = repo_root.join("apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh");

    let output = Command::new("bash")
        .arg(script)
        .arg("--dry-run")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sysbench"));
    assert!(stdout.contains("docker build"));
    assert!(stdout.contains("docker run"));
}

#[test]
fn llvm_demo_script_bootstraps_sched_claw_commands() {
    let repo_root = repo_root();
    let script = repo_root.join("apps/sched-claw/scripts/demos/llvm-clang-autotune.sh");
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("llvm")).unwrap();
    std::fs::write(
        dir.path().join("llvm/CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.20)\n",
    )
    .unwrap();

    let output = Command::new("bash")
        .arg(script)
        .arg("--llvm-src")
        .arg(dir.path())
        .arg("--dry-run")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sched-claw"));
    assert!(stdout.contains("workload-context.md"));
    assert!(stdout.contains("llvm-clang-build-tuning"));
}

#[test]
fn mysql_demo_script_bootstraps_sched_claw_commands() {
    let repo_root = repo_root();
    let script = repo_root.join("apps/sched-claw/scripts/demos/mysql-sysbench-autotune.sh");

    let output = Command::new("bash")
        .arg(script)
        .arg("--dry-run")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sched-claw"));
    assert!(stdout.contains("workload-context.md"));
    assert!(stdout.contains("mysql-sysbench-tuning"));
}
