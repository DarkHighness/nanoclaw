use crate::daemon_projection::{
    DaemonProjectionDescriptor, DaemonProjectionKind, find_projection_capabilities,
};
use crate::daemon_protocol::{
    ActiveDeploymentSnapshot, DaemonCapabilityDescriptor, DaemonCapabilityResult,
    DaemonLogsSnapshot, DaemonStatusSnapshot, DeploymentExitSnapshot, PerfCallGraphMode,
    PerfCollectionMode, PerfCollectionSnapshot, PerfTargetSelector, SchedClawDaemonResponse,
    SchedCollectionSnapshot,
};
use crate::doctor::DoctorReport;
use crate::history::{LoadedSessionDetail, SessionExportArtifact, SessionExportKind, preview_id};
use agent::{Skill, ToolKind, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
use clap::ValueEnum;
use std::fmt::Write as _;
use store::{SessionSearchResult, SessionSummary, SessionTokenUsageReport};
use unicode_width::UnicodeWidthStr;

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

pub fn render_daemon_response(response: &SchedClawDaemonResponse, style: OutputStyle) -> String {
    match response {
        SchedClawDaemonResponse::Status { snapshot } => render_daemon_status(snapshot, style),
        SchedClawDaemonResponse::Capabilities { capabilities } => {
            render_daemon_capabilities(capabilities, style)
        }
        SchedClawDaemonResponse::Logs { snapshot } => render_daemon_logs(snapshot, style),
        SchedClawDaemonResponse::Invocation { result } => render_daemon_invocation(result, style),
        SchedClawDaemonResponse::Error { message } => format!("daemon error: {message}"),
    }
}

fn render_daemon_invocation(result: &DaemonCapabilityResult, style: OutputStyle) -> String {
    match result {
        DaemonCapabilityResult::Rollout { message, snapshot } => {
            let body = render_daemon_status(snapshot, style);
            if body.is_empty() {
                message.clone()
            } else {
                format!("{message}\n\n{body}")
            }
        }
        DaemonCapabilityResult::PerfCapture { snapshot } => render_perf_collection(snapshot, style),
        DaemonCapabilityResult::SchedulerTraceCapture { snapshot } => {
            render_sched_collection(snapshot, style)
        }
    }
}

pub fn render_daemon_capabilities(
    capabilities: &[DaemonCapabilityDescriptor],
    style: OutputStyle,
) -> String {
    render_named_daemon_capabilities("Daemon Capabilities", capabilities, style)
}

pub fn render_daemon_capability_detail(
    descriptor: &DaemonCapabilityDescriptor,
    style: OutputStyle,
) -> String {
    let sections = vec![
        (
            "Overview",
            vec![
                ("Name".to_string(), descriptor.name.as_str().to_string()),
                (
                    "Kind".to_string(),
                    daemon_capability_kind_label(&descriptor.kind).to_string(),
                ),
                (
                    "Requires Root".to_string(),
                    bool_label(descriptor.requires_root),
                ),
                ("Summary".to_string(), descriptor.summary.clone()),
            ],
        ),
        (
            "Contract",
            vec![
                (
                    "Selectors".to_string(),
                    join_or_none(
                        descriptor
                            .selector_kinds
                            .iter()
                            .map(daemon_selector_kind_label)
                            .map(ToString::to_string)
                            .collect(),
                    ),
                ),
                (
                    "Outputs".to_string(),
                    join_or_none(descriptor.outputs.clone()),
                ),
                (
                    "Constraints".to_string(),
                    join_or_none(descriptor.constraints.clone()),
                ),
            ],
        ),
    ];
    render_sections(
        &format!("Daemon Capability · {}", descriptor.name.as_str()),
        &sections,
        style,
        None,
    )
}

pub fn render_daemon_projection_list(
    projections: &[DaemonProjectionDescriptor],
    style: OutputStyle,
) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("Daemon Projections · {}", projections.len())),
            &["Name", "Kind", "Capabilities", "Selectors", "Summary"],
            &projections
                .iter()
                .map(|projection| {
                    vec![
                        projection.name.as_str().to_string(),
                        daemon_projection_kind_label(projection.kind).to_string(),
                        join_or_none(
                            projection
                                .capabilities
                                .iter()
                                .map(|name| name.as_str().to_string())
                                .collect(),
                        ),
                        join_or_none(
                            projection
                                .selectors
                                .iter()
                                .map(daemon_selector_kind_label)
                                .map(ToString::to_string)
                                .collect(),
                        ),
                        projection.summary.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "Daemon Projections ({})", projections.len());
            for projection in projections {
                let _ = writeln!(
                    &mut out,
                    "- {} [{}]",
                    projection.name.as_str(),
                    daemon_projection_kind_label(projection.kind)
                );
                let _ = writeln!(&mut out, "  {}", projection.summary);
                let _ = writeln!(
                    &mut out,
                    "  capabilities: {}",
                    join_or_none(
                        projection
                            .capabilities
                            .iter()
                            .map(|name| name.as_str().to_string())
                            .collect(),
                    )
                );
                let _ = writeln!(
                    &mut out,
                    "  selectors: {}",
                    join_or_none(
                        projection
                            .selectors
                            .iter()
                            .map(daemon_selector_kind_label)
                            .map(ToString::to_string)
                            .collect(),
                    )
                );
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_daemon_projection_detail(
    projection: &DaemonProjectionDescriptor,
    style: OutputStyle,
) -> String {
    let capabilities = find_projection_capabilities(projection);
    let sections = vec![
        (
            "Overview",
            vec![
                ("Name".to_string(), projection.name.as_str().to_string()),
                (
                    "Kind".to_string(),
                    daemon_projection_kind_label(projection.kind).to_string(),
                ),
                ("Summary".to_string(), projection.summary.to_string()),
            ],
        ),
        (
            "Projection",
            vec![
                (
                    "Capabilities".to_string(),
                    join_or_none(
                        projection
                            .capabilities
                            .iter()
                            .map(|name| name.as_str().to_string())
                            .collect(),
                    ),
                ),
                (
                    "Selectors".to_string(),
                    join_or_none(
                        projection
                            .selectors
                            .iter()
                            .map(daemon_selector_kind_label)
                            .map(ToString::to_string)
                            .collect(),
                    ),
                ),
                (
                    "Examples".to_string(),
                    join_or_none(
                        projection
                            .examples
                            .iter()
                            .map(ToString::to_string)
                            .collect(),
                    ),
                ),
            ],
        ),
        (
            "Mapped Capabilities",
            vec![(
                "Contracts".to_string(),
                join_or_none(
                    capabilities
                        .iter()
                        .map(|capability| {
                            format!(
                                "{} [{}]",
                                capability.name.as_str(),
                                daemon_capability_kind_label(&capability.kind)
                            )
                        })
                        .collect(),
                ),
            )],
        ),
    ];
    render_sections(
        &format!("Daemon Projection · {}", projection.name.as_str()),
        &sections,
        style,
        if capabilities.is_empty() {
            None
        } else {
            Some(render_named_daemon_capabilities(
                "Capability Contracts",
                &capabilities,
                style,
            ))
        },
    )
}

fn render_named_daemon_capabilities(
    title: &str,
    capabilities: &[DaemonCapabilityDescriptor],
    style: OutputStyle,
) -> String {
    match style {
        OutputStyle::Table => render_grid(
            Some(format!("{title} · {}", capabilities.len())),
            &[
                "Name",
                "Kind",
                "Selectors",
                "Requires Root",
                "Outputs",
                "Constraints",
            ],
            &capabilities
                .iter()
                .map(|capability| {
                    vec![
                        capability.name.as_str().to_string(),
                        daemon_capability_kind_label(&capability.kind).to_string(),
                        join_or_none(
                            capability
                                .selector_kinds
                                .iter()
                                .map(daemon_selector_kind_label)
                                .map(ToString::to_string)
                                .collect(),
                        ),
                        bool_label(capability.requires_root),
                        join_or_none(capability.outputs.clone()),
                        join_or_none(capability.constraints.clone()),
                    ]
                })
                .collect::<Vec<_>>(),
        ),
        OutputStyle::Plain => {
            let mut out = String::new();
            let _ = writeln!(&mut out, "{title} ({})", capabilities.len());
            for capability in capabilities {
                let _ = writeln!(
                    &mut out,
                    "- {} [{}]",
                    capability.name.as_str(),
                    daemon_capability_kind_label(&capability.kind)
                );
                let _ = writeln!(&mut out, "  {}", capability.summary);
                let _ = writeln!(
                    &mut out,
                    "  selectors: {}",
                    join_or_none(
                        capability
                            .selector_kinds
                            .iter()
                            .map(daemon_selector_kind_label)
                            .map(ToString::to_string)
                            .collect(),
                    )
                );
                let _ = writeln!(&mut out, "  requires_root: {}", capability.requires_root);
                let _ = writeln!(
                    &mut out,
                    "  outputs: {}",
                    join_or_none(capability.outputs.clone())
                );
                let _ = writeln!(
                    &mut out,
                    "  constraints: {}",
                    join_or_none(capability.constraints.clone())
                );
            }
            out.trim_end().to_string()
        }
    }
}

pub fn render_perf_collection(snapshot: &PerfCollectionSnapshot, style: OutputStyle) -> String {
    let selector = match &snapshot.selector {
        PerfTargetSelector::Pid { pids } => {
            format!(
                "pid={}",
                pids.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        PerfTargetSelector::Uid { uid } => format!("uid={uid}"),
        PerfTargetSelector::Gid { gid } => format!("gid={gid}"),
        PerfTargetSelector::Cgroup { path } => format!("cgroup={path}"),
    };
    let call_graph = snapshot
        .call_graph
        .map(perf_call_graph_label)
        .unwrap_or_else(|| "<none>".to_string());
    let events = if snapshot.events.is_empty() {
        "<default>".to_string()
    } else {
        snapshot.events.join(", ")
    };
    render_sections(
        &format!("Perf Capture · {}", snapshot.label),
        &[
            (
                "Overview",
                vec![
                    (
                        "Mode".to_string(),
                        perf_collection_mode_label(snapshot.mode),
                    ),
                    ("Selector".to_string(), selector),
                    (
                        "Resolved PIDs".to_string(),
                        join_or_none(
                            snapshot
                                .resolved_pids
                                .iter()
                                .map(u32::to_string)
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    (
                        "Duration".to_string(),
                        format!("{} ms", snapshot.requested_duration_ms),
                    ),
                    ("Events".to_string(), events),
                    ("Call Graph".to_string(), call_graph),
                    (
                        "Sample Frequency".to_string(),
                        snapshot
                            .sample_frequency_hz
                            .map(|value| format!("{value} Hz"))
                            .unwrap_or_else(|| "<none>".to_string()),
                    ),
                    ("Stop Reason".to_string(), snapshot.stop_reason.clone()),
                    (
                        "Exit".to_string(),
                        format!("code={:?} signal={:?}", snapshot.exit_code, snapshot.signal),
                    ),
                ],
            ),
            (
                "Artifacts",
                vec![
                    ("Output Dir".to_string(), snapshot.output_dir.clone()),
                    (
                        "Primary Output".to_string(),
                        snapshot.primary_output_path.clone(),
                    ),
                    ("Command".to_string(), snapshot.command_path.clone()),
                    ("Selector".to_string(), snapshot.selector_path.clone()),
                    ("Stdout".to_string(), snapshot.stdout_path.clone()),
                    ("Stderr".to_string(), snapshot.stderr_path.clone()),
                ],
            ),
        ],
        style,
        Some(format!("perf argv: {}", snapshot.perf_argv.join(" "))),
    )
}

pub fn render_sched_collection(snapshot: &SchedCollectionSnapshot, style: OutputStyle) -> String {
    let selector = match &snapshot.selector {
        PerfTargetSelector::Pid { pids } => {
            format!(
                "pid={}",
                pids.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        PerfTargetSelector::Uid { uid } => format!("uid={uid}"),
        PerfTargetSelector::Gid { gid } => format!("gid={gid}"),
        PerfTargetSelector::Cgroup { path } => format!("cgroup={path}"),
    };
    render_sections(
        &format!("Scheduler Trace · {}", snapshot.label),
        &[
            (
                "Overview",
                vec![
                    ("Selector".to_string(), selector),
                    (
                        "Resolved PIDs".to_string(),
                        join_or_none(
                            snapshot
                                .resolved_pids
                                .iter()
                                .map(u32::to_string)
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    (
                        "Duration".to_string(),
                        format!("{} ms", snapshot.requested_duration_ms),
                    ),
                    (
                        "Latency By PID".to_string(),
                        bool_label(snapshot.latency_by_pid),
                    ),
                    ("Stop Reason".to_string(), snapshot.stop_reason.clone()),
                    (
                        "Exit".to_string(),
                        format!("code={:?} signal={:?}", snapshot.exit_code, snapshot.signal),
                    ),
                ],
            ),
            (
                "Artifacts",
                vec![
                    ("Output Dir".to_string(), snapshot.output_dir.clone()),
                    ("Data".to_string(), snapshot.data_path.clone()),
                    (
                        "Record Command".to_string(),
                        snapshot.record_command_path.clone(),
                    ),
                    ("Selector".to_string(), snapshot.selector_path.clone()),
                    (
                        "Record Stdout".to_string(),
                        snapshot.record_stdout_path.clone(),
                    ),
                    (
                        "Record Stderr".to_string(),
                        snapshot.record_stderr_path.clone(),
                    ),
                    ("Timehist".to_string(), snapshot.timehist_path.clone()),
                    (
                        "Timehist Command".to_string(),
                        snapshot.timehist_command_path.clone(),
                    ),
                    (
                        "Timehist Stderr".to_string(),
                        snapshot.timehist_stderr_path.clone(),
                    ),
                    ("Latency".to_string(), snapshot.latency_path.clone()),
                    (
                        "Latency Command".to_string(),
                        snapshot.latency_command_path.clone(),
                    ),
                    (
                        "Latency Stderr".to_string(),
                        snapshot.latency_stderr_path.clone(),
                    ),
                ],
            ),
        ],
        style,
        Some(format!(
            "record argv: {}\ntimehist argv: {}\nlatency argv: {}",
            snapshot.record_argv.join(" "),
            snapshot.timehist_argv.join(" "),
            snapshot.latency_argv.join(" ")
        )),
    )
}

fn perf_collection_mode_label(mode: PerfCollectionMode) -> String {
    match mode {
        PerfCollectionMode::Stat => "stat".to_string(),
        PerfCollectionMode::Record => "record".to_string(),
    }
}

fn perf_call_graph_label(mode: PerfCallGraphMode) -> String {
    match mode {
        PerfCallGraphMode::FramePointer => "frame_pointer".to_string(),
        PerfCallGraphMode::Dwarf => "dwarf".to_string(),
        PerfCallGraphMode::Lbr => "lbr".to_string(),
    }
}

fn daemon_projection_kind_label(kind: DaemonProjectionKind) -> &'static str {
    match kind {
        DaemonProjectionKind::Discovery => "discovery",
        DaemonProjectionKind::Invocation => "invocation",
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
        (
            "Skill Helpers".to_string(),
            report.helper_script_count.to_string(),
        ),
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
        (
            "Expected Daemon Capabilities".to_string(),
            report.expected_daemon_capabilities.len().to_string(),
        ),
        (
            "Advertised Daemon Capabilities".to_string(),
            report.daemon_capabilities.len().to_string(),
        ),
    ];

    let appendix = match style {
        OutputStyle::Table => {
            let mut sections = Vec::new();
            if !report.expected_daemon_capabilities.is_empty() {
                sections.push(render_named_daemon_capabilities(
                    "Expected Daemon Capabilities",
                    &report.expected_daemon_capabilities,
                    OutputStyle::Table,
                ));
            }
            if !report.daemon_capabilities.is_empty() {
                sections.push(render_named_daemon_capabilities(
                    "Advertised Daemon Capabilities",
                    &report.daemon_capabilities,
                    OutputStyle::Table,
                ));
            }
            sections.push(render_grid(
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
            ));
            sections.join("\n\n")
        }
        OutputStyle::Plain => {
            let mut out = String::new();
            if !report.expected_daemon_capabilities.is_empty() {
                let _ = writeln!(
                    &mut out,
                    "{}",
                    render_named_daemon_capabilities(
                        "Expected Daemon Capabilities",
                        &report.expected_daemon_capabilities,
                        OutputStyle::Plain
                    )
                );
                let _ = writeln!(&mut out);
            }
            if !report.daemon_capabilities.is_empty() {
                let _ = writeln!(
                    &mut out,
                    "{}",
                    render_named_daemon_capabilities(
                        "Advertised Daemon Capabilities",
                        &report.daemon_capabilities,
                        OutputStyle::Plain
                    )
                );
                let _ = writeln!(&mut out);
            }
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

fn daemon_capability_kind_label(
    kind: &crate::daemon_protocol::DaemonCapabilityKind,
) -> &'static str {
    match kind {
        crate::daemon_protocol::DaemonCapabilityKind::DeploymentControl => "deployment_control",
        crate::daemon_protocol::DaemonCapabilityKind::PerfStatCapture => "perf_stat_capture",
        crate::daemon_protocol::DaemonCapabilityKind::PerfRecordCapture => "perf_record_capture",
        crate::daemon_protocol::DaemonCapabilityKind::SchedulerTraceCapture => {
            "scheduler_trace_capture"
        }
    }
}

fn daemon_selector_kind_label(kind: &crate::daemon_protocol::DaemonSelectorKind) -> &'static str {
    match kind {
        crate::daemon_protocol::DaemonSelectorKind::Pid => "pid",
        crate::daemon_protocol::DaemonSelectorKind::Uid => "uid",
        crate::daemon_protocol::DaemonSelectorKind::Gid => "gid",
        crate::daemon_protocol::DaemonSelectorKind::Cgroup => "cgroup",
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
        OutputStyle, render_daemon_capabilities, render_daemon_projection_detail,
        render_daemon_projection_list, render_daemon_status, render_doctor_report,
        render_perf_collection, render_sched_collection, render_session_export_artifact,
        render_session_list, render_skill_list, render_tool_list,
    };
    use crate::daemon_projection::{
        DaemonProjectionDescriptor, DaemonProjectionKind, expected_daemon_projections,
    };
    use crate::daemon_protocol::{
        DaemonCapabilityDescriptor, DaemonCapabilityKind, DaemonCapabilityName, DaemonSelectorKind,
        DaemonStatusSnapshot, PerfCollectionMode, PerfCollectionSnapshot, PerfTargetSelector,
        SchedCollectionSnapshot,
    };
    use crate::doctor::{DoctorCheck, DoctorReport, DoctorStatus};
    use crate::history::{SessionExportArtifact, SessionExportKind};
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
        let rendered = render_skill_list(&[fixture_skill("triage")], OutputStyle::Plain);
        assert!(rendered.contains("Skills (1)"));
        assert!(rendered.contains("triage"));
    }

    #[test]
    fn renders_session_list_plain_view() {
        let rendered = render_session_list(
            &[SessionSummary {
                session_id: SessionId::new(),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                transcript_message_count: 4,
                agent_session_count: 1,
                last_user_prompt: Some("hello".to_string()),
                token_usage: None,
            }],
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Sessions (1)"));
        assert!(rendered.contains("prompt: hello"));
    }

    #[test]
    fn renders_doctor_report_summary() {
        let report = DoctorReport {
            workspace_root: "/repo".into(),
            app_state_dir: "/repo/.nanoclaw/apps/sched-claw".into(),
            daemon_socket: "/repo/.nanoclaw/apps/sched-claw/sched-claw.sock".into(),
            provider: "openai".to_string(),
            model_alias: "gpt_5_4_default".to_string(),
            model_name: "gpt-5.4".to_string(),
            helper_script_count: 4,
            configured_skill_roots: Vec::new(),
            expected_daemon_capabilities: vec![DaemonCapabilityDescriptor {
                name: DaemonCapabilityName::DeploymentControl,
                kind: DaemonCapabilityKind::DeploymentControl,
                summary: "rollout".to_string(),
                selector_kinds: Vec::new(),
                outputs: vec!["status".to_string()],
                constraints: vec!["bounded".to_string()],
                requires_root: true,
            }],
            daemon_capabilities: vec![DaemonCapabilityDescriptor {
                name: DaemonCapabilityName::PerfStatCapture,
                kind: DaemonCapabilityKind::PerfStatCapture,
                summary: "capture".to_string(),
                selector_kinds: vec![DaemonSelectorKind::Pid],
                outputs: vec!["perf.stat.csv".to_string()],
                constraints: vec!["duration bounded".to_string()],
                requires_root: true,
            }],
            checks: vec![DoctorCheck {
                category: "runtime",
                name: "provider",
                status: DoctorStatus::Fail,
                detail: "missing".to_string(),
                remediation: Some("set env".to_string()),
            }],
        };
        let rendered = render_doctor_report(&report, OutputStyle::Plain);
        assert!(rendered.contains("Doctor"));
        assert!(rendered.contains("Skill Helpers: 4"));
        assert!(rendered.contains("Expected Daemon Capabilities (1)"));
        assert!(rendered.contains("Advertised Daemon Capabilities (1)"));
        assert!(rendered.contains("[fail] runtime / provider"));
    }

    #[test]
    fn renders_daemon_capabilities_plain_view() {
        let rendered = render_daemon_capabilities(
            &[DaemonCapabilityDescriptor {
                name: DaemonCapabilityName::SchedulerTraceCapture,
                kind: DaemonCapabilityKind::SchedulerTraceCapture,
                summary: "trace".to_string(),
                selector_kinds: vec![DaemonSelectorKind::Pid, DaemonSelectorKind::Cgroup],
                outputs: vec!["perf.sched.data".to_string()],
                constraints: vec!["bounded".to_string()],
                requires_root: true,
            }],
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Daemon Capabilities (1)"));
        assert!(rendered.contains("scheduler_trace_capture"));
        assert!(rendered.contains("selectors: pid, cgroup"));
    }

    #[test]
    fn renders_daemon_projection_list_plain_view() {
        let rendered =
            render_daemon_projection_list(&expected_daemon_projections(), OutputStyle::Plain);
        assert!(rendered.contains("Daemon Projections"));
        assert!(rendered.contains("collect-perf"));
        assert!(rendered.contains("capabilities: perf_stat_capture, perf_record_capture"));
    }

    #[test]
    fn renders_daemon_projection_detail_with_capability_contracts() {
        let rendered = render_daemon_projection_detail(
            &DaemonProjectionDescriptor {
                name: crate::daemon_projection::DaemonProjectionName::CollectSched,
                kind: DaemonProjectionKind::Invocation,
                summary: "trace",
                capabilities: vec![DaemonCapabilityName::SchedulerTraceCapture],
                selectors: vec![DaemonSelectorKind::Pid, DaemonSelectorKind::Cgroup],
                examples: vec!["sched-claw daemon collect-sched --pid 42 --duration-ms 1000"],
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Daemon Projection · collect-sched"));
        assert!(
            rendered
                .contains("Examples: sched-claw daemon collect-sched --pid 42 --duration-ms 1000")
        );
        assert!(rendered.contains("Capability Contracts"));
        assert!(rendered.contains("scheduler_trace_capture"));
    }

    #[test]
    fn renders_daemon_status_sections() {
        let rendered = render_daemon_status(
            &DaemonStatusSnapshot {
                daemon_pid: 42,
                workspace_root: "/repo".to_string(),
                socket_path: "/repo/sock".to_string(),
                allowed_roots: vec!["/repo".to_string()],
                active: None,
                last_exit: None,
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Daemon Status"));
        assert!(rendered.contains("Daemon PID: 42"));
    }

    #[test]
    fn renders_perf_collection_sections() {
        let rendered = render_perf_collection(
            &PerfCollectionSnapshot {
                label: "perf-a".to_string(),
                mode: PerfCollectionMode::Stat,
                selector: PerfTargetSelector::Pid { pids: vec![42] },
                resolved_pids: vec![42],
                requested_duration_ms: 500,
                events: vec!["cycles".to_string()],
                sample_frequency_hz: None,
                call_graph: None,
                output_dir: "/repo/artifacts/perf-a".to_string(),
                primary_output_path: "/repo/artifacts/perf-a/perf.stat.csv".to_string(),
                command_path: "/repo/artifacts/perf-a/perf.command.json".to_string(),
                selector_path: "/repo/artifacts/perf-a/perf.selector.json".to_string(),
                stdout_path: "/repo/artifacts/perf-a/perf.stdout.log".to_string(),
                stderr_path: "/repo/artifacts/perf-a/perf.stderr.log".to_string(),
                started_at_unix_ms: 1,
                ended_at_unix_ms: 2,
                stop_reason: "duration_elapsed".to_string(),
                exit_code: Some(0),
                signal: None,
                perf_argv: vec!["stat".to_string(), "-p".to_string(), "42".to_string()],
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Perf Capture · perf-a"));
        assert!(rendered.contains("Mode: stat"));
        assert!(rendered.contains("Primary Output: /repo/artifacts/perf-a/perf.stat.csv"));
    }

    #[test]
    fn renders_sched_collection_sections() {
        let rendered = render_sched_collection(
            &SchedCollectionSnapshot {
                label: "sched-a".to_string(),
                selector: PerfTargetSelector::Pid { pids: vec![7] },
                resolved_pids: vec![7],
                requested_duration_ms: 500,
                output_dir: "/repo/artifacts/sched-a".to_string(),
                data_path: "/repo/artifacts/sched-a/perf.sched.data".to_string(),
                record_command_path: "/repo/artifacts/sched-a/perf.sched.record.command.json"
                    .to_string(),
                selector_path: "/repo/artifacts/sched-a/perf.sched.selector.json".to_string(),
                record_stdout_path: "/repo/artifacts/sched-a/perf.sched.record.stdout.log"
                    .to_string(),
                record_stderr_path: "/repo/artifacts/sched-a/perf.sched.record.stderr.log"
                    .to_string(),
                timehist_path: "/repo/artifacts/sched-a/perf.sched.timehist.txt".to_string(),
                timehist_command_path: "/repo/artifacts/sched-a/perf.sched.timehist.command.json"
                    .to_string(),
                timehist_stderr_path: "/repo/artifacts/sched-a/perf.sched.timehist.stderr.log"
                    .to_string(),
                latency_path: "/repo/artifacts/sched-a/perf.sched.latency.txt".to_string(),
                latency_command_path: "/repo/artifacts/sched-a/perf.sched.latency.command.json"
                    .to_string(),
                latency_stderr_path: "/repo/artifacts/sched-a/perf.sched.latency.stderr.log"
                    .to_string(),
                latency_by_pid: true,
                started_at_unix_ms: 1,
                ended_at_unix_ms: 2,
                stop_reason: "duration_elapsed".to_string(),
                exit_code: Some(0),
                signal: None,
                record_argv: vec!["sched".to_string(), "record".to_string()],
                timehist_argv: vec!["sched".to_string(), "timehist".to_string()],
                latency_argv: vec!["sched".to_string(), "latency".to_string()],
            },
            OutputStyle::Plain,
        );
        assert!(rendered.contains("Scheduler Trace · sched-a"));
        assert!(rendered.contains("Latency By PID: yes"));
        assert!(rendered.contains("/repo/artifacts/sched-a/perf.sched.timehist.txt"));
    }

    #[test]
    fn renders_export_artifact_summary() {
        let rendered = render_session_export_artifact(&SessionExportArtifact {
            session_id: SessionId::new(),
            output_path: "/tmp/out.txt".into(),
            item_count: 12,
            kind: SessionExportKind::TranscriptText,
        });
        assert!(rendered.contains("Exported transcript"));
        assert!(rendered.contains("/tmp/out.txt"));
    }

    fn fixture_skill(name: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: "fixture".to_string(),
            aliases: vec!["alias".to_string()],
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
                skill_dir: PathBuf::from(format!("/tmp/skills/{name}")),
                hub: None,
                shadowed_copies: Vec::new(),
            },
        }
    }
}
