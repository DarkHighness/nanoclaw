use crate::experiment::{
    CandidateBuildRecord, CandidateSpec, CommandStatus, StepCommandRecord, VerifierBackend,
    VerifierCommandRecord, experiments_dir, now_unix_ms,
};
use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct BuildCaptureOptions {
    pub skip_verifier: bool,
    pub bpftool_path: String,
    pub verifier_backend: CliVerifierBackend,
}

impl Default for BuildCaptureOptions {
    fn default() -> Self {
        Self {
            skip_verifier: false,
            bpftool_path: "bpftool".to_string(),
            verifier_backend: CliVerifierBackend::BpftoolProgLoadall,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CliVerifierBackend {
    BpftoolProgLoadall,
}

impl CliVerifierBackend {
    #[must_use]
    pub fn manifest_backend(self) -> VerifierBackend {
        match self {
            Self::BpftoolProgLoadall => VerifierBackend::BpftoolProgLoadall,
        }
    }
}

pub fn capture_candidate_build(
    workspace_root: &Path,
    experiment_id: &str,
    candidate: &CandidateSpec,
    options: &BuildCaptureOptions,
) -> Result<CandidateBuildRecord> {
    let build_command = candidate.build_command.as_deref().ok_or_else(|| {
        anyhow!(
            "candidate {} does not define a build command",
            candidate.candidate_id
        )
    })?;
    let requested_at_unix_ms = now_unix_ms();
    let artifact_dir = build_attempt_dir(
        workspace_root,
        experiment_id,
        &candidate.candidate_id,
        requested_at_unix_ms,
    );
    std::fs::create_dir_all(&artifact_dir).with_context(|| {
        format!(
            "failed to create build artifact dir {}",
            artifact_dir.display()
        )
    })?;

    let source_path = candidate.source_path.clone();
    let object_path = candidate
        .object_path
        .clone()
        .or_else(|| candidate.source_path.as_deref().map(default_object_path));
    let rendered_build_command = substitute_tokens(
        build_command,
        experiment_id,
        &candidate.candidate_id,
        source_path.as_deref(),
        object_path.as_deref(),
    );
    let build = run_step(
        workspace_root,
        &artifact_dir,
        "build",
        &rendered_build_command,
        &BTreeMap::new(),
    )?;
    let build = finalize_build_step(workspace_root, object_path.as_deref(), build)?;

    let verifier = if options.skip_verifier {
        skipped_verifier_step(
            workspace_root,
            &artifact_dir,
            options.verifier_backend.manifest_backend(),
            object_path.as_deref(),
            "verification skipped by operator request",
        )?
    } else if !matches!(build.status, CommandStatus::Success) {
        skipped_verifier_step(
            workspace_root,
            &artifact_dir,
            options.verifier_backend.manifest_backend(),
            object_path.as_deref(),
            "verification skipped because build did not produce a usable object",
        )?
    } else if let Some(object_path) = object_path.as_deref() {
        let rendered_verifier_command =
            render_verifier_command(options.verifier_backend, &options.bpftool_path, object_path);
        let mut verifier = run_step(
            workspace_root,
            &artifact_dir,
            "verify",
            &rendered_verifier_command,
            &BTreeMap::new(),
        )?;
        verifier.summary = summarize_verifier_status(
            workspace_root,
            verifier.status,
            &verifier.stderr_path,
            verifier.summary.clone(),
        )?;
        VerifierCommandRecord {
            backend: options.verifier_backend.manifest_backend(),
            status: verifier.status,
            command: verifier.command,
            command_path: verifier.command_path,
            exit_code: verifier.exit_code,
            duration_ms: verifier.duration_ms,
            stdout_path: verifier.stdout_path,
            stderr_path: verifier.stderr_path,
            summary: verifier.summary,
        }
    } else {
        skipped_verifier_step(
            workspace_root,
            &artifact_dir,
            options.verifier_backend.manifest_backend(),
            None,
            "verification skipped because candidate has no object path",
        )?
    };

    Ok(CandidateBuildRecord {
        requested_at_unix_ms,
        artifact_dir: relative_path(workspace_root, &artifact_dir),
        source_path,
        object_path,
        build,
        verifier,
    })
}

fn build_attempt_dir(
    workspace_root: &Path,
    experiment_id: &str,
    candidate_id: &str,
    requested_at_unix_ms: u64,
) -> PathBuf {
    experiments_dir(workspace_root)
        .join(experiment_id)
        .join("artifacts")
        .join("builds")
        .join(candidate_id)
        .join(requested_at_unix_ms.to_string())
}

fn finalize_build_step(
    workspace_root: &Path,
    object_path: Option<&str>,
    mut build: StepCommandRecord,
) -> Result<StepCommandRecord> {
    if !matches!(build.status, CommandStatus::Success) {
        build.summary = summarize_build_status(
            workspace_root,
            build.status,
            &build.stderr_path,
            build.summary,
        )?;
        return Ok(build);
    }

    let Some(object_path) = object_path else {
        build.status = CommandStatus::Failed;
        build.summary = Some("build completed but candidate has no object path".to_string());
        append_log_line(
            &workspace_root.join(&build.stderr_path),
            "sched-claw: candidate has no object path; verifier cannot continue",
        )?;
        return Ok(build);
    };
    let object_abs = resolve_path(workspace_root, object_path);
    if !object_abs.is_file() {
        build.status = CommandStatus::Failed;
        build.summary = Some(format!(
            "build exited successfully but object path does not exist: {}",
            object_abs.display()
        ));
        append_log_line(
            &workspace_root.join(&build.stderr_path),
            &format!(
                "sched-claw: expected object artifact is missing at {}",
                object_abs.display()
            ),
        )?;
        return Ok(build);
    }

    build.summary = summarize_build_status(
        workspace_root,
        build.status,
        &build.stderr_path,
        build.summary,
    )?;
    Ok(build)
}

fn run_step(
    workspace_root: &Path,
    artifact_dir: &Path,
    step_name: &str,
    command: &str,
    env: &BTreeMap<String, String>,
) -> Result<StepCommandRecord> {
    let command_path = artifact_dir.join(format!("{step_name}.command.txt"));
    let stdout_path = artifact_dir.join(format!("{step_name}.stdout.log"));
    let stderr_path = artifact_dir.join(format!("{step_name}.stderr.log"));
    write_command_capture(&command_path, workspace_root, env, command)?;

    let start = Instant::now();
    let status = {
        let stdout = File::create(&stdout_path)
            .with_context(|| format!("failed to create {}", stdout_path.display()))?;
        let stderr = File::create(&stderr_path)
            .with_context(|| format!("failed to create {}", stderr_path.display()))?;
        let mut child = Command::new("sh");
        child
            .arg("-lc")
            .arg(command)
            .current_dir(workspace_root)
            .envs(env)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        match child.status() {
            Ok(status) => Ok(status),
            Err(error) => Err(error),
        }
    };
    let duration_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

    let (status, exit_code, summary) = match status {
        Ok(status) if status.success() => (CommandStatus::Success, status.code(), None),
        Ok(status) => (
            CommandStatus::Failed,
            status.code(),
            Some(format!(
                "command exited unsuccessfully with code {}",
                status
                    .code()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<signal>".to_string())
            )),
        ),
        Err(error) => {
            append_log_line(
                &stderr_path,
                &format!("sched-claw: failed to spawn command: {error}"),
            )?;
            (
                CommandStatus::Failed,
                None,
                Some(format!("failed to spawn command: {error}")),
            )
        }
    };

    Ok(StepCommandRecord {
        status,
        command: command.to_string(),
        command_path: relative_path(workspace_root, &command_path),
        exit_code,
        duration_ms,
        stdout_path: relative_path(workspace_root, &stdout_path),
        stderr_path: relative_path(workspace_root, &stderr_path),
        summary,
    })
}

fn skipped_verifier_step(
    workspace_root: &Path,
    artifact_dir: &Path,
    backend: VerifierBackend,
    object_path: Option<&str>,
    reason: &str,
) -> Result<VerifierCommandRecord> {
    let command = object_path
        .map(|path| {
            render_verifier_command(CliVerifierBackend::BpftoolProgLoadall, "bpftool", path)
        })
        .unwrap_or_else(|| "<no verifier command>".to_string());
    let command_path = artifact_dir.join("verify.command.txt");
    let stdout_path = artifact_dir.join("verify.stdout.log");
    let stderr_path = artifact_dir.join("verify.stderr.log");
    write_command_capture(&command_path, workspace_root, &BTreeMap::new(), &command)?;
    std::fs::write(&stdout_path, "")
        .with_context(|| format!("failed to write {}", stdout_path.display()))?;
    std::fs::write(&stderr_path, format!("sched-claw: {reason}\n"))
        .with_context(|| format!("failed to write {}", stderr_path.display()))?;
    Ok(VerifierCommandRecord {
        backend,
        status: CommandStatus::Skipped,
        command,
        command_path: relative_path(workspace_root, &command_path),
        exit_code: None,
        duration_ms: 0,
        stdout_path: relative_path(workspace_root, &stdout_path),
        stderr_path: relative_path(workspace_root, &stderr_path),
        summary: Some(reason.to_string()),
    })
}

fn write_command_capture(
    path: &Path,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    command: &str,
) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    writeln!(file, "# cwd: {}", cwd.display())
        .with_context(|| format!("failed to write {}", path.display()))?;
    if env.is_empty() {
        writeln!(file, "# env: <inherit>")
            .with_context(|| format!("failed to write {}", path.display()))?;
    } else {
        for (key, value) in env {
            writeln!(file, "# env: {key}={value}")
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
    }
    writeln!(file, "{command}").with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn render_verifier_command(
    backend: CliVerifierBackend,
    bpftool_path: &str,
    object_path: &str,
) -> String {
    match backend {
        // `bpftool -d -L prog loadall` loads the object through libbpf and the kernel
        // verifier without pinning programs, which keeps this host-local probe from
        // silently leaving persistent bpffs state behind on success.
        CliVerifierBackend::BpftoolProgLoadall => {
            format!("{bpftool_path} -d -L prog loadall {object_path}")
        }
    }
}

fn substitute_tokens(
    value: &str,
    experiment_id: &str,
    candidate_id: &str,
    source_path: Option<&str>,
    object_path: Option<&str>,
) -> String {
    let mut rendered = value.replace("{experiment}", experiment_id);
    rendered = rendered.replace("{candidate}", candidate_id);
    if let Some(source_path) = source_path {
        rendered = rendered.replace("{source}", source_path);
    }
    if let Some(object_path) = object_path {
        rendered = rendered.replace("{object}", object_path);
    }
    rendered
}

fn default_object_path(source_path: &str) -> String {
    if let Some(stripped) = source_path.strip_suffix(".bpf.c") {
        format!("{stripped}.bpf.o")
    } else if let Some(stripped) = source_path.strip_suffix(".c") {
        format!("{stripped}.o")
    } else {
        format!("{source_path}.o")
    }
}

fn append_log_line(path: &Path, line: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("failed to append {}", path.display()))?;
    Ok(())
}

fn summarize_build_status(
    workspace_root: &Path,
    status: CommandStatus,
    stderr_path: &str,
    fallback: Option<String>,
) -> Result<Option<String>> {
    if matches!(status, CommandStatus::Success) {
        return Ok(Some("build completed successfully".to_string()));
    }
    summarize_log(
        &resolve_path(workspace_root, stderr_path),
        &[
            "fatal error:",
            "error:",
            "undefined reference",
            "not found",
            "No such file",
        ],
        fallback,
    )
}

fn summarize_verifier_status(
    workspace_root: &Path,
    status: CommandStatus,
    stderr_path: &str,
    fallback: Option<String>,
) -> Result<Option<String>> {
    if matches!(status, CommandStatus::Success) {
        return Ok(Some("verifier accepted the object".to_string()));
    }
    summarize_log(
        &resolve_path(workspace_root, stderr_path),
        &[
            "Permission denied",
            "Operation not permitted",
            "libbpf:",
            "verifier",
            "error:",
            "Error:",
            "failed",
            "not found",
        ],
        fallback,
    )
}

fn summarize_log(
    path: &Path,
    patterns: &[&str],
    fallback: Option<String>,
) -> Result<Option<String>> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    for pattern in patterns {
        if let Some(line) = raw.lines().find(|line| line.contains(pattern)) {
            return Ok(Some(line.trim().to_string()));
        }
    }
    if let Some(line) = raw.lines().rev().find(|line| !line.trim().is_empty()) {
        return Ok(Some(line.trim().to_string()));
    }
    Ok(fallback)
}

fn resolve_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

fn relative_path(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{BuildCaptureOptions, capture_candidate_build};
    use crate::experiment::{CandidateSpec, CommandStatus};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn captures_build_and_verifier_failure_artifacts() {
        let dir = tempdir().unwrap();
        let workspace_root = dir.path();
        let tools_dir = workspace_root.join("tools");
        std::fs::create_dir_all(&tools_dir).unwrap();
        let compiler = tools_dir.join("fake-clang.sh");
        let bpftool = tools_dir.join("fake-bpftool.sh");
        std::fs::write(
            &compiler,
            "#!/usr/bin/env bash\nset -euo pipefail\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then\n    shift\n    out=\"$1\"\n  fi\n  shift || true\ndone\nprintf 'fake compiler stdout\\n'\nprintf 'fake compiler stderr\\n' >&2\nmkdir -p \"$(dirname \"$out\")\"\nprintf 'object' >\"$out\"\n",
        )
        .unwrap();
        std::fs::write(
            &bpftool,
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'bpftool debug\\n' >&2\nprintf 'libbpf: verifier rejected fake object\\n' >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(&bpftool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let source_path = ".nanoclaw/apps/sched-claw/experiments/demo/sources/cand-a.bpf.c";
        let source_abs = workspace_root.join(source_path);
        std::fs::create_dir_all(source_abs.parent().unwrap()).unwrap();
        std::fs::write(&source_abs, "int x;").unwrap();

        let candidate = CandidateSpec {
            candidate_id: "cand-a".to_string(),
            template: "latency_guard".to_string(),
            source_path: Some(source_path.to_string()),
            object_path: Some(
                ".nanoclaw/apps/sched-claw/experiments/demo/sources/cand-a.bpf.o".to_string(),
            ),
            build_command: Some(format!(
                "{} -c {{source}} -o {{object}}",
                compiler.display()
            )),
            daemon_argv: Vec::new(),
            daemon_cwd: None,
            daemon_env: BTreeMap::new(),
            knobs: BTreeMap::new(),
            notes: None,
        };
        let record = capture_candidate_build(
            workspace_root,
            "demo",
            &candidate,
            &BuildCaptureOptions {
                skip_verifier: false,
                bpftool_path: bpftool.display().to_string(),
                ..BuildCaptureOptions::default()
            },
        )
        .unwrap();

        assert_eq!(record.build.status, CommandStatus::Success);
        assert_eq!(record.verifier.status, CommandStatus::Failed);
        assert!(
            record
                .verifier
                .summary
                .as_deref()
                .unwrap_or_default()
                .contains("libbpf")
        );
        assert!(workspace_root.join(&record.build.command_path).is_file());
        assert!(workspace_root.join(&record.verifier.stderr_path).is_file());
    }

    #[test]
    fn skips_verifier_when_build_fails() {
        let dir = tempdir().unwrap();
        let workspace_root = dir.path();
        let source_path = ".nanoclaw/apps/sched-claw/experiments/demo/sources/cand-a.bpf.c";
        let source_abs = workspace_root.join(source_path);
        std::fs::create_dir_all(source_abs.parent().unwrap()).unwrap();
        std::fs::write(&source_abs, "int x;").unwrap();

        let candidate = CandidateSpec {
            candidate_id: "cand-a".to_string(),
            template: "latency_guard".to_string(),
            source_path: Some(source_path.to_string()),
            object_path: Some(
                ".nanoclaw/apps/sched-claw/experiments/demo/sources/cand-a.bpf.o".to_string(),
            ),
            build_command: Some("missing-compiler -c {source} -o {object}".to_string()),
            daemon_argv: Vec::new(),
            daemon_cwd: None,
            daemon_env: BTreeMap::new(),
            knobs: BTreeMap::new(),
            notes: None,
        };
        let record = capture_candidate_build(
            workspace_root,
            "demo",
            &candidate,
            &BuildCaptureOptions::default(),
        )
        .unwrap();

        assert_eq!(record.build.status, CommandStatus::Failed);
        assert_eq!(record.verifier.status, CommandStatus::Skipped);
        assert!(
            record
                .build
                .summary
                .as_deref()
                .unwrap_or_default()
                .contains("not found")
        );
    }
}
