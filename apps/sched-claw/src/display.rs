use crate::candidate_templates::TemplateSpec;
use crate::daemon_protocol::{
    ActiveDeploymentSnapshot, DaemonLogsSnapshot, DaemonStatusSnapshot, DeploymentExitSnapshot,
    SchedExtDaemonResponse,
};
use crate::doctor::DoctorReport;
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

pub fn render_doctor_report(report: &DoctorReport, style: OutputStyle) -> String {
    let counts = report.counts();
    let overview = vec![
        (
            "Workspace Root".to_string(),
            report.workspace_root.display().to_string(),
        ),
        (
            "App State Dir".to_string(),
            report.app_state_dir.display().to_string(),
        ),
        (
            "Daemon Socket".to_string(),
            report.daemon_socket.display().to_string(),
        ),
        (
            "Primary Model".to_string(),
            format!(
                "{} / {} / {}",
                report.provider, report.model_alias, report.model_name
            ),
        ),
        (
            "Overall".to_string(),
            report.overall_status().as_str().to_string(),
        ),
        ("Checks".to_string(), report.checks.len().to_string()),
        ("Pass".to_string(), counts.pass.to_string()),
        ("Warn".to_string(), counts.warn.to_string()),
        ("Fail".to_string(), counts.fail.to_string()),
        ("Templates".to_string(), report.template_count.to_string()),
        (
            "Configured Skill Roots".to_string(),
            join_or_none(
                report
                    .configured_skill_roots
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
            ),
        ),
    ];

    let appendix = match style {
        OutputStyle::Table => render_grid(
            Some(format!("Doctor Checks · {}", report.checks.len())),
            &["Category", "Check", "Status", "Detail", "Remediation"],
            &report
                .checks
                .iter()
                .map(|check| {
                    vec![
                        check.category.to_string(),
                        check.name.to_string(),
                        check.status.as_str().to_string(),
                        check.detail.clone(),
                        check
                            .remediation
                            .clone()
                            .unwrap_or_else(|| "<none>".to_string()),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Checks:");
            for check in &report.checks {
                let _ = writeln!(
                    &mut out,
                    "- [{}] {} / {}",
                    check.status.as_str(),
                    check.category,
                    check.name
                );
                let _ = writeln!(&mut out, "  detail: {}", check.detail);
                if let Some(remediation) = &check.remediation {
                    let _ = writeln!(&mut out, "  remediation: {remediation}");
                }
            }
            out.trim_end().to_string()
        }
    };

    render_sections("Doctor", &[("Overview", overview)], style, Some(appendix))
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
                "Evidence",
                "Analyses",
                "Designs",
                "Candidates",
                "Decisions",
                "Deployments",
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
                        summary.evidence_count.to_string(),
                        summary.analysis_count.to_string(),
                        summary.design_count.to_string(),
                        summary.candidate_count.to_string(),
                        summary.decision_count.to_string(),
                        summary.deployment_count.to_string(),
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
                    "- {} workload={} primary_metric={} baselines={} evidence={} analyses={} designs={} candidates={} decisions={} deployments={}",
                    summary.experiment_id,
                    summary.workload_name,
                    summary.primary_metric_name,
                    summary.baseline_run_count,
                    summary.evidence_count,
                    summary.analysis_count,
                    summary.design_count,
                    summary.candidate_count,
                    summary.decision_count,
                    summary.deployment_count
                );
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_template_list(templates: &[TemplateSpec], style: OutputStyle) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Templates · {}", templates.len())),
            &["#", "Name", "Summary", "Knobs", "Build Command"],
            &templates
                .iter()
                .enumerate()
                .map(|(index, template)| {
                    vec![
                        (index + 1).to_string(),
                        template.name.to_string(),
                        template.summary.to_string(),
                        template.knob_specs.len().to_string(),
                        template.build_command_template.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Templates ({})", templates.len());
            for template in templates {
                let _ = writeln!(
                    &mut out,
                    "- {} [{} knobs]",
                    template.name,
                    template.knob_specs.len()
                );
                let _ = writeln!(&mut out, "  {}", template.summary);
                let _ = writeln!(&mut out, "  build: {}", template.build_command_template);
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_template_detail(template: &TemplateSpec, style: OutputStyle) -> String {
    let sections = vec![
        (
            "Overview",
            vec![
                ("Name".to_string(), template.name.to_string()),
                ("Summary".to_string(), template.summary.to_string()),
                ("Description".to_string(), template.description.to_string()),
                (
                    "Build Command".to_string(),
                    template.build_command_template.to_string(),
                ),
            ],
        ),
        (
            "Knobs",
            vec![(
                "Defaults".to_string(),
                if template.knob_specs.is_empty() {
                    "<none>".to_string()
                } else {
                    template
                        .knob_specs
                        .iter()
                        .map(|knob| {
                            format!(
                                "{}={} ({})",
                                knob.name, knob.default_value, knob.description
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            )],
        ),
    ];
    render_sections(
        &format!("Template · {}", template.name),
        &sections,
        style,
        None,
    )
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
                    "Target".to_string(),
                    manifest.workload.effective_target().summary(),
                ),
                (
                    "Target Kind".to_string(),
                    manifest
                        .workload
                        .effective_target()
                        .kind_label()
                        .to_string(),
                ),
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
                    "Performance Policy".to_string(),
                    manifest.performance_policy.summary(),
                ),
                (
                    "Collection Policy".to_string(),
                    manifest.collection_policy.summary(),
                ),
                (
                    "Evaluation Policy".to_string(),
                    manifest.evaluation_policy.summary(),
                ),
                (
                    "Search Policy".to_string(),
                    manifest.search_policy.summary(),
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
                    "Evidence Records".to_string(),
                    manifest.evidence.len().to_string(),
                ),
                ("Analyses".to_string(), manifest.analyses.len().to_string()),
                (
                    "Design Notes".to_string(),
                    manifest.designs.len().to_string(),
                ),
                (
                    "Deployments".to_string(),
                    manifest.deployments.len().to_string(),
                ),
                (
                    "Decisions".to_string(),
                    manifest.decisions.len().to_string(),
                ),
                (
                    "Build Attempts".to_string(),
                    count_candidate_builds(manifest).to_string(),
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
        "Baseline Labels\n{}\n{}\n\nEvidence\n{}\n{}\n\nAnalyses\n{}\n{}\n\nDesign Notes\n{}\n{}\n\nCandidates\n{}\n{}\n\nDecisions\n{}\n{}\n\nBuild Attempts\n{}\n{}\n\nDeployments\n{}\n{}",
        "-".repeat("Baseline Labels".len()),
        list_run_labels(&manifest.baseline_runs),
        "-".repeat("Evidence".len()),
        list_evidence_summaries(manifest),
        "-".repeat("Analyses".len()),
        list_analysis_summaries(manifest),
        "-".repeat("Design Notes".len()),
        list_design_summaries(manifest),
        "-".repeat("Candidates".len()),
        list_candidate_summaries(manifest),
        "-".repeat("Decisions".len()),
        list_decision_summaries(manifest),
        "-".repeat("Build Attempts".len()),
        list_build_summaries(manifest),
        "-".repeat("Deployments".len()),
        list_deployment_summaries(manifest)
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
                    "Spread %",
                    "Decision",
                    "Guardrails",
                    "Reasons",
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
                            entry
                                .candidate_primary_relative_spread_pct
                                .map(format_pct)
                                .unwrap_or_else(|| "<missing>".to_string()),
                            entry.decision.as_str().to_string(),
                            render_guardrail_statuses(entry),
                            join_or_none(entry.status_reasons.clone()),
                        ]
                    })
                    .collect::<Vec<_>>(),
            );
            let _ = write!(
                &mut out,
                "\nprimary metric: {} ({})\nevaluation policy: {}\nbaseline runs: {}\nbaseline value: {}\nbaseline spread: {}",
                report.primary_metric.name,
                report.primary_metric.goal.as_str(),
                report.evaluation_policy.summary(),
                report.baseline_run_count,
                report
                    .baseline_primary_value
                    .map(format_float)
                    .unwrap_or_else(|| "<missing>".to_string()),
                report
                    .baseline_primary_relative_spread_pct
                    .map(format_pct)
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
            let _ = writeln!(
                &mut out,
                "evaluation_policy: {}",
                report.evaluation_policy.summary()
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
            let _ = writeln!(
                &mut out,
                "baseline_spread: {}",
                report
                    .baseline_primary_relative_spread_pct
                    .map(format_pct)
                    .unwrap_or_else(|| "<missing>".to_string())
            );
            for entry in &report.entries {
                let _ = writeln!(
                    &mut out,
                    "- {} template={} runs={} value={} improvement={} spread={} decision={}",
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
                    entry
                        .candidate_primary_relative_spread_pct
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
                if !entry.status_reasons.is_empty() {
                    let _ = writeln!(
                        &mut out,
                        "  reasons: {}",
                        join_or_none(entry.status_reasons.clone())
                    );
                }
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_experiment_artifact(artifact: &ExperimentArtifact) -> String {
    let mut line = format!(
        "{} experiment {} at {}",
        artifact.action,
        artifact.experiment_id,
        artifact.manifest_path.display()
    );
    if !artifact.details.is_empty() {
        let details = artifact
            .details
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = write!(&mut line, " [{details}]");
    }
    line
}

pub fn render_candidate_build_capture(
    experiment_id: &str,
    candidate_id: &str,
    manifest_path: &std::path::Path,
    record: &crate::experiment::CandidateBuildRecord,
    style: OutputStyle,
) -> String {
    let sections = vec![
        (
            "Overview",
            vec![
                ("Experiment".to_string(), experiment_id.to_string()),
                ("Candidate".to_string(), candidate_id.to_string()),
                ("Manifest".to_string(), manifest_path.display().to_string()),
                ("Artifact Dir".to_string(), record.artifact_dir.clone()),
                (
                    "Source".to_string(),
                    record
                        .source_path
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Object".to_string(),
                    record
                        .object_path
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
            ],
        ),
        (
            "Build",
            vec![
                (
                    "Status".to_string(),
                    record.build.status.as_str().to_string(),
                ),
                (
                    "Exit Code".to_string(),
                    record
                        .build
                        .exit_code
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Duration (ms)".to_string(),
                    record.build.duration_ms.to_string(),
                ),
                ("Command".to_string(), record.build.command.clone()),
                (
                    "Command File".to_string(),
                    record.build.command_path.clone(),
                ),
                ("Stdout".to_string(), record.build.stdout_path.clone()),
                ("Stderr".to_string(), record.build.stderr_path.clone()),
                (
                    "Summary".to_string(),
                    record
                        .build
                        .summary
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
            ],
        ),
        (
            "Verifier",
            vec![
                (
                    "Backend".to_string(),
                    record.verifier.backend.as_str().to_string(),
                ),
                (
                    "Status".to_string(),
                    record.verifier.status.as_str().to_string(),
                ),
                (
                    "Exit Code".to_string(),
                    record
                        .verifier
                        .exit_code
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                (
                    "Duration (ms)".to_string(),
                    record.verifier.duration_ms.to_string(),
                ),
                ("Command".to_string(), record.verifier.command.clone()),
                (
                    "Command File".to_string(),
                    record.verifier.command_path.clone(),
                ),
                ("Stdout".to_string(), record.verifier.stdout_path.clone()),
                ("Stderr".to_string(), record.verifier.stderr_path.clone()),
                (
                    "Summary".to_string(),
                    record
                        .verifier
                        .summary
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
            ],
        ),
    ];
    render_sections(
        &format!("Build Capture · {} / {}", experiment_id, candidate_id),
        &sections,
        style,
        None,
    )
}

pub fn render_workload_run_capture(
    experiment_id: &str,
    candidate_id: Option<&str>,
    manifest_path: &std::path::Path,
    capture: &crate::run_capture::WorkloadRunCapture,
    style: OutputStyle,
) -> String {
    let sections = vec![
        (
            "Overview",
            vec![
                ("Experiment".to_string(), experiment_id.to_string()),
                (
                    "Candidate".to_string(),
                    candidate_id.unwrap_or("<baseline>").to_string(),
                ),
                ("Manifest".to_string(), manifest_path.display().to_string()),
                ("Label".to_string(), capture.run.label.clone()),
                (
                    "Scheduler".to_string(),
                    capture.run.scheduler.as_str().to_string(),
                ),
                (
                    "Artifact Dir".to_string(),
                    capture.manifest_artifact_dir.clone(),
                ),
                ("Metrics File".to_string(), capture.metrics_path.clone()),
                (
                    "Perf Stat".to_string(),
                    capture
                        .perf_stat
                        .as_ref()
                        .map(|capture| capture.artifact_path.clone())
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
            ],
        ),
        (
            "Execution",
            vec![
                ("Command File".to_string(), capture.command_path.clone()),
                ("Stdout".to_string(), capture.stdout_path.clone()),
                ("Stderr".to_string(), capture.stderr_path.clone()),
                (
                    "Daemon Logs".to_string(),
                    capture
                        .daemon_logs_path
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                ),
                ("Summary".to_string(), capture.summary.clone()),
            ],
        ),
        (
            "Metrics",
            if capture.run.metrics.is_empty() {
                vec![("Metrics".to_string(), "<none>".to_string())]
            } else {
                capture
                    .run
                    .metrics
                    .iter()
                    .map(|(name, value)| (name.clone(), format_float(*value)))
                    .collect::<Vec<_>>()
            },
        ),
    ];
    render_sections(
        &format!("Workload Run · {}", capture.run.label),
        &sections,
        style,
        None,
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
        (
            "Lease Timeout (ms)".to_string(),
            active
                .lease_timeout_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
        ),
        (
            "Lease Expires At (unix_ms)".to_string(),
            active
                .lease_expires_at_unix_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
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
        ("Exit Reason".to_string(), last_exit.exit_reason.clone()),
        (
            "Lease Timeout (ms)".to_string(),
            last_exit
                .lease_timeout_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
        ),
        (
            "Lease Expires At (unix_ms)".to_string(),
            last_exit
                .lease_expires_at_unix_ms
                .map(|value| value.to_string())
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

fn list_evidence_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    if manifest.evidence.is_empty() {
        "<none>".to_string()
    } else {
        manifest
            .evidence
            .iter()
            .map(|evidence| {
                format!(
                    "{} [{}] scheduler={} candidate={} collector={} focus={} artifacts={} metrics={} summary={}",
                    evidence.evidence_id,
                    evidence.kind.as_str(),
                    evidence
                        .scheduler
                        .map(|scheduler| scheduler.as_str().to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                    evidence
                        .candidate_id
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    evidence
                        .collector
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    evidence
                        .focus
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    join_or_none(evidence.artifact_paths.clone()),
                    if evidence.metrics.is_empty() {
                        "<none>".to_string()
                    } else {
                        evidence
                            .metrics
                            .iter()
                            .map(|(name, value)| format!("{name}={}", format_float(*value)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                    evidence
                        .summary
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn list_analysis_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    if manifest.analyses.is_empty() {
        "<none>".to_string()
    } else {
        manifest
            .analyses
            .iter()
            .map(|analysis| {
                format!(
                    "{} [{}] title={} evidence={} facts={} inferences={} unknowns={} recommendations={} summary={}",
                    analysis.analysis_id,
                    analysis.confidence.as_str(),
                    analysis.title,
                    join_or_none(analysis.evidence_ids.clone()),
                    analysis.facts.len(),
                    analysis.inferences.len(),
                    analysis.unknowns.len(),
                    analysis.recommendations.len(),
                    analysis
                        .summary
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn list_design_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    if manifest.designs.is_empty() {
        "<none>".to_string()
    } else {
        manifest
            .designs
            .iter()
            .map(|design| {
                format!(
                    "{} candidate={} levers={} invariants={} code_targets={} risks={} fallback={} refs=evidence:{} analysis:{} summary={}",
                    design.design_id,
                    design
                        .candidate_id
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    design.policy_levers.len(),
                    design.invariants.len(),
                    design.code_targets.len(),
                    design.risks.len(),
                    design.fallback_criteria.len(),
                    join_or_none(design.evidence_ids.clone()),
                    join_or_none(design.analysis_ids.clone()),
                    design
                        .summary
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string())
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
                    "{} [{}] runs={} builds={} source={} object={} last_build={} last_verify={} daemon={} knobs={} lineage={}",
                    candidate.spec.candidate_id,
                    candidate.spec.template,
                    candidate.runs.len(),
                    candidate.builds.len(),
                    candidate
                        .spec
                        .source_path
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    candidate
                        .spec
                        .object_path
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string()),
                    candidate
                        .builds
                        .last()
                        .map(|build| build.build.status.as_str().to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                    candidate
                        .builds
                        .last()
                        .map(|build| build.verifier.status.as_str().to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                    if candidate.spec.daemon_argv.is_empty() {
                        "<none>".to_string()
                    } else {
                        candidate.spec.daemon_argv.join(" ")
                    },
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
                    },
                    format_candidate_lineage(&candidate.spec.lineage)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn list_decision_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    if manifest.decisions.is_empty() {
        "<none>".to_string()
    } else {
        manifest
            .decisions
            .iter()
            .map(|record| {
                format!(
                    "{} candidate={} status={} improvement={} refs=evidence:{} analysis:{} design:{} rationale={}",
                    record.decision_id,
                    record.candidate_id,
                    record.decision.as_str(),
                    record
                        .primary_improvement_pct
                        .map(format_pct)
                        .unwrap_or_else(|| "<none>".to_string()),
                    join_or_none(record.evidence_ids.clone()),
                    join_or_none(record.analysis_ids.clone()),
                    join_or_none(record.design_ids.clone()),
                    record
                        .rationale
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn list_build_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    let entries = manifest
        .candidates
        .iter()
        .flat_map(|candidate| {
            candidate.builds.iter().map(move |build| {
                format!(
                    "{} build={} verify={} artifact={} object={}",
                    candidate.spec.candidate_id,
                    build.build.status.as_str(),
                    build.verifier.status.as_str(),
                    build.artifact_dir,
                    build
                        .object_path
                        .clone()
                        .unwrap_or_else(|| "<none>".to_string())
                )
            })
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        "<none>".to_string()
    } else {
        entries.join("\n")
    }
}

fn count_candidate_builds(manifest: &crate::experiment::ExperimentManifest) -> usize {
    manifest
        .candidates
        .iter()
        .map(|candidate| candidate.builds.len())
        .sum()
}

fn format_candidate_lineage(lineage: &crate::experiment::CandidateLineage) -> String {
    if lineage.is_empty() {
        return "<none>".to_string();
    }
    let mut parts = Vec::new();
    if let Some(parent_candidate_id) = &lineage.parent_candidate_id {
        parts.push(format!("parent={parent_candidate_id}"));
    }
    if !lineage.evidence_ids.is_empty() {
        parts.push(format!("evidence={}", lineage.evidence_ids.join(",")));
    }
    if !lineage.analysis_ids.is_empty() {
        parts.push(format!("analysis={}", lineage.analysis_ids.join(",")));
    }
    if !lineage.design_ids.is_empty() {
        parts.push(format!("design={}", lineage.design_ids.join(",")));
    }
    if let Some(mutation_note) = &lineage.mutation_note {
        parts.push(format!("note={mutation_note}"));
    }
    parts.join(" ")
}

fn list_deployment_summaries(manifest: &crate::experiment::ExperimentManifest) -> String {
    if manifest.deployments.is_empty() {
        "<none>".to_string()
    } else {
        manifest
            .deployments
            .iter()
            .map(|deployment| {
                format!(
                    "{} candidate={} pid={} lease_ms={} argv={}",
                    deployment.label,
                    deployment.candidate_id,
                    deployment.daemon_pid,
                    deployment
                        .lease_timeout_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "<none>".to_string()),
                    if deployment.argv.is_empty() {
                        "<none>".to_string()
                    } else {
                        deployment.argv.join(" ")
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
        OutputStyle, render_candidate_build_capture, render_daemon_status, render_doctor_report,
        render_experiment_artifact, render_experiment_detail, render_experiment_list,
        render_experiment_score, render_session_export_artifact, render_session_list,
        render_skill_list, render_template_list, render_tool_list, render_workload_run_capture,
    };
    use crate::candidate_templates::template_specs;
    use crate::daemon_protocol::DaemonStatusSnapshot;
    use crate::doctor::{DoctorCheck, DoctorReport, DoctorStatus};
    use crate::experiment::{
        AnalysisConfidence, AnalysisRecord, CandidateBuildRecord, CandidateDecision,
        CandidateDecisionRecord, CandidateLineage, CandidateScore, CommandStatus, DesignRecord,
        EvaluationPolicy, EvidenceKind, EvidenceRecord, ExperimentArtifact, ExperimentScoreReport,
        ExperimentSummary, SearchPolicy, StepCommandRecord, VerifierBackend, VerifierCommandRecord,
    };
    use crate::history::{SessionExportArtifact, SessionExportKind};
    use crate::metrics::{MetricGoal, MetricTarget};
    use crate::run_capture::WorkloadRunCapture;
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
    fn renders_doctor_report_summary() {
        let rendered = render_doctor_report(
            &DoctorReport {
                workspace_root: PathBuf::from("/repo"),
                app_state_dir: PathBuf::from("/repo/.nanoclaw/apps/sched-claw"),
                daemon_socket: PathBuf::from("/repo/.nanoclaw/apps/sched-claw/sched-claw.sock"),
                provider: "openai".to_string(),
                model_alias: "gpt_5_4_default".to_string(),
                model_name: "gpt-5.4".to_string(),
                template_count: 4,
                configured_skill_roots: vec![PathBuf::from("/repo/apps/code-agent/skills")],
                checks: vec![
                    DoctorCheck {
                        category: "runtime",
                        name: "selected provider credentials",
                        status: DoctorStatus::Pass,
                        detail: "OPENAI_API_KEY is configured".to_string(),
                        remediation: None,
                    },
                    DoctorCheck {
                        category: "daemon",
                        name: "privileged sched-ext daemon",
                        status: DoctorStatus::Fail,
                        detail: "socket missing".to_string(),
                        remediation: Some("start the daemon".to_string()),
                    },
                ],
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Doctor"));
        assert!(rendered.contains("Overall: fail"));
        assert!(rendered.contains("[fail] daemon / privileged sched-ext daemon"));
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
                evidence_count: 1,
                analysis_count: 1,
                design_count: 1,
                candidate_count: 2,
                decision_count: 1,
                deployment_count: 1,
            }],
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Experiments (1)"));
        assert!(rendered.contains("latency_ms"));
        assert!(rendered.contains("deployments=1"));
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
                evaluation_policy: EvaluationPolicy::default(),
                baseline_run_count: 2,
                baseline_primary_value: Some(10.0),
                baseline_primary_relative_spread_pct: Some(5.0),
                entries: vec![CandidateScore {
                    candidate_id: "cand-a".to_string(),
                    template: "locality".to_string(),
                    run_count: 1,
                    primary_candidate_value: Some(8.0),
                    primary_improvement_pct: Some(20.0),
                    candidate_primary_relative_spread_pct: Some(0.0),
                    decision: CandidateDecision::Promote,
                    breached_guardrails: Vec::new(),
                    status_reasons: Vec::new(),
                }],
            },
            OutputStyle::Table,
        );
        assert!(rendered.contains("Experiment Score · demo"));
        assert!(rendered.contains("cand-a"));
        assert!(rendered.contains("20.00%"));
    }

    #[test]
    fn renders_experiment_detail_with_evidence_sections() {
        let rendered = render_experiment_detail(
            &crate::experiment::LoadedExperiment {
                manifest_path: PathBuf::from("/tmp/demo/experiment.toml"),
                manifest: crate::experiment::ExperimentManifest {
                    version: 1,
                    experiment_id: "demo".to_string(),
                    created_at_unix_ms: 1,
                    updated_at_unix_ms: 2,
                    host: crate::workload::HostFingerprint::capture(),
                    workload: crate::workload::WorkloadContract {
                        name: "bench".to_string(),
                        ..Default::default()
                    },
                    primary_metric: MetricTarget {
                        name: "latency_ms".to_string(),
                        goal: MetricGoal::Minimize,
                        unit: Some("ms".to_string()),
                        notes: None,
                    },
                    performance_policy: Default::default(),
                    collection_policy: Default::default(),
                    evaluation_policy: EvaluationPolicy::default(),
                    search_policy: SearchPolicy {
                        max_candidates: Some(4),
                        max_total_candidate_runs: Some(8),
                        max_runs_per_candidate: Some(3),
                        max_total_builds: Some(6),
                        stop_after_first_promote: true,
                        notes: Some("stop after first strong candidate".to_string()),
                    },
                    guardrails: Vec::new(),
                    baseline_runs: Vec::new(),
                    evidence: vec![EvidenceRecord {
                        evidence_id: "perf-a".to_string(),
                        recorded_at_unix_ms: 1,
                        kind: EvidenceKind::PerfStat,
                        collector: Some("perf stat -d -d".to_string()),
                        focus: Some("ipc".to_string()),
                        phase: Some("baseline".to_string()),
                        scheduler: Some(crate::experiment::SchedulerKind::Cfs),
                        candidate_id: None,
                        artifact_paths: vec!["artifacts/evidence/perf-a.txt".to_string()],
                        metrics: BTreeMap::from([("ipc".to_string(), 1.1)]),
                        summary: Some("ipc remained low".to_string()),
                        notes: Vec::new(),
                    }],
                    analyses: vec![AnalysisRecord {
                        analysis_id: "analysis-a".to_string(),
                        recorded_at_unix_ms: 2,
                        title: "Baseline diagnosis".to_string(),
                        confidence: AnalysisConfidence::Medium,
                        evidence_ids: vec!["perf-a".to_string()],
                        facts: vec!["ipc stayed low".to_string()],
                        inferences: vec!["migration pressure is plausible".to_string()],
                        unknowns: vec!["llc misses missing".to_string()],
                        recommendations: vec!["increase locality bias".to_string()],
                        summary: Some("locality is a likely lever".to_string()),
                    }],
                    designs: vec![DesignRecord {
                        design_id: "design-a".to_string(),
                        recorded_at_unix_ms: 3,
                        title: "Locality candidate".to_string(),
                        candidate_id: Some("cand-a".to_string()),
                        evidence_ids: vec!["perf-a".to_string()],
                        analysis_ids: vec!["analysis-a".to_string()],
                        policy_levers: vec!["prefer same-cpu wakeups".to_string()],
                        invariants: vec!["do not starve remote tasks".to_string()],
                        code_targets: vec!["pick_idle_cpu".to_string()],
                        risks: vec!["throughput can drop".to_string()],
                        fallback_criteria: vec!["rollback on throughput loss".to_string()],
                        summary: Some("first design pass".to_string()),
                    }],
                    candidates: vec![crate::experiment::CandidateRecord {
                        spec: crate::experiment::CandidateSpec {
                            candidate_id: "cand-a".to_string(),
                            template: "latency_guard".to_string(),
                            lineage: CandidateLineage {
                                parent_candidate_id: Some("cand-base".to_string()),
                                evidence_ids: vec!["perf-a".to_string()],
                                analysis_ids: vec!["analysis-a".to_string()],
                                design_ids: vec!["design-a".to_string()],
                                mutation_note: Some("tighten locality bias".to_string()),
                            },
                            source_path: None,
                            object_path: None,
                            build_command: None,
                            daemon_argv: Vec::new(),
                            daemon_cwd: None,
                            daemon_env: BTreeMap::new(),
                            knobs: BTreeMap::new(),
                            notes: None,
                        },
                        runs: Vec::new(),
                        builds: Vec::new(),
                    }],
                    decisions: vec![CandidateDecisionRecord {
                        decision_id: "decision-a".to_string(),
                        candidate_id: "cand-a".to_string(),
                        recorded_at_unix_ms: 4,
                        decision: CandidateDecision::Promote,
                        evidence_ids: vec!["perf-a".to_string()],
                        analysis_ids: vec!["analysis-a".to_string()],
                        design_ids: vec!["design-a".to_string()],
                        primary_improvement_pct: Some(12.5),
                        rationale: Some("latency improved without guardrail breach".to_string()),
                    }],
                    deployments: Vec::new(),
                },
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Evidence Records"));
        assert!(rendered.contains("perf-a [perf_stat]"));
        assert!(rendered.contains("analysis-a [medium]"));
        assert!(rendered.contains("design-a candidate=cand-a"));
        assert!(rendered.contains("Search Policy"));
        assert!(rendered.contains("Collection Policy"));
        assert!(rendered.contains("decision-a candidate=cand-a status=promote"));
        assert!(rendered.contains("lineage=parent=cand-base"));
    }

    #[test]
    fn renders_experiment_artifact_summary() {
        let rendered = render_experiment_artifact(&ExperimentArtifact {
            action: "initialized",
            experiment_id: "demo".to_string(),
            manifest_path: PathBuf::from("/tmp/demo/experiment.toml"),
            details: vec![("candidate".to_string(), "cand-a".to_string())],
        });
        assert!(rendered.contains("initialized experiment demo"));
        assert!(rendered.contains("candidate=cand-a"));
    }

    #[test]
    fn renders_candidate_build_capture_sections() {
        let rendered = render_candidate_build_capture(
            "demo",
            "cand-a",
            std::path::Path::new("/tmp/demo/experiment.toml"),
            &CandidateBuildRecord {
                requested_at_unix_ms: 1,
                artifact_dir: "artifacts/builds/cand-a/1".to_string(),
                source_path: Some("sources/cand-a.bpf.c".to_string()),
                object_path: Some("sources/cand-a.bpf.o".to_string()),
                build: StepCommandRecord {
                    status: CommandStatus::Success,
                    command: "clang ...".to_string(),
                    command_path: "build.command.txt".to_string(),
                    exit_code: Some(0),
                    duration_ms: 12,
                    stdout_path: "build.stdout.log".to_string(),
                    stderr_path: "build.stderr.log".to_string(),
                    summary: Some("build completed successfully".to_string()),
                },
                verifier: VerifierCommandRecord {
                    backend: VerifierBackend::BpftoolProgLoadall,
                    status: CommandStatus::Failed,
                    command: "bpftool ...".to_string(),
                    command_path: "verify.command.txt".to_string(),
                    exit_code: Some(1),
                    duration_ms: 5,
                    stdout_path: "verify.stdout.log".to_string(),
                    stderr_path: "verify.stderr.log".to_string(),
                    summary: Some("libbpf: verifier rejected object".to_string()),
                },
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Build Capture"));
        assert!(rendered.contains("bpftool_prog_loadall"));
        assert!(rendered.contains("build completed successfully"));
    }

    #[test]
    fn renders_workload_run_capture_sections() {
        let rendered = render_workload_run_capture(
            "demo",
            Some("cand-a"),
            std::path::Path::new("/tmp/demo/experiment.toml"),
            &WorkloadRunCapture {
                run: crate::experiment::RecordedRun {
                    label: "cand-a-run".to_string(),
                    recorded_at_unix_ms: 1,
                    scheduler: crate::experiment::SchedulerKind::SchedExt,
                    artifact_dir: "artifacts/runs/cand-a/1".to_string(),
                    metrics: BTreeMap::from([("latency_ms".to_string(), 7.0)]),
                    notes: Some("status=success".to_string()),
                },
                manifest_artifact_dir: "artifacts/runs/cand-a/1".to_string(),
                command_path: "workload.command.txt".to_string(),
                stdout_path: "workload.stdout.log".to_string(),
                stderr_path: "workload.stderr.log".to_string(),
                metrics_path: "metrics.env".to_string(),
                perf_stat: Some(crate::run_capture::PerfStatCapture {
                    artifact_path: "perf.stat.csv".to_string(),
                    collector: "perf stat -x, --no-big-num -e cycles,instructions".to_string(),
                    metrics: BTreeMap::from([("ipc".to_string(), 1.5)]),
                }),
                daemon_logs_path: Some("daemon.logs.txt".to_string()),
                summary: "status=success".to_string(),
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Workload Run"));
        assert!(rendered.contains("daemon.logs.txt"));
        assert!(rendered.contains("perf.stat.csv"));
        assert!(rendered.contains("latency_ms"));
    }

    #[test]
    fn renders_template_list_plain_view() {
        let rendered = render_template_list(template_specs(), OutputStyle::Plain);
        assert!(rendered.contains("Templates"));
        assert!(rendered.contains("dsq_locality"));
    }
}
