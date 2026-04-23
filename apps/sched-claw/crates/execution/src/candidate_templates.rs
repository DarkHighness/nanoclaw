use crate::experiment::experiments_dir;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemplateKnobSpec {
    pub name: &'static str,
    pub default_value: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemplateSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub build_command_template: &'static str,
    pub knob_specs: &'static [TemplateKnobSpec],
    pub source_template: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializedTemplate {
    pub template_name: String,
    pub relative_source_path: String,
    pub absolute_source_path: PathBuf,
    pub relative_object_path: String,
    pub applied_knobs: BTreeMap<String, String>,
}

const DSQ_LOCALITY_SOURCE: &str =
    include_str!("../../../templates/sched_ext/dsq_locality.bpf.c.tmpl");
const LATENCY_GUARD_SOURCE: &str =
    include_str!("../../../templates/sched_ext/latency_guard.bpf.c.tmpl");
const BALANCED_QUEUE_SOURCE: &str =
    include_str!("../../../templates/sched_ext/balanced_queue.bpf.c.tmpl");
const CGROUP_LANE_SOURCE: &str =
    include_str!("../../../templates/sched_ext/cgroup_lane.bpf.c.tmpl");

const DSQ_LOCALITY_KNOBS: &[TemplateKnobSpec] = &[
    TemplateKnobSpec {
        name: "slice_us",
        default_value: "2500",
        description: "Nominal per-task time slice in microseconds.",
    },
    TemplateKnobSpec {
        name: "wakeup_locality_bias",
        default_value: "80",
        description: "Higher values preserve the previous CPU more aggressively.",
    },
    TemplateKnobSpec {
        name: "idle_scan_budget",
        default_value: "4",
        description: "How many idle CPUs to inspect before widening the search.",
    },
    TemplateKnobSpec {
        name: "migration_penalty",
        default_value: "1200",
        description: "Soft migration cost applied before cross-CPU dispatch.",
    },
];

const LATENCY_GUARD_KNOBS: &[TemplateKnobSpec] = &[
    TemplateKnobSpec {
        name: "slice_us",
        default_value: "1000",
        description: "Short latency-oriented slice in microseconds.",
    },
    TemplateKnobSpec {
        name: "preempt_wakeup",
        default_value: "1",
        description: "Whether to allow direct wakeup preemption.",
    },
    TemplateKnobSpec {
        name: "idle_pull",
        default_value: "1",
        description: "Whether idle CPUs proactively pull runnable work.",
    },
    TemplateKnobSpec {
        name: "short_task_ns",
        default_value: "250000",
        description: "Budget used to classify tasks as short interactive work.",
    },
];

const BALANCED_QUEUE_KNOBS: &[TemplateKnobSpec] = &[
    TemplateKnobSpec {
        name: "slice_us",
        default_value: "4000",
        description: "Throughput-oriented slice in microseconds.",
    },
    TemplateKnobSpec {
        name: "queue_steal",
        default_value: "1",
        description: "Whether busy CPUs may steal work from shared queues.",
    },
    TemplateKnobSpec {
        name: "rebalance_interval_ms",
        default_value: "6",
        description: "Background rebalance cadence in milliseconds.",
    },
    TemplateKnobSpec {
        name: "shared_dsq_depth",
        default_value: "64",
        description: "Target depth for the shared dispatch queue.",
    },
];

const CGROUP_LANE_KNOBS: &[TemplateKnobSpec] = &[
    TemplateKnobSpec {
        name: "slice_us",
        default_value: "2000",
        description: "Base time slice in microseconds.",
    },
    TemplateKnobSpec {
        name: "interactive_boost",
        default_value: "20",
        description: "Interactive class bonus applied during enqueue.",
    },
    TemplateKnobSpec {
        name: "background_weight",
        default_value: "60",
        description: "Relative weight for background cgroup lanes.",
    },
    TemplateKnobSpec {
        name: "cgroup_slice_floor_us",
        default_value: "1000",
        description: "Minimum slice floor reserved for each cgroup lane.",
    },
];

const TEMPLATE_SPECS: &[TemplateSpec] = &[
    TemplateSpec {
        name: "dsq_locality",
        summary: "Preserve CPU locality and reduce migration churn.",
        description: "A locality-biased sched-ext starting point with explicit wakeup and migration knobs.",
        build_command_template: "clang -O2 -g -target bpf -c {source} -o {object}",
        knob_specs: DSQ_LOCALITY_KNOBS,
        source_template: DSQ_LOCALITY_SOURCE,
    },
    TemplateSpec {
        name: "latency_guard",
        summary: "Favor short wakeup latency and interactive tasks.",
        description: "A short-slice sched-ext starting point that exposes wakeup preemption and idle pull behavior.",
        build_command_template: "clang -O2 -g -target bpf -c {source} -o {object}",
        knob_specs: LATENCY_GUARD_KNOBS,
        source_template: LATENCY_GUARD_SOURCE,
    },
    TemplateSpec {
        name: "balanced_queue",
        summary: "Maximize steady throughput with shared queue balancing.",
        description: "A throughput-oriented sched-ext starting point with shared queue depth and rebalance controls.",
        build_command_template: "clang -O2 -g -target bpf -c {source} -o {object}",
        knob_specs: BALANCED_QUEUE_KNOBS,
        source_template: BALANCED_QUEUE_SOURCE,
    },
    TemplateSpec {
        name: "cgroup_lane",
        summary: "Separate workload classes into controllable cgroup lanes.",
        description: "A cgroup-aware sched-ext starting point with interactive and background lane controls.",
        build_command_template: "clang -O2 -g -target bpf -c {source} -o {object}",
        knob_specs: CGROUP_LANE_KNOBS,
        source_template: CGROUP_LANE_SOURCE,
    },
];

pub fn template_specs() -> &'static [TemplateSpec] {
    TEMPLATE_SPECS
}

pub fn find_template(name: &str) -> Option<&'static TemplateSpec> {
    TEMPLATE_SPECS.iter().find(|template| template.name == name)
}

pub fn render_build_command(
    template: &TemplateSpec,
    source_path: &str,
    object_path: &str,
) -> String {
    apply_placeholders(
        template.build_command_template,
        &[("source", source_path), ("object", object_path)],
    )
}

pub fn materialize_template(
    workspace_root: &Path,
    experiment_id: &str,
    candidate_id: &str,
    template: &TemplateSpec,
    knobs: &BTreeMap<String, String>,
    output_path: Option<&str>,
) -> Result<MaterializedTemplate> {
    let applied_knobs = merge_knobs(template, knobs)?;
    let relative_source_path = output_path.map(PathBuf::from).unwrap_or_else(|| {
        default_source_relative_path(workspace_root, experiment_id, candidate_id)
    });
    let absolute_source_path = workspace_root.join(&relative_source_path);
    if let Some(parent) = absolute_source_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let object_path = object_path_for_source(&relative_source_path);
    let mut rendered = template.source_template.to_string();
    rendered = apply_placeholders(
        &rendered,
        &[
            ("candidate_id", candidate_id),
            ("experiment_id", experiment_id),
        ],
    );
    for (name, value) in &applied_knobs {
        rendered = apply_placeholders(&rendered, &[(name.as_str(), value.as_str())]);
    }
    std::fs::write(&absolute_source_path, rendered).with_context(|| {
        format!(
            "failed to write materialized template {}",
            absolute_source_path.display()
        )
    })?;

    Ok(MaterializedTemplate {
        template_name: template.name.to_string(),
        relative_source_path: relative_source_path.to_string_lossy().to_string(),
        absolute_source_path,
        relative_object_path: object_path.to_string_lossy().to_string(),
        applied_knobs,
    })
}

fn merge_knobs(
    template: &TemplateSpec,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for key in overrides.keys() {
        if !template.knob_specs.iter().any(|spec| spec.name == key) {
            bail!("unknown knob `{key}` for template `{}`", template.name);
        }
    }

    Ok(template
        .knob_specs
        .iter()
        .map(|spec| {
            (
                spec.name.to_string(),
                overrides
                    .get(spec.name)
                    .cloned()
                    .unwrap_or_else(|| spec.default_value.to_string()),
            )
        })
        .collect())
}

fn default_source_relative_path(
    workspace_root: &Path,
    experiment_id: &str,
    candidate_id: &str,
) -> PathBuf {
    let experiments_root = experiments_dir(workspace_root);
    experiments_root
        .strip_prefix(workspace_root)
        .unwrap_or(experiments_root.as_path())
        .join(experiment_id)
        .join("sources")
        .join(format!("{candidate_id}.bpf.c"))
}

fn object_path_for_source(source_path: &Path) -> PathBuf {
    let file_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("candidate.bpf.c");
    let object_name = if let Some(stripped) = file_name.strip_suffix(".bpf.c") {
        format!("{stripped}.bpf.o")
    } else if let Some(stripped) = file_name.strip_suffix(".c") {
        format!("{stripped}.o")
    } else {
        format!("{file_name}.o")
    };
    source_path.with_file_name(object_name)
}

fn apply_placeholders(template: &str, substitutions: &[(&str, &str)]) -> String {
    let mut rendered = template.to_string();
    for (key, value) in substitutions {
        let needle = format!("{{{{{key}}}}}");
        rendered = rendered.replace(&needle, value);
        let brace_needle = format!("{{{key}}}");
        rendered = rendered.replace(&brace_needle, value);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{
        find_template, materialize_template, object_path_for_source, render_build_command,
        template_specs,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn lists_known_templates() {
        let names = template_specs()
            .iter()
            .map(|template| template.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"dsq_locality"));
        assert!(names.contains(&"cgroup_lane"));
    }

    #[test]
    fn materializes_template_with_default_knobs() {
        let dir = tempdir().unwrap();
        let template = find_template("latency_guard").unwrap();
        let artifact = materialize_template(
            dir.path(),
            "exp-a",
            "cand-a",
            template,
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        let rendered = std::fs::read_to_string(&artifact.absolute_source_path).unwrap();
        assert!(rendered.contains("cand-a"));
        assert!(rendered.contains("const volatile u32 slice_us = 1000;"));
        assert!(artifact.relative_object_path.ends_with("cand-a.bpf.o"));
        assert_eq!(
            render_build_command(
                template,
                &artifact.relative_source_path,
                &artifact.relative_object_path
            ),
            format!(
                "clang -O2 -g -target bpf -c {} -o {}",
                artifact.relative_source_path, artifact.relative_object_path
            )
        );
    }

    #[test]
    fn object_path_tracks_bpf_sources() {
        let object_path = object_path_for_source(std::path::Path::new("sources/demo.bpf.c"));
        assert_eq!(object_path.to_string_lossy(), "sources/demo.bpf.o");
    }

    #[test]
    fn rejects_unknown_knobs() {
        let dir = tempdir().unwrap();
        let template = find_template("balanced_queue").unwrap();
        let error = materialize_template(
            dir.path(),
            "exp-a",
            "cand-a",
            template,
            &BTreeMap::from([("unknown".to_string(), "1".to_string())]),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown knob"));
    }
}
