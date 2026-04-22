use crate::experiment::CandidateSpec;
use anyhow::{Result, bail};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct DeployOverrides {
    pub label: Option<String>,
    pub loader: Option<String>,
    pub loader_args: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub lease_timeout_ms: Option<u64>,
    pub replace_existing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateActivationPlan {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
    pub source_path: Option<String>,
    pub lease_timeout_ms: Option<u64>,
    pub replace_existing: bool,
}

pub fn build_activation_plan(
    experiment_id: &str,
    candidate: &CandidateSpec,
    overrides: &DeployOverrides,
) -> Result<CandidateActivationPlan> {
    let label = overrides
        .label
        .clone()
        .unwrap_or_else(|| format!("{experiment_id}:{}", candidate.candidate_id));
    let cwd = overrides
        .cwd
        .clone()
        .or_else(|| candidate.daemon_cwd.clone());

    let mut env = candidate.daemon_env.clone();
    env.extend(overrides.env.clone());

    let source_path = candidate.source_path.clone();
    let object_path = candidate
        .object_path
        .clone()
        .or_else(|| source_path.as_deref().map(default_object_path));
    let argv = if let Some(loader) = &overrides.loader {
        resolve_loader_argv(
            loader,
            &overrides.loader_args,
            experiment_id,
            &candidate.candidate_id,
            source_path.as_deref(),
            object_path.as_deref(),
        )?
    } else {
        let argv = candidate
            .daemon_argv
            .iter()
            .map(|arg| {
                substitute_tokens(
                    arg,
                    experiment_id,
                    &candidate.candidate_id,
                    source_path.as_deref(),
                    object_path.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        if argv.is_empty() {
            bail!(
                "candidate {} does not define daemon argv; use experiment materialize or deploy overrides",
                candidate.candidate_id
            );
        }
        argv
    };

    Ok(CandidateActivationPlan {
        label,
        argv,
        cwd,
        env,
        source_path,
        lease_timeout_ms: overrides.lease_timeout_ms,
        replace_existing: overrides.replace_existing,
    })
}

fn resolve_loader_argv(
    loader: &str,
    loader_args: &[String],
    experiment_id: &str,
    candidate_id: &str,
    source_path: Option<&str>,
    object_path: Option<&str>,
) -> Result<Vec<String>> {
    let mut argv = vec![loader.to_string()];
    if loader_args.is_empty() {
        if let Some(source_path) = source_path {
            argv.push(source_path.to_string());
            return Ok(argv);
        }
        bail!("loader args are empty and candidate has no source path to append");
    }

    let mut saw_source_like_placeholder = false;
    for arg in loader_args {
        if arg.contains("{source}") || arg.contains("{object}") {
            saw_source_like_placeholder = true;
        }
        argv.push(substitute_tokens(
            arg,
            experiment_id,
            candidate_id,
            source_path,
            object_path,
        ));
    }
    if !saw_source_like_placeholder && let Some(source_path) = source_path {
        argv.push(source_path.to_string());
    }
    Ok(argv)
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

#[cfg(test)]
mod tests {
    use super::{DeployOverrides, build_activation_plan};
    use crate::experiment::CandidateSpec;
    use std::collections::BTreeMap;

    #[test]
    fn uses_candidate_daemon_argv_placeholders() {
        let candidate = CandidateSpec {
            candidate_id: "cand-a".to_string(),
            template: "latency_guard".to_string(),
            source_path: Some("artifacts/cand-a.bpf.c".to_string()),
            object_path: Some("artifacts/cand-a.bpf.o".to_string()),
            build_command: None,
            daemon_argv: vec!["loader".to_string(), "{source}".to_string()],
            daemon_cwd: None,
            daemon_env: BTreeMap::new(),
            knobs: BTreeMap::new(),
            notes: None,
        };
        let plan = build_activation_plan("exp-a", &candidate, &DeployOverrides::default()).unwrap();
        assert_eq!(plan.argv, vec!["loader", "artifacts/cand-a.bpf.c"]);
    }

    #[test]
    fn loader_overrides_append_source_by_default() {
        let candidate = CandidateSpec {
            candidate_id: "cand-a".to_string(),
            template: "latency_guard".to_string(),
            source_path: Some("artifacts/cand-a.bpf.c".to_string()),
            object_path: Some("artifacts/cand-a.bpf.o".to_string()),
            build_command: None,
            daemon_argv: Vec::new(),
            daemon_cwd: None,
            daemon_env: BTreeMap::from([("MODE".to_string(), "baseline".to_string())]),
            knobs: BTreeMap::new(),
            notes: None,
        };
        let plan = build_activation_plan(
            "exp-a",
            &candidate,
            &DeployOverrides {
                loader: Some("/tmp/mock-loader".to_string()),
                env: BTreeMap::from([("MODE".to_string(), "candidate".to_string())]),
                ..DeployOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(
            plan.argv,
            vec!["/tmp/mock-loader", "artifacts/cand-a.bpf.c"]
        );
        assert_eq!(plan.env.get("MODE").map(String::as_str), Some("candidate"));
    }
}
