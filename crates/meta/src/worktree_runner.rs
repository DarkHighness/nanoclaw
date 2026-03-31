use crate::git_gate::{capture_worktree_diff, create_detached_worktree, remove_worktree};
use crate::runner_trace::{
    WorktreeCommandSpec, WorktreeCommandStatus, WorktreeCommandTrace, WorktreeMutation,
    WorktreeRunTrace,
};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tokio::process::Command;
use types::{ArtifactId, ArtifactVersion, new_opaque_id};

#[derive(Debug, Error)]
pub enum WorktreeRunnerError {
    #[error("invalid worktree run plan: {0}")]
    InvalidPlan(String),
    #[error("worktree IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed in {cwd:?} ({args:?}) exit={exit_code:?}: {stderr}")]
    GitCommandFailed {
        cwd: PathBuf,
        args: Vec<String>,
        exit_code: Option<i32>,
        stderr: String,
    },
}

pub type WorktreeRunnerResult<T> = std::result::Result<T, WorktreeRunnerError>;

#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeRunPlan {
    pub repo_root: PathBuf,
    pub artifact_id: ArtifactId,
    pub version: ArtifactVersion,
    pub baseline_ref: String,
    pub mutations: Vec<WorktreeMutation>,
    pub commands: Vec<WorktreeCommandSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeRunOutcome {
    pub succeeded: bool,
    pub trace: WorktreeRunTrace,
}

#[derive(Clone, Default)]
pub struct WorktreeRunner;

impl WorktreeRunner {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub async fn run(&self, plan: WorktreeRunPlan) -> WorktreeRunnerResult<WorktreeRunOutcome> {
        validate_plan(&plan)?;
        let worktree_path = plan
            .repo_root
            .join(".nanoclaw")
            .join("meta-worktrees")
            .join(format!(
                "{}-{}-{}",
                plan.artifact_id,
                plan.version.version_id,
                new_opaque_id()
            ));
        if let Some(parent) = worktree_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        create_detached_worktree(&plan.repo_root, &worktree_path, &plan.baseline_ref).await?;
        let evaluation = run_in_worktree(&plan, &worktree_path).await;
        let cleanup = remove_worktree(&plan.repo_root, &worktree_path).await;
        match (evaluation, cleanup) {
            (Ok(mut outcome), Ok(())) => {
                outcome.trace.cleanup_performed = true;
                Ok(outcome)
            }
            // Cleanup failure is more important than preserving the evaluation
            // result because a leaked worktree breaks the isolation contract for
            // future self-edit runs.
            (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(error),
            (Err(error), Ok(())) => Err(error),
        }
    }
}

async fn run_in_worktree(
    plan: &WorktreeRunPlan,
    worktree_path: &Path,
) -> WorktreeRunnerResult<WorktreeRunOutcome> {
    for mutation in &plan.mutations {
        apply_mutation(worktree_path, mutation).await?;
    }

    let mut traces = Vec::new();
    let mut succeeded = true;
    for command in &plan.commands {
        let trace = run_command(worktree_path, command).await?;
        if trace.status == WorktreeCommandStatus::Failed {
            succeeded = false;
        }
        traces.push(trace);
    }

    let git_diff = capture_worktree_diff(worktree_path).await?;
    Ok(WorktreeRunOutcome {
        succeeded,
        trace: WorktreeRunTrace {
            artifact_id: plan.artifact_id.clone(),
            version_id: plan.version.version_id.clone(),
            artifact_kind: plan.version.kind,
            baseline_ref: plan.baseline_ref.clone(),
            worktree_path: worktree_path.to_path_buf(),
            mutations: plan.mutations.clone(),
            commands: traces,
            git_diff,
            cleanup_performed: false,
        },
    })
}

async fn apply_mutation(
    worktree_path: &Path,
    mutation: &WorktreeMutation,
) -> WorktreeRunnerResult<()> {
    match mutation {
        // Mutations must stay relative to the isolated worktree so a failed
        // candidate can never escape into the main checkout during evaluation.
        WorktreeMutation::WriteFile {
            relative_path,
            content,
        } => {
            let target = resolve_relative_path(worktree_path, relative_path)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(target, content).await?;
        }
        WorktreeMutation::RemoveFile { relative_path } => {
            let target = resolve_relative_path(worktree_path, relative_path)?;
            match fs::remove_file(target).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn resolve_relative_path(
    worktree_path: &Path,
    relative_path: &Path,
) -> WorktreeRunnerResult<PathBuf> {
    if relative_path.is_absolute() {
        return Err(WorktreeRunnerError::InvalidPlan(format!(
            "absolute mutation paths are not allowed: {}",
            relative_path.display()
        )));
    }
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(WorktreeRunnerError::InvalidPlan(format!(
            "parent/root mutation paths are not allowed: {}",
            relative_path.display()
        )));
    }
    Ok(worktree_path.join(relative_path))
}

async fn run_command(
    worktree_path: &Path,
    spec: &WorktreeCommandSpec,
) -> WorktreeRunnerResult<WorktreeCommandTrace> {
    let (program, args) = spec.argv.split_first().ok_or_else(|| {
        WorktreeRunnerError::InvalidPlan("worktree command argv must not be empty".to_string())
    })?;
    let output = Command::new(program)
        .current_dir(worktree_path)
        .args(args)
        .envs(spec.env.clone())
        .output()
        .await?;
    Ok(WorktreeCommandTrace {
        argv: spec.argv.clone(),
        status: if output.status.success() {
            WorktreeCommandStatus::Succeeded
        } else {
            WorktreeCommandStatus::Failed
        },
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn validate_plan(plan: &WorktreeRunPlan) -> WorktreeRunnerResult<()> {
    if plan.baseline_ref.trim().is_empty() {
        return Err(WorktreeRunnerError::InvalidPlan(
            "baseline_ref must not be empty".to_string(),
        ));
    }
    if !plan.repo_root.join(".git").exists() {
        return Err(WorktreeRunnerError::InvalidPlan(format!(
            "repo_root is not a git repository: {}",
            plan.repo_root.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{WorktreeRunPlan, WorktreeRunner};
    use crate::runner_trace::{WorktreeCommandSpec, WorktreeMutation};
    use nanoclaw_test_support::run_current_thread_test;
    use std::path::Path;
    use std::process::Command as StdCommand;
    use tempfile::tempdir;
    use types::{ArtifactId, ArtifactKind, ArtifactVersion, ArtifactVersionId};

    fn init_git_repo(path: &Path) {
        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "ci@example.com"]);
        run_git(path, &["config", "user.name", "CI"]);
        std::fs::write(path.join("prompt.txt"), "baseline\n").unwrap();
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", "initial"]);
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = StdCommand::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn plan(repo_root: &Path) -> WorktreeRunPlan {
        WorktreeRunPlan {
            repo_root: repo_root.to_path_buf(),
            artifact_id: ArtifactId::from("artifact-runner"),
            version: ArtifactVersion {
                version_id: ArtifactVersionId::from("artifact-runner-v2"),
                kind: ArtifactKind::Prompt,
                label: "prompt-v2".to_string(),
                description: Some("tighten retries".to_string()),
                parent_version_id: Some(ArtifactVersionId::from("artifact-runner-v1")),
                source_signal_ids: vec![],
                source_task_ids: vec![],
                source_case_ids: vec![],
                payload: serde_json::json!({"prompt":"candidate"}),
                metadata: serde_json::json!({"owner":"nanoclaw"}),
            },
            baseline_ref: "HEAD".to_string(),
            mutations: vec![WorktreeMutation::WriteFile {
                relative_path: "prompt.txt".into(),
                content: "candidate\n".to_string(),
            }],
            commands: vec![WorktreeCommandSpec {
                argv: vec![
                    "sh".to_string(),
                    "-lc".to_string(),
                    "test \"$(cat prompt.txt)\" = candidate".to_string(),
                ],
                env: Default::default(),
            }],
        }
    }

    #[test]
    fn runner_executes_in_isolated_worktree_and_cleans_up() {
        run_current_thread_test(async {
            let dir = tempdir().unwrap();
            init_git_repo(dir.path());

            let runner = WorktreeRunner::new();
            let outcome = runner.run(plan(dir.path())).await.unwrap();

            assert!(outcome.succeeded);
            assert!(outcome.trace.cleanup_performed);
            assert!(outcome.trace.git_diff.contains("prompt.txt"));
            assert_eq!(
                std::fs::read_to_string(dir.path().join("prompt.txt")).unwrap(),
                "baseline\n"
            );
            assert!(!outcome.trace.worktree_path.exists());
        });
    }

    #[test]
    fn runner_keeps_failed_command_inside_trace() {
        run_current_thread_test(async {
            let dir = tempdir().unwrap();
            init_git_repo(dir.path());

            let mut plan = plan(dir.path());
            plan.commands = vec![WorktreeCommandSpec {
                argv: vec!["sh".to_string(), "-lc".to_string(), "exit 7".to_string()],
                env: Default::default(),
            }];

            let runner = WorktreeRunner::new();
            let outcome = runner.run(plan).await.unwrap();

            assert!(!outcome.succeeded);
            assert!(outcome.trace.cleanup_performed);
            assert_eq!(outcome.trace.commands.len(), 1);
            assert_eq!(outcome.trace.commands[0].exit_code, Some(7));
            assert!(!outcome.trace.worktree_path.exists());
        });
    }
}
