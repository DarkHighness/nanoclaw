use crate::app_config::app_state_dir;
use anyhow::{Context, Result, anyhow};
use include_dir::{Dir, DirEntry, include_dir};
use std::path::{Path, PathBuf};

static BUILTIN_SKILLS_SOURCE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills");

pub fn builtin_skill_root(workspace_root: &Path) -> PathBuf {
    app_state_dir(workspace_root).join("builtin-skills")
}

pub fn materialize_builtin_skills(workspace_root: &Path) -> Result<PathBuf> {
    let root = builtin_skill_root(workspace_root);
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("failed to reset {}", root.display()))?;
    }
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create {}", root.display()))?;
    write_embedded_skill_dir(&BUILTIN_SKILLS_SOURCE, &root)?;
    Ok(root)
}

fn write_embedded_skill_dir(dir: &Dir<'_>, destination: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(child) => {
                let child_destination = destination.join(
                    child
                        .path()
                        .file_name()
                        .ok_or_else(|| anyhow!("missing embedded skill directory name"))?,
                );
                std::fs::create_dir_all(&child_destination)
                    .with_context(|| format!("failed to create {}", child_destination.display()))?;
                write_embedded_skill_dir(child, &child_destination)?;
            }
            DirEntry::File(file) => {
                let file_destination = destination.join(
                    file.path()
                        .file_name()
                        .ok_or_else(|| anyhow!("missing embedded skill file name"))?,
                );
                std::fs::write(&file_destination, file.contents())
                    .with_context(|| format!("failed to write {}", file_destination.display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{builtin_skill_root, materialize_builtin_skills};
    use tempfile::tempdir;

    #[test]
    fn materializes_embedded_skill_bundle() {
        let dir = tempdir().unwrap();
        let root = materialize_builtin_skills(dir.path()).unwrap();

        assert_eq!(root, builtin_skill_root(dir.path()));
        assert!(root.join("linux-scheduler-triage/SKILL.md").is_file());
        assert!(root.join("llvm-clang-build-tuning/SKILL.md").is_file());
        assert!(root.join("mysql-sysbench-tuning/SKILL.md").is_file());
        assert!(root.join("sched-claw-product-readiness/SKILL.md").is_file());
        assert!(root.join("sched-ext-codegen/SKILL.md").is_file());
        assert!(root.join("sched-ext-build-verify/SKILL.md").is_file());
        assert!(root.join("sched-ext-design-loop/SKILL.md").is_file());
        assert!(root.join("sched-ext-rollout-safety/SKILL.md").is_file());
        assert!(root.join("sched-ext-run-evaluation/SKILL.md").is_file());
        assert!(root.join("sched-perf-analysis/SKILL.md").is_file());
        assert!(root.join("sched-perf-collection/SKILL.md").is_file());
        assert!(root.join("sched-workload-contract/SKILL.md").is_file());
        assert!(
            root.join("linux-scheduler-triage/references/official-docs.md")
                .is_file()
        );
        assert!(
            root.join("llvm-clang-build-tuning/references/demo-contract.md")
                .is_file()
        );
        assert!(
            root.join("mysql-sysbench-tuning/references/demo-contract.md")
                .is_file()
        );
        assert!(
            root.join("sched-claw-product-readiness/references/readiness-matrix.md")
                .is_file()
        );
        assert!(
            root.join("sched-claw-product-readiness/references/harness-engineering.md")
                .is_file()
        );
        assert!(
            root.join("sched-ext-codegen/references/codegen-levers.md")
                .is_file()
        );
        assert!(
            root.join("sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh")
                .is_file()
        );
        assert!(
            root.join("sched-ext-codegen/scripts/scaffold_design_brief.sh")
                .is_file()
        );
        assert!(
            root.join("sched-ext-build-verify/references/build-and-verifier-checklist.md")
                .is_file()
        );
        assert!(
            root.join("sched-ext-design-loop/references/rollout-checklist.md")
                .is_file()
        );
        assert!(
            root.join("sched-ext-rollout-safety/references/activation-checklist.md")
                .is_file()
        );
        assert!(
            root.join("sched-ext-run-evaluation/references/repetition-and-scoring.md")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/references/analysis-patterns.md")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/scripts/bootstrap_uv_env.sh")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/scripts/analyze_perf_csv.py")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/scripts/compose_perf_evidence.py")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/scripts/compose_sched_trace_evidence.py")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/scripts/render_perf_report.sh")
                .is_file()
        );
        assert!(
            root.join("sched-perf-analysis/scripts/summarize_sched_latency.py")
                .is_file()
        );
        assert!(
            root.join("sched-perf-collection/references/collection-matrix.md")
                .is_file()
        );
        assert!(
            root.join("sched-perf-collection/scripts/collect_perf.sh")
                .is_file()
        );
        assert!(
            root.join("sched-perf-collection/scripts/collect_sched_timeline.sh")
                .is_file()
        );
        assert!(
            root.join("sched-workload-contract/references/selector-and-metric-policy.md")
                .is_file()
        );
    }
}
