use crate::worktree_runner::{WorktreeRunnerError, WorktreeRunnerResult};
use std::path::Path;
use tokio::process::Command;

pub async fn create_detached_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    baseline_ref: &str,
) -> WorktreeRunnerResult<()> {
    run_git(
        repo_root,
        &[
            "worktree",
            "add",
            "--detach",
            worktree_path.to_string_lossy().as_ref(),
            baseline_ref,
        ],
    )
    .await
    .map(|_| ())
}

pub async fn remove_worktree(repo_root: &Path, worktree_path: &Path) -> WorktreeRunnerResult<()> {
    run_git(
        repo_root,
        &[
            "worktree",
            "remove",
            "--force",
            worktree_path.to_string_lossy().as_ref(),
        ],
    )
    .await
    .map(|_| ())
}

pub async fn capture_worktree_diff(worktree_path: &Path) -> WorktreeRunnerResult<String> {
    run_git(
        worktree_path,
        &[
            "diff",
            "--no-ext-diff",
            "--stat",
            "--patch",
            "--find-renames",
        ],
    )
    .await
}

pub async fn capture_worktree_changed_paths(
    worktree_path: &Path,
) -> WorktreeRunnerResult<Vec<std::path::PathBuf>> {
    let output = run_git(
        worktree_path,
        &["diff", "--name-only", "--find-renames", "-z"],
    )
    .await?;
    Ok(output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(std::path::PathBuf::from)
        .collect())
}

async fn run_git(cwd: &Path, args: &[&str]) -> WorktreeRunnerResult<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        return Err(WorktreeRunnerError::GitCommandFailed {
            cwd: cwd.to_path_buf(),
            args: args.iter().map(|value| (*value).to_string()).collect(),
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
