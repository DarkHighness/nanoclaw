use crate::daemon_protocol::{
    ActiveDeploymentSnapshot, DaemonLogsSnapshot, DaemonStatusSnapshot, DeploymentExitSnapshot,
    SchedExtDaemonResponse,
};
use crate::experiment::{
    ExperimentArtifact, ExperimentScoreReport, ExperimentSummary, GuardrailStatus, LoadedExperiment,
};
use crate::history::{LoadedSessionDetail, SessionExportArtifact, SessionExportKind, preview_id};
use clap::ValueEnum;
use std::fmt::Write as _;
use unicode_width::UnicodeWidthStr;

use agent::{Skill, ToolKind, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
use store::{SessionSearchResult, SessionSummary, SessionTokenUsageReport};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputStyle {
    Table,
    Plain,
}

impl OutputStyle {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Plain => "plain",
        }
    }
}

pub fn render_tool_list(tool_specs: &[ToolSpec], style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Tools · {}", tool_specs.len())),
            &["#", "Name", "Kind", "Source", "Origin", "Description"],
            &tool_specs
                .iter()
                .enumerate()
                .map(|(index, spec)| {
                    vec![
                        (index + 1).to_string(),
                        spec.name.to_string(),
                        tool_kind_label(&spec.kind).to_string(),
                        tool_source_label(&spec.source),
                        tool_origin_label(&spec.origin),
                        spec.description.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Tools ({})", tool_specs.len());
            for spec in tool_specs {
                let _ = writeln!(
                    &mut out,
                    "- {} [{} / {} / {}]",
                    spec.name,
                    tool_kind_label(&spec.kind),
                    tool_source_label(&spec.source),
                    tool_origin_label(&spec.origin),
                );
                let _ = writeln!(&mut out, "  {}", spec.description);
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_tool_detail(spec: &ToolSpec, style: OutputStyle) -> String {
    let mut sections = vec![(
        "Overview",
        vec![
            ("Name".to_string(), spec.name.to_string()),
            ("Description".to_string(), spec.description.clone()),
            ("Kind".to_string(), tool_kind_label(&spec.kind).to_string()),
            (
                "Output Mode".to_string(),
                tool_output_mode_label(&spec.output_mode).to_string(),
            ),
            ("Source".to_string(), tool_source_label(&spec.source)),
            ("Origin".to_string(), tool_origin_label(&spec.origin)),
        ],
    )];
    sections.push((
        "Capabilities",
        vec![
            (
                "Aliases".to_string(),
                join_or_none(spec.aliases.iter().map(ToString::to_string).collect()),
            ),
            (
                "Parallel Calls".to_string(),
                bool_label(spec.supports_parallel_tool_calls),
            ),
            ("Deferred Load".to_string(), bool_label(spec.defer_loading)),
            (
                "Hidden From Model".to_string(),
                bool_label(spec.availability.hidden_from_model),
            ),
        ],
    ));
    sections.push((
        "Approval",
        vec![
            ("Read Only".to_string(), bool_label(spec.approval.read_only)),
            (
                "Mutates State".to_string(),
                bool_label(spec.approval.mutates_state),
            ),
            (
                "Idempotent".to_string(),
                spec.approval
                    .idempotent
                    .map(bool_label)
                    .unwrap_or_else(|| "<unspecified>".to_string()),
            ),
            (
                "Needs Network".to_string(),
                bool_label(spec.approval.needs_network),
            ),
            (
                "Needs Host Escape".to_string(),
                bool_label(spec.approval.needs_host_escape),
            ),
            (
                "Approval Message".to_string(),
                spec.approval
                    .approval_message
                    .clone()
                    .unwrap_or_else(|| "<none>".to_string()),
            ),
        ],
    ));
    let availability = vec![
        (
            "Feature Flags".to_string(),
            join_or_none(spec.availability.feature_flags.clone()),
        ),
        (
            "Provider Allowlist".to_string(),
            join_or_none(spec.availability.provider_allowlist.clone()),
        ),
        (
            "Model Allowlist".to_string(),
            join_or_none(spec.availability.model_allowlist.clone()),
        ),
        (
            "Role Allowlist".to_string(),
            join_or_none(spec.availability.role_allowlist.clone()),
        ),
    ];
    if availability.iter().any(|(_, value)| value != "<none>") {
        sections.push(("Availability", availability));
    }
    render_sections(
        &format!("Tool · {}", spec.name),
        &sections,
        style,
        Some(format!(
            "Schemas: input={}, output={}",
            bool_label(spec.input_schema.is_some()),
            bool_label(spec.output_schema.is_some())
        )),
    )
}

pub fn render_skill_list(skills: &[Skill], style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Skills · {}", skills.len())),
            &["#", "Name", "Source", "Aliases", "Tags", "Description"],
            &skills
                .iter()
                .enumerate()
                .map(|(index, skill)| {
                    vec![
                        (index + 1).to_string(),
                        skill.name.clone(),
                        skill.provenance.root.kind.as_str().to_string(),
                        join_or_none(skill.aliases.clone()),
                        join_or_none(skill.tags.clone()),
                        skill.description.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Skills ({})", skills.len());
            for skill in skills {
                let _ = writeln!(
                    &mut out,
                    "- {} [{}]",
                    skill.name,
                    skill.provenance.root.kind.as_str()
                );
                let _ = writeln!(&mut out, "  {}", skill.description);
                if !skill.aliases.is_empty() {
                    let _ = writeln!(&mut out, "  aliases: {}", skill.aliases.join(", "));
                }
                if !skill.tags.is_empty() {
                    let _ = writeln!(&mut out, "  tags: {}", skill.tags.join(", "));
                }
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_skill_detail(skill: &Skill, style: OutputStyle) -> String {
    let mut sections = vec![(
        "Overview",
        vec![
            ("Name".to_string(), skill.name.clone()),
            ("Description".to_string(), skill.description.clone()),
            ("Aliases".to_string(), join_or_none(skill.aliases.clone())),
            ("Tags".to_string(), join_or_none(skill.tags.clone())),
            (
                "Root Kind".to_string(),
                skill.provenance.root.kind.as_str().to_string(),
            ),
        ],
    )];

    let activation = vec![
        (
            "Platforms".to_string(),
            join_or_none(skill.activation.platforms.clone()),
        ),
        (
            "Required Tools".to_string(),
            join_or_none(
                skill
                    .activation
                    .requires_tools
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
        ),
        (
            "Fallback Tools".to_string(),
            join_or_none(
                skill
                    .activation
                    .fallback_for_tools
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            ),
        ),
    ];
    if activation.iter().any(|(_, value)| value != "<none>") {
        sections.push(("Activation", activation));
    }

    sections.push((
        "Files",
        vec![
            (
                "Skill Path".to_string(),
                skill.skill_path().display().to_string(),
            ),
            (
                "Root Path".to_string(),
                skill.provenance.root.path.display().to_string(),
            ),
            (
                "References".to_string(),
                join_or_none(
                    skill
                        .references
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                ),
            ),
            (
                "Scripts".to_string(),
                join_or_none(
                    skill
                        .scripts
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                ),
            ),
            (
                "Assets".to_string(),
                join_or_none(
                    skill
                        .assets
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                ),
            ),
        ],
    ));

    render_sections(
        &format!("Skill · {}", skill.name),
        &sections,
        style,
        Some(format!(
            "Instruction Body\n{}\n{}",
            "-".repeat("Instruction Body".len()),
            skill.body.trim()
        )),
    )
}

pub fn render_daemon_response(response: &SchedExtDaemonResponse, style: OutputStyle) -> String {
    match response {
        SchedExtDaemonResponse::Status { snapshot } => render_daemon_status(snapshot, style),
        SchedExtDaemonResponse::Logs { snapshot } => render_daemon_logs(snapshot, style),
        SchedExtDaemonResponse::Ack { message, snapshot } => {
            let body = render_daemon_status(snapshot, style);
            if body.is_empty() {
                message.clone()
            } else {
                format!("{message}\n\n{body}")
            }
        }
        SchedExtDaemonResponse::Error { message } => format!("daemon error: {message}"),
    }
}

pub fn render_session_list(summaries: &[SessionSummary], style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Sessions · {}", summaries.len())),
            &[
                "#",
                "Session",
                "Last Prompt",
                "Events",
                "Messages",
                "Agents",
            ],
            &summaries
                .iter()
                .enumerate()
                .map(|(index, summary)| {
                    vec![
                        (index + 1).to_string(),
                        preview_id(summary.session_id.as_str()),
                        summary
                            .last_user_prompt
                            .clone()
                            .unwrap_or_else(|| "<none>".to_string()),
                        summary.event_count.to_string(),
                        summary.transcript_message_count.to_string(),
                        summary.agent_session_count.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Sessions ({})", summaries.len());
            for summary in summaries {
                let _ = writeln!(
                    &mut out,
                    "- {} events={} messages={} agents={}",
                    summary.session_id,
                    summary.event_count,
                    summary.transcript_message_count,
                    summary.agent_session_count
                );
                if let Some(prompt) = &summary.last_user_prompt {
                    let _ = writeln!(&mut out, "  prompt: {prompt}");
                }
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_session_search_results(
    results: &[SessionSearchResult],
    style: OutputStyle,
) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Session Matches · {}", results.len())),
            &["#", "Session", "Matched Events", "Last Prompt", "Preview"],
            &results
                .iter()
                .enumerate()
                .map(|(index, result)| {
                    vec![
                        (index + 1).to_string(),
                        preview_id(result.summary.session_id.as_str()),
                        result.matched_event_count.to_string(),
                        result
                            .summary
                            .last_user_prompt
                            .clone()
                            .unwrap_or_else(|| "<none>".to_string()),
                        join_or_none(result.preview_matches.clone()),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Session Matches ({})", results.len());
            for result in results {
                let _ = writeln!(
                    &mut out,
                    "- {} matched_events={}",
                    result.summary.session_id, result.matched_event_count
                );
                for preview in &result.preview_matches {
                    let _ = writeln!(&mut out, "  preview: {preview}");
                }
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_session_detail(detail: &LoadedSessionDetail, style: OutputStyle) -> String {
    let mut sections = vec![(
        "Overview",
        vec![
            ("Session".to_string(), detail.summary.session_id.to_string()),
            (
                "Last Prompt".to_string(),
                detail
                    .summary
                    .last_user_prompt
                    .clone()
                    .unwrap_or_else(|| "<none>".to_string()),
            ),
            ("Events".to_string(), detail.summary.event_count.to_string()),
            (
                "Messages".to_string(),
                detail.summary.transcript_message_count.to_string(),
            ),
            (
                "Agent Sessions".to_string(),
                detail.summary.agent_session_count.to_string(),
            ),
        ],
    )];
    sections.push((
        "Runtime",
        vec![
            (
                "Agent Session IDs".to_string(),
                join_or_none(
                    detail
                        .agent_session_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                ),
            ),
            (
                "Time Range (ms)".to_string(),
                format!(
                    "{} -> {}",
                    detail.summary.first_timestamp_ms, detail.summary.last_timestamp_ms
                ),
            ),
        ],
    ));
    sections.push(("Token Usage", token_usage_rows(&detail.token_usage)));
    render_sections(
        &format!("Session · {}", detail.summary.session_id),
        &sections,
        style,
        Some(format!(
            "Transcript\n{}\n{}",
            "-".repeat("Transcript".len()),
            if detail.transcript.is_empty() {
                "<empty>".to_string()
            } else {
                crate::history::render_transcript_text(&detail.transcript)
            }
        )),
    )
}

pub fn render_session_export_artifact(artifact: &SessionExportArtifact) -> String {
    let kind = match artifact.kind {
        SessionExportKind::EventsJsonl => "events",
        SessionExportKind::TranscriptText => "transcript",
    };
    format!(
        "Exported {kind} for {} to {} ({} items).",
        artifact.session_id,
        artifact.output_path.display(),
        artifact.item_count
    )
}

pub fn render_experiment_list(summaries: &[ExperimentSummary], style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Experiments · {}", summaries.len())),
            &[
                "#",
                "Experiment",
                "Workload",
                "Primary Metric",
                "Baselines",
                "Candidates",
                "Updated (ms)",
            ],
            &summaries
                .iter()
                .enumerate()
                .map(|(index, summary)| {
                    vec![
                        (index + 1).to_string(),
                        summary.experiment_id.clone(),
                        summary.workload_name.clone(),
                        summary.primary_metric_name.clone(),
                        summary.baseline_run_count.to_string(),
                        summary.candidate_count.to_string(),
                        summary.updated_at_unix_ms.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Experiments ({})", summaries.len());
            for summary in summaries {
                let _ = writeln!(
                    &mut out,
                    "- {} workload={} primary_metric={} baselines={} candidates={}",
                    summary.experiment_id,
                    summary.workload_name,
                    summary.primary_metric_name,
                    summary.baseline_run_count,
                    summary.candidate_count
                );
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_experiment_detail(experiment: &LoadedExperiment, style: OutputStyle) -> String {
    let manifest = &experiment.manifest;
    let sections = vec![
        (
            "Overview",
            vec![
                ("Experiment".to_string(), manifest.experiment_id.clone()),
                (
                    "Manifest Path".to_string(),
                    experiment.manifest_path.display().to_string(),
                ),
                ("Version".to_string(), manifest.version.to_string()),
                (
                    "Updated (ms)".to_string(),
                    manifest.updated_at_unix_ms.to_string(),
                ),
            ],
        ),
        (
            "Workload",
            vec![
                ("Name".to_string(), manifest.workload.name.clone()),
                (
                    "Description".to_string(),
                    manifest
                        .workload
                        .description
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Cwd".to_string(),
                    manifest
                        .workload
                        .cwd
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Argv".to_string(),
                    join_or_none(manifest.workload.argv.clone()),
                ),
                (
                    "Scope".to_string(),
                    manifest
                        .workload
                        .scope
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Phase".to_string(),
                    manifest
                        .workload
                        .phase
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
            ],
        ),
        (
            "Metrics",
            vec![
                (
                    "Primary Metric".to_string(),
                    format!(
                        "{} ({})",
                        manifest.primary_metric.name,
                        manifest.primary_metric.goal.as_str()
                    ),
                ),
                (
                    "Primary Unit".to_string(),
                    manifest
                        .primary_metric
                        .unit
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Guardrails".to_string(),
                    if manifest.guardrails.is_empty() {
                        "<none>".to_string()
                    } else {
                        manifest
                            .guardrails
                            .iter()
                            .map(|guardrail| {
                                format!(
                                    "{}:{}:{}%",
                                    guardrail.name,
                                    guardrail.goal.as_str(),
                                    format_float(guardrail.max_regression_pct)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                ),
            ],
        ),
        (
            "Runs",
            vec![
                (
                    "Baseline Runs".to_string(),
                    manifest.baseline_runs.len().to_string(),
                ),
                (
                    "Candidates".to_string(),
                    manifest.candidates.len().to_string(),
                ),
                (
                    "Host Kernel".to_string(),
                    manifest
                        .host
                        .kernel_release
                        .clone()
                        .unwrap_or_else(|| "<unknown>".to_string()),
                ),
                (
                    "CPU Model".to_string(),
                    manifest
                        .host
                        .cpu_model
                        .clone()
                        .unwrap_or_else(|| "<unknown>".to_string()),
                ),
            ],
        ),
    ];

    let appendix = format!(
        "Baseline Labels\n{}\n{}\n\nCandidates\n{}\n{}",
        "-".repeat("Baseline Labels".len()),
        list_run_labels(&manifest.baseline_runs),
        "-".repeat("Candidates".len()),
        list_candidate_summaries(manifest)
    );
    render_sections(
        &format!("Experiment · {}", manifest.experiment_id),
        &sections,
        style,
        Some(appendix),
    )
}

pub fn render_experiment_score(report: &ExperimentScoreReport, style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => {
            let mut out = render_grid(
                Some(format!("Experiment Score · {}", report.experiment_id)),
                &[
                    "Candidate",
                    "Template",
                    "Runs",
                    "Primary Value",
                    "Improvement %",
                    "Decision",
                    "Guardrails",
                ],
                &report
                    .entries
                    .iter()
                    .map(|entry| {
                        vec![
                            entry.candidate_id.clone(),
                            entry.template.clone(),
                            entry.run_count.to_string(),
                            entry
                                .primary_candidate_value
                                .map(format_float)
                                .unwrap_or_else(|| "<missing>".to_string()),
                            entry
                                .primary_improvement_pct
                                .map(format_pct)
                                .unwrap_or_else(|| "<missing>".to_string()),
                            entry.decision.as_str().to_string(),
                            render_guardrail_statuses(entry),
                        ]
                    })
                    .collect::<Vec<_>>(),
            );
            let _ = write!(
                &mut out,
                "\nprimary metric: {} ({})\nbaseline runs: {}\nbaseline value: {}",
                report.primary_metric.name,
                report.primary_metric.goal.as_str(),
                report.baseline_run_count,
                report
                    .baseline_primary_value
                    .map(format_float)
                    .unwrap_or_else(|| "<missing>".to_string())
            );
            out
        }
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Experiment Score · {}", report.experiment_id);
            let _ = writeln!(
                &mut out,
                "primary_metric: {} ({})",
                report.primary_metric.name,
                report.primary_metric.goal.as_str()
            );
            let _ = writeln!(&mut out, "baseline_runs: {}", report.baseline_run_count);
            let _ = writeln!(
                &mut out,
                "baseline_value: {}",
                report
                    .baseline_primary_value
                    .map(format_float)
                    .unwrap_or_else(|| "<missing>".to_string())
            );
            for entry in &report.entries {
                let _ = writeln!(
                    &mut out,
                    "- {} template={} runs={} value={} improvement={} decision={}",
                    entry.candidate_id,
                    entry.template,
                    entry.run_count,
                    entry
                        .primary_candidate_value
                        .map(format_float)
                        .unwrap_or_else(|| "<missing>".to_string()),
                    entry
                        .primary_improvement_pct
                        .map(format_pct)
                        .unwrap_or_else(|| "<missing>".to_string()),
                    entry.decision.as_str()
                );
                if !entry.breached_guardrails.is_empty() {
                    let _ = writeln!(
                        &mut out,
                        "  guardrails: {}",
                        render_guardrail_statuses(entry)
                    );
                }
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_experiment_artifact(artifact: &ExperimentArtifact) -> String {
    format!(
        "{} experiment {} at {}",
        artifact.action,
        artifact.experiment_id,
        artifact.manifest_path.display()
    )
}

pub fn render_daemon_status(snapshot: &DaemonStatusSnapshot, style: OutputStyle) -> String {
    let mut sections = vec![(
        "Overview",
        vec![
            ("Daemon PID".to_string(), snapshot.daemon_pid.to_string()),
            (
                "Workspace Root".to_string(),
                snapshot.workspace_root.clone(),
            ),
            ("Socket Path".to_string(), snapshot.socket_path.clone()),
            (
                "Allowed Roots".to_string(),
                join_or_none(snapshot.allowed_roots.clone()),
            ),
            ("Active".to_string(), bool_label(snapshot.active.is_some())),
        ],
    )];

    if let Some(active) = &snapshot.active {
        sections.push(("Active Deployment", active_rows(active)));
    } else if let Some(last_exit) = &snapshot.last_exit {
        sections.push(("Last Exit", exit_rows(last_exit)));
    } else {
        sections.push(("Last Exit", vec![("State".to_string(), "none".to_string())]));
    }
    render_sections("Daemon Status", &sections, style, None)
}

pub fn render_daemon_logs(snapshot: &DaemonLogsSnapshot, style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => {
            if snapshot.lines.is_empty() {
                return render_sections(
                    "Daemon Logs",
                    &[(
                        "Overview",
                        vec![
                            (
                                "Active Label".to_string(),
                                snapshot
                                    .active_label
                                    .clone()
                                    .unwrap_or_else(|| "<none>".to_string()),
                            ),
                            ("Truncated".to_string(), bool_label(snapshot.truncated)),
                            ("Logs".to_string(), "<empty>".to_string()),
                        ],
                    )],
                    style,
                    None,
                );
            }
            let mut out = render_grid(
                Some(format!(
                    "Daemon Logs · {}",
                    snapshot.active_label.as_deref().unwrap_or("<none>")
                )),
                &["Timestamp (ms)", "Source", "Line"],
                &snapshot
                    .lines
                    .iter()
                    .map(|entry| {
                        vec![
                            entry.emitted_at_unix_ms.to_string(),
                            entry.source.clone(),
                            entry.line.clone(),
                        ]
                    })
                    .collect::<Vec<_>>(),
            );
            let _ = write!(&mut out, "\ntruncated: {}", bool_label(snapshot.truncated));
            out
        }
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(
                &mut out,
                "active_label: {}",
                snapshot.active_label.as_deref().unwrap_or("<none>")
            );
            let _ = writeln!(&mut out, "truncated: {}", snapshot.truncated);
            if snapshot.lines.is_empty() {
                let _ = writeln!(&mut out, "logs: <empty>");
                return out.trim_end().to_string();
            }
            let _ = writeln!(&mut out, "logs:");
            for entry in &snapshot.lines {
                let _ = writeln!(
                    &mut out,
                    "[{}][{}] {}",
                    entry.emitted_at_unix_ms, entry.source, entry.line
                );
            }
            out.trim_end().to_string()
        }
    }
}

fn active_rows(active: &ActiveDeploymentSnapshot) -> Vec<(String, String)> {
    vec![
        ("Label".to_string(), active.label.clone()),
        ("PID".to_string(), active.pid.to_string()),
        ("Cwd".to_string(), active.cwd.clone()),
        ("Argv".to_string(), active.argv.join(" ")),
        (
            "Started At (unix_s)".to_string(),
            active.started_at_unix_s.to_string(),
        ),
        ("Log Lines".to_string(), active.log_line_count.to_string()),
    ]
}

fn exit_rows(last_exit: &DeploymentExitSnapshot) -> Vec<(String, String)> {
    vec![
        ("Label".to_string(), last_exit.label.clone()),
        ("PID".to_string(), last_exit.pid.to_string()),
        ("Cwd".to_string(), last_exit.cwd.clone()),
        ("Argv".to_string(), last_exit.argv.join(" ")),
        (
            "Started At (unix_s)".to_string(),
            last_exit.started_at_unix_s.to_string(),
        ),
        (
            "Ended At (unix_s)".to_string(),
            last_exit.ended_at_unix_s.to_string(),
        ),
        (
            "Exit Code".to_string(),
            last_exit
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
        ),
        (
            "Signal".to_string(),
            last_exit
                .signal
                .map(|signal| signal.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
        ),
        (
            "Log Lines".to_string(),
            last_exit.log_line_count.to_string(),
        ),
    ]
}

fn token_usage_rows(report: &SessionTokenUsageReport) -> Vec<(String, String)> {
    let aggregate = report.aggregate_usage;
    let prefix_cache_rate = report
        .aggregate_prefix_cache_hit_rate_basis_points()
        .map(|basis_points| format!("{:.2}%", f64::from(basis_points) / 100.0))
        .unwrap_or_else(|| "<none>".to_string());
    let context_window = report
        .session
        .as_ref()
        .and_then(|record| record.ledger.context_window)
        .map(|window| format!("{} / {}", window.used_tokens, window.max_tokens))
        .unwrap_or_else(|| "<none>".to_string());
    vec![
        (
            "Session Record".to_string(),
            bool_label(report.session.is_some()),
        ),
        (
            "Root Agent Sessions".to_string(),
            report.agent_sessions.len().to_string(),
        ),
        ("Subagents".to_string(), report.subagents.len().to_string()),
        ("Tasks".to_string(), report.tasks.len().to_string()),
        (
            "Visible Total Tokens".to_string(),
            aggregate.visible_total_tokens().to_string(),
        ),
        (
            "Input Tokens".to_string(),
            aggregate.input_tokens.to_string(),
        ),
        (
            "Output Tokens".to_string(),
            aggregate.output_tokens.to_string(),
        ),
        (
            "Cache Read Tokens".to_string(),
            aggregate.cache_read_tokens.to_string(),
        ),
        (
            "Reasoning Tokens".to_string(),
            aggregate.reasoning_tokens.to_string(),
        ),
        ("Prefix Cache Hit Rate".to_string(), prefix_cache_rate),
        ("Context Window".to_string(), context_window),
    ]
}

fn render_sections(
    title: &str,
    sections: &[(&str, Vec<(String, String)>)],
    style: OutputStyle,
    appendix: Option<String>,
) -> String {
    let mut out = match style {
        OutputStyle::Table => {
            let rows = sections
                .iter()
                .flat_map(|(section, rows)| {
                    rows.iter().map(|(key, value)| {
                        vec![(*section).to_string(), key.clone(), value.clone()]
                    })
                })
                .collect::<Vec<_>>();
            render_grid(
                Some(title.to_string()),
                &["Section", "Field", "Value"],
                &rows,
            )
        }
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "{title}");
            for (section, rows) in sections {
                let _ = writeln!(&mut out, "{section}:");
                for (key, value) in rows {
                    let _ = writeln!(&mut out, "  {key}: {value}");
                }
            }
            out.trim_end().to_string()
        }
    };

    if let Some(appendix) = appendix
        && !appendix.trim().is_empty()
    {
        let separator = if out.is_empty() { "" } else { "\n\n" };
        out.push_str(separator);
        out.push_str(appendix.trim());
    }
    out
}

fn render_grid(title: Option<String>, headers: &[&str], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths = headers
        .iter()
        .map(|header| UnicodeWidthStr::width(*header))
        .collect::<Vec<_>>();

    for row in rows {
        for (index, cell) in row.iter().enumerate().take(cols) {
            widths[index] = widths[index].max(max_line_width(cell));
        }
    }

    let mut out = String::new();
    if let Some(title) = title
        && !title.trim().is_empty()
    {
        let _ = writeln!(&mut out, "{title}");
    }
    let border = table_border(&widths);
    let _ = writeln!(&mut out, "{border}");
    let _ = writeln!(
        &mut out,
        "{}",
        table_row(
            &headers
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>(),
            &widths
        )
    );
    let _ = writeln!(&mut out, "{border}");
    for row in rows {
        let _ = writeln!(&mut out, "{}", table_row(row, &widths));
    }
    let _ = writeln!(&mut out, "{border}");
    out.trim_end().to_string()
}

fn table_border(widths: &[usize]) -> String {
    let mut line = String::new();
    line.push('+');
    for width in widths {
        line.push_str(&"-".repeat(*width + 2));
        line.push('+');
    }
    line
}

fn table_row(values: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    line.push('|');
    for (index, width) in widths.iter().enumerate() {
        let value = values.get(index).cloned().unwrap_or_default();
        let display_width = max_line_width(&value);
        let padding = width.saturating_sub(display_width);
        line.push(' ');
        line.push_str(&value.replace('\n', " "));
        line.push_str(&" ".repeat(padding));
        line.push(' ');
        line.push('|');
    }
    line
}

fn max_line_width(value: &str) -> usize {
    value
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or_default()
}

fn join_or_none(values: Vec<String>) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values.join(", ")
    }
}

fn bool_label(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

fn format_float(value: f64) -> String {
    format!("{value:.3}")
}

fn format_pct(value: f64) -> String {
    format!("{value:.2}%")
}

fn list_run_labels(runs: &[crate::experiment::RecordedRun]) -> String {
    if runs.is_empty() {
        "<none>".to_string()
    } else {
        runs.iter()
            .map(|run| {
                format!(
                    "{} [{}] {}",
                    run.label,
                    run.scheduler.as_str(),
                    run.artifact_dir
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn list_candidate_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    if manifest.candidates.is_empty() {
        "<none>".to_string()
    } else {
        manifest
            .candidates
            .iter()
            .map(|candidate| {
                format!(
                    "{} [{}] runs={} knobs={}",
                    candidate.spec.candidate_id,
                    candidate.spec.template,
                    candidate.runs.len(),
                    if candidate.spec.knobs.is_empty() {
                        "<none>".to_string()
                    } else {
                        candidate
                            .spec
                            .knobs
                            .iter()
                            .map(|(key, value)| format!("{key}={value}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn render_guardrail_statuses(entry: &crate::experiment::CandidateScore) -> String {
    let rendered = entry
        .breached_guardrails
        .iter()
        .map(|guardrail| {
            let status = match guardrail.status {
                GuardrailStatus::Pass => "pass",
                GuardrailStatus::Breach => "breach",
                GuardrailStatus::Missing => "missing",
            };
            let delta = guardrail
                .improvement_pct
                .map(format_pct)
                .unwrap_or_else(|| "<missing>".to_string());
            format!("{}:{} ({delta})", guardrail.name, status)
        })
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        "<none>".to_string()
    } else {
        rendered.join(", ")
    }
}

fn tool_kind_label(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Function => "function",
        ToolKind::Freeform => "freeform",
        ToolKind::Native => "native",
    }
}

fn tool_output_mode_label(mode: &ToolOutputMode) -> &'static str {
    match mode {
        ToolOutputMode::Text => "text",
        ToolOutputMode::ContentParts => "content_parts",
    }
}

fn tool_source_label(source: &ToolSource) -> String {
    match source {
        ToolSource::Builtin => "builtin".to_string(),
        ToolSource::Dynamic => "dynamic".to_string(),
        ToolSource::Plugin { plugin } => format!("plugin:{plugin}"),
        ToolSource::McpTool { server_name } => format!("mcp_tool:{server_name}"),
        ToolSource::McpResource { server_name } => format!("mcp_resource:{server_name}"),
        ToolSource::ProviderBuiltin { provider } => format!("provider_builtin:{provider}"),
    }
}

fn tool_origin_label(origin: &ToolOrigin) -> String {
    match origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputStyle, render_daemon_status, render_experiment_artifact, render_experiment_list,
        render_experiment_score, render_session_export_artifact, render_session_list,
        render_skill_list, render_tool_list,
    };
    use crate::daemon_protocol::DaemonStatusSnapshot;
    use crate::experiment::{
        CandidateDecision, CandidateScore, ExperimentArtifact, ExperimentScoreReport,
        ExperimentSummary,
    };
    use crate::history::{SessionExportArtifact, SessionExportKind};
    use crate::metrics::{MetricGoal, MetricTarget};
    use agent::{
        SessionId, Skill, SkillProvenance, SkillRoot, SkillRootKind, ToolOrigin, ToolOutputMode,
        ToolSource, ToolSpec,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use store::SessionSummary;

    #[test]
    fn renders_tool_table_with_headers() {
        let tool = ToolSpec::function(
            "read",
            "Read a file",
            json!({"type":"object","properties":{}}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        );
        let rendered = render_tool_list(&[tool], OutputStyle::Table);
        assert!(rendered.contains("Tools · 1"));
        assert!(rendered.contains("| Name "));
        assert!(rendered.contains("Read a file"));
    }

    #[test]
    fn renders_skill_plain_view() {
        let skill = Skill {
            name: "linux-scheduler-triage".to_string(),
            description: "Collect scheduler evidence".to_string(),
            aliases: vec!["scheduler-triage".to_string()],
            body: "body".to_string(),
            root_dir: PathBuf::from("/tmp/skills"),
            tags: vec!["linux".to_string()],
            hooks: Vec::new(),
            references: Vec::new(),
            scripts: Vec::new(),
            assets: Vec::new(),
            metadata: BTreeMap::new(),
            extension_metadata: BTreeMap::new(),
            activation: Default::default(),
            provenance: SkillProvenance {
                root: SkillRoot {
                    path: PathBuf::from("/tmp/skills"),
                    kind: SkillRootKind::External,
                },
                skill_dir: PathBuf::from("/tmp/skills/linux-scheduler-triage"),
                hub: None,
                shadowed_copies: Vec::new(),
            },
        };
        let rendered = render_skill_list(&[skill], OutputStyle::Plain);
        assert!(rendered.contains("Skills (1)"));
        assert!(rendered.contains("aliases: scheduler-triage"));
    }

    #[test]
    fn renders_daemon_status_sections() {
        let snapshot = DaemonStatusSnapshot {
            daemon_pid: 42,
            workspace_root: "/repo".to_string(),
            socket_path: "/repo/sock".to_string(),
            allowed_roots: vec!["/repo".to_string()],
            active: None,
            last_exit: None,
        };
        let rendered = render_daemon_status(&snapshot, OutputStyle::Plain);
        assert!(rendered.contains("Daemon Status"));
        assert!(rendered.contains("Daemon PID: 42"));
        assert!(rendered.contains("State: none"));
    }

    #[test]
    fn renders_session_list_plain_view() {
        let rendered = render_session_list(
            &[SessionSummary {
                session_id: SessionId::from("session_abc123"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                agent_session_count: 1,
                transcript_message_count: 2,
                last_user_prompt: Some("inspect wakeup latency".to_string()),
                token_usage: None,
            }],
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Sessions (1)"));
        assert!(rendered.contains("inspect wakeup latency"));
    }

    #[test]
    fn renders_export_artifact_summary() {
        let rendered = render_session_export_artifact(&SessionExportArtifact {
            kind: SessionExportKind::TranscriptText,
            session_id: SessionId::from("session_abc123"),
            output_path: PathBuf::from("/tmp/transcript.txt"),
            item_count: 4,
        });
        assert!(rendered.contains("Exported transcript"));
        assert!(rendered.contains("/tmp/transcript.txt"));
    }

    #[test]
    fn renders_experiment_list_plain_view() {
        let rendered = render_experiment_list(
            &[ExperimentSummary {
                experiment_id: "demo".to_string(),
                manifest_path: PathBuf::from("/tmp/demo/experiment.toml"),
                updated_at_unix_ms: 42,
                workload_name: "bench".to_string(),
                primary_metric_name: "latency_ms".to_string(),
                baseline_run_count: 1,
                candidate_count: 2,
            }],
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Experiments (1)"));
        assert!(rendered.contains("latency_ms"));
    }

    #[test]
    fn renders_experiment_score_table() {
        let rendered = render_experiment_score(
            &ExperimentScoreReport {
                experiment_id: "demo".to_string(),
                manifest_path: PathBuf::from("/tmp/demo/experiment.toml"),
                primary_metric: MetricTarget {
                    name: "latency_ms".to_string(),
                    goal: MetricGoal::Minimize,
                    unit: Some("ms".to_string()),
                    notes: None,
                },
                baseline_run_count: 2,
                baseline_primary_value: Some(10.0),
                entries: vec![CandidateScore {
                    candidate_id: "cand-a".to_string(),
                    template: "locality".to_string(),
                    run_count: 1,
                    primary_candidate_value: Some(8.0),
                    primary_improvement_pct: Some(20.0),
                    decision: CandidateDecision::Promote,
                    breached_guardrails: Vec::new(),
                }],
            },
            OutputStyle::Table,
        );
        assert!(rendered.contains("Experiment Score · demo"));
        assert!(rendered.contains("cand-a"));
        assert!(rendered.contains("20.00%"));
    }

    #[test]
    fn renders_experiment_artifact_summary() {
        let rendered = render_experiment_artifact(&ExperimentArtifact {
            action: "initialized",
            experiment_id: "demo".to_string(),
            manifest_path: PathBuf::from("/tmp/demo/experiment.toml"),
        });
        assert!(rendered.contains("initialized experiment demo"));
    }
}
