use super::state::{InspectorEntry, TranscriptEntry, TranscriptShellDetail, preview_text};
use crate::backend::{
    ArtifactDecisionExecutionOutcome, ArtifactProposalExecutionOutcome, BenchmarkExecutionOutcome,
    ImproveExecutionOutcome, LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction,
    LiveTaskMessageOutcome, LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome,
    LoadedAgentSession, LoadedArtifact, LoadedExperiment, LoadedSession, LoadedSubagentSession,
    LoadedTask, McpPromptSummary, McpResourceSummary, McpServerSummary,
    PersistedAgentSessionSummary, PersistedArtifactSummary, PersistedExperimentSummary,
    PersistedSessionSearchMatch, PersistedSessionSummary, PersistedTaskSummary,
    SessionExportArtifact, SessionExportKind, SessionOperationAction, SessionOperationOutcome,
    StartupDiagnosticsSnapshot, message_to_text, preview_id,
};
use crate::tool_render::{
    ToolDetail, tool_argument_details, tool_arguments_preview_lines, tool_output_details,
};
use agent::types::{
    AgentEnvelopeKind, AgentSessionId, AgentStatus, ArtifactKind, ArtifactLedgerEventEnvelope,
    ArtifactLedgerEventKind, ArtifactPromotionDecisionKind, ExperimentEventEnvelope,
    ExperimentEventKind, ExperimentTarget, HookEvent, Message, PromotionDecisionKind,
    SessionEventEnvelope, SessionEventKind,
};
use store::TokenUsageRecord;

pub(crate) fn format_session_summary_line(summary: &PersistedSessionSummary) -> TranscriptEntry {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    info_summary_entry(
        format!("{}  {}", preview_id(&summary.session_ref), prompt),
        [format!(
            "{} messages · {} events · {} agent sessions · resume {}",
            summary.transcript_message_count,
            summary.event_count,
            summary.worker_session_count,
            summary.resume_support.label()
        )],
    )
}

pub(crate) fn format_agent_session_summary_line(
    summary: &PersistedAgentSessionSummary,
) -> TranscriptEntry {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    info_summary_entry(
        format!(
            "{}  {}",
            preview_id(&summary.agent_session_ref),
            summary.label
        ),
        [format!(
            "session {} · {} messages · {} events · resume {} · prompt {}",
            preview_id(&summary.session_ref),
            summary.transcript_message_count,
            summary.event_count,
            summary.resume_support.label(),
            prompt
        )],
    )
}

pub(crate) fn format_task_summary_line(summary: &PersistedTaskSummary) -> TranscriptEntry {
    info_summary_entry(
        format!("{}  {}", summary.task_id, summary.status),
        [
            format!(
                "role {} · session {}",
                summary.role,
                preview_id(&summary.session_ref)
            ),
            preview_text(&summary.summary, 72),
        ],
    )
}

pub(crate) fn format_live_task_summary_line(summary: &LiveTaskSummary) -> TranscriptEntry {
    info_summary_entry(
        format!("{}  {}", summary.task_id, summary.status),
        [
            format!(
                "role {} · agent {}",
                summary.role,
                preview_id(&summary.agent_id)
            ),
            format!(
                "session {} · agent session {}",
                preview_id(&summary.session_ref),
                preview_id(&summary.agent_session_ref)
            ),
        ],
    )
}

pub(crate) fn format_live_task_spawn_outcome(
    outcome: &LiveTaskSpawnOutcome,
) -> Vec<InspectorEntry> {
    vec![InspectorEntry::transcript(info_summary_entry(
        format!("Spawned task {}", outcome.task.task_id),
        [
            format!("role {}", outcome.task.role),
            format!("status {}", outcome.task.status),
            format!("agent {}", outcome.task.agent_id),
            format!("session {}", outcome.task.session_ref),
            format!("agent session {}", outcome.task.agent_session_ref),
            format!("prompt {}", preview_text(&outcome.prompt, 96)),
        ],
    ))]
}

pub(crate) fn format_session_search_line(result: &PersistedSessionSearchMatch) -> TranscriptEntry {
    let prompt = result
        .summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    info_summary_entry(
        format!("{}  {}", preview_id(&result.summary.session_ref), prompt),
        [format!(
            "{} messages · {} events · {} agent sessions · resume {} · matched {} event(s){}",
            result.summary.transcript_message_count,
            result.summary.event_count,
            result.summary.worker_session_count,
            result.summary.resume_support.label(),
            result.matched_event_count,
            result
                .preview_matches
                .is_empty()
                .then_some(String::new())
                .unwrap_or_else(|| {
                    format!(
                        " · preview {}",
                        preview_text(&result.preview_matches.join(" | "), 72)
                    )
                })
        )],
    )
}

pub(crate) fn format_experiment_summary_line(
    summary: &PersistedExperimentSummary,
) -> TranscriptEntry {
    let goal = summary
        .goal
        .as_deref()
        .map(|value| preview_text(value, 40))
        .unwrap_or_else(|| "no goal recorded".to_string());
    let mut details = vec![format!(
        "{} candidates · {} baselines · {} events",
        summary.candidate_count, summary.baseline_count, summary.event_count
    )];
    if let Some(target) = summary.target {
        details.push(format!("target {}", experiment_target_label(target)));
    }
    if let Some(decision) = summary.last_decision {
        details.push(format!(
            "last decision {}",
            promotion_decision_label(decision)
        ));
    }
    if let Some(candidate) = &summary.promoted_candidate_ref {
        details.push(format!("promoted {}", preview_id(candidate)));
    }
    info_summary_entry(
        format!("{}  {}", preview_id(&summary.experiment_ref), goal),
        details,
    )
}

pub(crate) fn format_artifact_summary_line(summary: &PersistedArtifactSummary) -> TranscriptEntry {
    let title = summary
        .latest_version_ref
        .as_deref()
        .map(preview_id)
        .unwrap_or_else(|| "no versions yet".to_string());
    let mut details = vec![format!(
        "{} versions · {} signals · {} tasks · {} cases · {} events",
        summary.version_count,
        summary.source_signal_count,
        summary.source_task_count,
        summary.source_case_count,
        summary.event_count
    )];
    if let Some(kind) = summary.kind {
        details.push(format!("kind {}", artifact_kind_label(kind)));
    }
    if let Some(decision) = summary.last_decision {
        details.push(format!(
            "last decision {}",
            artifact_decision_label(decision)
        ));
    }
    if let Some(version_ref) = &summary.active_version_ref {
        details.push(format!("active {}", preview_id(version_ref)));
    }
    if let Some(version_ref) = &summary.promoted_version_ref {
        details.push(format!("promoted {}", preview_id(version_ref)));
    }
    info_summary_entry(
        format!("{}  {}", preview_id(&summary.artifact_ref), title),
        details,
    )
}

pub(crate) fn format_session_inspector(session: &LoadedSession) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Session"),
        InspectorEntry::field("session ref", session.summary.session_id.to_string()),
        InspectorEntry::field("event count", session.summary.event_count.to_string()),
        InspectorEntry::field(
            "message count",
            session.summary.transcript_message_count.to_string(),
        ),
        InspectorEntry::field(
            "worker sessions",
            session.summary.agent_session_count.to_string(),
        ),
    ];
    if let Some(session_usage) = &session.token_usage.session {
        lines.push(InspectorEntry::section("Token Budget"));
        if let Some(window) = session_usage.ledger.context_window {
            lines.push(InspectorEntry::field(
                "context",
                format!("{} / {}", window.used_tokens, window.max_tokens),
            ));
        }
        lines.push(InspectorEntry::field(
            "session tokens",
            format!(
                "in={} out={} cache={}",
                session_usage.ledger.cumulative_usage.input_tokens,
                session_usage.ledger.cumulative_usage.output_tokens,
                session_usage.ledger.cumulative_usage.cache_read_tokens,
            ),
        ));
    }
    if !session.token_usage.aggregate_usage.is_zero() {
        lines.push(InspectorEntry::field(
            "total tokens",
            format!(
                "in={} out={} prefill={} decode={} cache={}",
                session.token_usage.aggregate_usage.input_tokens,
                session.token_usage.aggregate_usage.output_tokens,
                session.token_usage.aggregate_usage.prefill_tokens,
                session.token_usage.aggregate_usage.decode_tokens,
                session.token_usage.aggregate_usage.cache_read_tokens,
            ),
        ));
    }
    if !session.token_usage.subagents.is_empty() {
        lines.push(InspectorEntry::section("Subagents"));
        lines.push(InspectorEntry::field(
            "subagent count",
            session.token_usage.subagents.len().to_string(),
        ));
        lines.extend(
            session
                .token_usage
                .subagents
                .iter()
                .take(4)
                .map(|record| InspectorEntry::transcript(format_token_usage_record_line(record))),
        );
    }
    if let Some(prompt) = &session.summary.last_user_prompt {
        lines.push(InspectorEntry::section("Prompt"));
        lines.push(InspectorEntry::field(
            "last prompt",
            preview_text(prompt, 80),
        ));
    }
    if !session.agent_session_ids.is_empty() {
        lines.push(InspectorEntry::section("Runtime IDs"));
        lines.push(InspectorEntry::field(
            "runtime sessions",
            session
                .agent_session_ids
                .iter()
                .map(|agent_session_id: &AgentSessionId| preview_id(agent_session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if !session.events.is_empty() {
        lines.push(InspectorEntry::section("Recent Events"));
        lines.extend(
            session
                .events
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|event| InspectorEntry::transcript(format_session_event_line(event))),
        );
    }
    lines
}

pub(crate) fn format_experiment_inspector(experiment: &LoadedExperiment) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Experiment"),
        InspectorEntry::field(
            "experiment ref",
            experiment.summary.experiment_id.to_string(),
        ),
        InspectorEntry::field("event count", experiment.summary.event_count.to_string()),
        InspectorEntry::field(
            "candidate count",
            experiment.summary.candidate_count.to_string(),
        ),
        InspectorEntry::field(
            "baseline count",
            experiment.summary.baseline_count.to_string(),
        ),
    ];
    if let Some(target) = experiment.summary.target {
        lines.push(InspectorEntry::field(
            "target",
            experiment_target_label(target),
        ));
    }
    if let Some(goal) = &experiment.summary.goal {
        lines.push(InspectorEntry::section("Goal"));
        lines.push(InspectorEntry::field("goal", preview_text(goal, 96)));
    }
    if experiment.summary.source_session_id.is_some()
        || experiment.summary.source_agent_session_id.is_some()
    {
        lines.push(InspectorEntry::section("Source"));
        if let Some(session_id) = &experiment.summary.source_session_id {
            lines.push(InspectorEntry::field("session ref", session_id.to_string()));
        }
        if let Some(agent_session_id) = &experiment.summary.source_agent_session_id {
            lines.push(InspectorEntry::field(
                "agent session ref",
                agent_session_id.to_string(),
            ));
        }
    }
    if experiment.summary.promoted_candidate_id.is_some()
        || experiment.summary.last_decision.is_some()
    {
        lines.push(InspectorEntry::section("Decision"));
        if let Some(candidate_id) = &experiment.summary.promoted_candidate_id {
            lines.push(InspectorEntry::field(
                "promoted candidate",
                candidate_id.to_string(),
            ));
        }
        if let Some(decision) = experiment.summary.last_decision {
            lines.push(InspectorEntry::field(
                "last decision",
                promotion_decision_label(decision),
            ));
        }
    }
    if !experiment.events.is_empty() {
        lines.push(InspectorEntry::section("Recent Events"));
        lines.extend(
            experiment
                .events
                .iter()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|event| InspectorEntry::transcript(format_experiment_event_line(event))),
        );
    }
    lines
}

pub(crate) fn format_artifact_inspector(artifact: &LoadedArtifact) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Artifact"),
        InspectorEntry::field("artifact ref", artifact.summary.artifact_id.to_string()),
        InspectorEntry::field("event count", artifact.summary.event_count.to_string()),
        InspectorEntry::field("version count", artifact.summary.version_count.to_string()),
        InspectorEntry::field(
            "source signals",
            artifact.summary.source_signal_count.to_string(),
        ),
        InspectorEntry::field(
            "source tasks",
            artifact.summary.source_task_count.to_string(),
        ),
        InspectorEntry::field(
            "source cases",
            artifact.summary.source_case_count.to_string(),
        ),
    ];
    if let Some(kind) = artifact.summary.kind {
        lines.push(InspectorEntry::field("kind", artifact_kind_label(kind)));
    }
    if artifact.summary.latest_version_id.is_some()
        || artifact.summary.active_version_id.is_some()
        || artifact.summary.promoted_version_id.is_some()
    {
        lines.push(InspectorEntry::section("Versions"));
        if let Some(version_id) = &artifact.summary.latest_version_id {
            lines.push(InspectorEntry::field(
                "latest version",
                version_id.to_string(),
            ));
        }
        if let Some(version_id) = &artifact.summary.active_version_id {
            lines.push(InspectorEntry::field(
                "active version",
                version_id.to_string(),
            ));
        }
        if let Some(version_id) = &artifact.summary.promoted_version_id {
            lines.push(InspectorEntry::field(
                "promoted version",
                version_id.to_string(),
            ));
        }
    }
    if let Some(decision) = artifact.summary.last_decision {
        lines.push(InspectorEntry::section("Decision"));
        lines.push(InspectorEntry::field(
            "last decision",
            artifact_decision_label(decision),
        ));
    }
    if !artifact.events.is_empty() {
        lines.push(InspectorEntry::section("Recent Events"));
        lines.extend(
            artifact
                .events
                .iter()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|event| InspectorEntry::transcript(format_artifact_event_line(event))),
        );
    }
    lines
}

pub(crate) fn format_agent_session_inspector(session: &LoadedAgentSession) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Agent Session"),
        InspectorEntry::field(
            "agent session ref",
            session.summary.agent_session_ref.clone(),
        ),
        InspectorEntry::field("session ref", session.summary.session_ref.clone()),
        InspectorEntry::field("label", session.summary.label.clone()),
        InspectorEntry::field("event count", session.summary.event_count.to_string()),
        InspectorEntry::field(
            "message count",
            session.summary.transcript_message_count.to_string(),
        ),
        InspectorEntry::field("resume", session.summary.resume_support.label()),
    ];
    if let Some(token_usage) = &session.token_usage {
        lines.push(InspectorEntry::section("Token Budget"));
        if let Some(window) = token_usage.ledger.context_window {
            lines.push(InspectorEntry::field(
                "context",
                format!("{} / {}", window.used_tokens, window.max_tokens),
            ));
        }
        lines.push(InspectorEntry::field(
            "agent tokens",
            format!(
                "in={} out={} cache={}",
                token_usage.ledger.cumulative_usage.input_tokens,
                token_usage.ledger.cumulative_usage.output_tokens,
                token_usage.ledger.cumulative_usage.cache_read_tokens,
            ),
        ));
    }
    if let Some(prompt) = &session.summary.last_user_prompt {
        lines.push(InspectorEntry::section("Prompt"));
        lines.push(InspectorEntry::field(
            "last prompt",
            preview_text(prompt, 80),
        ));
    }
    if !session.subagents.is_empty() {
        lines.push(InspectorEntry::section("Spawned Subagents"));
        lines.push(InspectorEntry::field(
            "count",
            session.subagents.len().to_string(),
        ));
        lines.extend(
            session
                .subagents
                .iter()
                .take(6)
                .map(|subagent| InspectorEntry::transcript(format_loaded_subagent_line(subagent))),
        );
    }
    if !session.events.is_empty() {
        lines.push(InspectorEntry::section("Recent Events"));
        lines.extend(
            session
                .events
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|event| InspectorEntry::transcript(format_session_event_line(event))),
        );
    }
    lines
}

pub(crate) fn format_task_inspector(task: &LoadedTask) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Task"),
        InspectorEntry::field("task id", task.summary.task_id.clone()),
        InspectorEntry::field("session ref", task.summary.session_ref.clone()),
        InspectorEntry::field(
            "parent agent session ref",
            task.summary.parent_agent_session_ref.clone(),
        ),
        InspectorEntry::field("role", task.summary.role.clone()),
        InspectorEntry::field("status", task.summary.status.to_string()),
        InspectorEntry::field("summary", task.summary.summary.clone()),
    ];
    if let Some(child_session_ref) = &task.summary.child_session_ref {
        lines.push(InspectorEntry::section("Runtime"));
        lines.push(InspectorEntry::field(
            "child session ref",
            child_session_ref.clone(),
        ));
        if let Some(child_agent_session_ref) = &task.summary.child_agent_session_ref {
            lines.push(InspectorEntry::field(
                "child agent session ref",
                child_agent_session_ref.clone(),
            ));
        }
    }
    lines.push(InspectorEntry::section("Prompt"));
    lines.push(InspectorEntry::field(
        "prompt",
        preview_text(&task.spec.prompt, 96),
    ));
    if let Some(steer) = &task.spec.steer {
        lines.push(InspectorEntry::field("steer", preview_text(steer, 96)));
    }
    if !task.spec.requested_write_set.is_empty() {
        lines.push(InspectorEntry::field(
            "writes",
            preview_text(&task.spec.requested_write_set.join(", "), 96),
        ));
    }
    if !task.spec.dependency_ids.is_empty() {
        lines.push(InspectorEntry::field(
            "deps",
            preview_text(&task.spec.dependency_ids.join(", "), 96),
        ));
    }
    if let Some(token_usage) = &task.token_usage {
        lines.push(InspectorEntry::section("Token Budget"));
        if let Some(window) = token_usage.ledger.context_window {
            lines.push(InspectorEntry::field(
                "context",
                format!("{} / {}", window.used_tokens, window.max_tokens),
            ));
        }
        lines.push(InspectorEntry::field(
            "task tokens",
            format!(
                "in={} out={} cache={}",
                token_usage.ledger.cumulative_usage.input_tokens,
                token_usage.ledger.cumulative_usage.output_tokens,
                token_usage.ledger.cumulative_usage.cache_read_tokens,
            ),
        ));
    }
    if let Some(result) = &task.result {
        lines.push(InspectorEntry::section("Result"));
        lines.push(InspectorEntry::field(
            "result",
            preview_text(&result.summary, 96),
        ));
        if !result.claimed_files.is_empty() {
            lines.push(InspectorEntry::field(
                "claimed files",
                preview_text(&result.claimed_files.join(", "), 96),
            ));
        }
    }
    if let Some(error) = &task.error {
        lines.push(InspectorEntry::section("Error"));
        lines.push(InspectorEntry::Plain(preview_text(error, 96)));
    }
    if !task.artifacts.is_empty() {
        lines.push(InspectorEntry::section("Artifacts"));
        lines.extend(task.artifacts.iter().take(6).map(|artifact| {
            InspectorEntry::Plain(preview_text(
                &format!("{} {}", artifact.kind, artifact.uri),
                96,
            ))
        }));
    }
    if !task.messages.is_empty() {
        lines.push(InspectorEntry::section("Agent Messages"));
        lines.extend(
            task.messages
                .iter()
                .take(6)
                .map(|message| InspectorEntry::transcript(format_task_message_line(message))),
        );
    }
    lines
}

pub(crate) fn format_session_transcript_lines(session: &LoadedSession) -> Vec<TranscriptEntry> {
    project_transcript_lines(&session.transcript)
}

pub(crate) fn format_visible_transcript_lines(transcript: &[Message]) -> Vec<TranscriptEntry> {
    project_transcript_lines(transcript)
}

pub(crate) fn format_visible_transcript_preview_lines(
    transcript: &[Message],
) -> Vec<TranscriptEntry> {
    project_transcript_lines(transcript)
}

fn project_transcript_lines(transcript: &[Message]) -> Vec<TranscriptEntry> {
    let transcript = transcript
        .iter()
        .map(|message| project_transcript_entry(&message_to_text(message)))
        .collect::<Vec<_>>();
    if transcript.is_empty() {
        vec![TranscriptEntry::AssistantMessage(
            "No transcript messages recorded for this session.".to_string(),
        )]
    } else {
        transcript
    }
}

fn project_transcript_entry(raw: &str) -> TranscriptEntry {
    if let Some(body) = raw.strip_prefix("user> ") {
        TranscriptEntry::UserPrompt(body.to_string())
    } else if let Some(body) = raw.strip_prefix("assistant> ") {
        TranscriptEntry::AssistantMessage(body.to_string())
    } else if let Some(body) = raw.strip_prefix("system> ") {
        TranscriptEntry::AssistantMessage(body.to_string())
    } else if let Some(body) = raw.strip_prefix("tool> ") {
        TranscriptEntry::AssistantMessage(body.to_string())
    } else if let Some(body) = raw.strip_prefix("error> ") {
        error_summary_entry(body.to_string(), std::iter::empty::<String>())
    } else {
        TranscriptEntry::AssistantMessage(raw.to_string())
    }
}

#[derive(Clone, Copy)]
enum SummaryTone {
    Info,
    Success,
    Error,
}

fn summary_entry(
    tone: SummaryTone,
    headline: impl Into<String>,
    details: impl IntoIterator<Item = String>,
) -> TranscriptEntry {
    let detail_lines = details
        .into_iter()
        .filter(|detail| !detail.is_empty())
        .map(|text| TranscriptShellDetail::Raw {
            text,
            continuation: false,
        })
        .collect();
    match tone {
        SummaryTone::Info => TranscriptEntry::shell_summary_details(headline, detail_lines),
        SummaryTone::Success => TranscriptEntry::success_summary_details(headline, detail_lines),
        SummaryTone::Error => TranscriptEntry::error_summary_details(headline, detail_lines),
    }
}

fn info_summary_entry(
    headline: impl Into<String>,
    details: impl IntoIterator<Item = String>,
) -> TranscriptEntry {
    summary_entry(SummaryTone::Info, headline, details)
}

fn success_summary_entry(
    headline: impl Into<String>,
    details: impl IntoIterator<Item = String>,
) -> TranscriptEntry {
    summary_entry(SummaryTone::Success, headline, details)
}

fn error_summary_entry(
    headline: impl Into<String>,
    details: impl IntoIterator<Item = String>,
) -> TranscriptEntry {
    summary_entry(SummaryTone::Error, headline, details)
}

fn experiment_target_label(target: ExperimentTarget) -> &'static str {
    match target {
        ExperimentTarget::Prompt => "prompt",
        ExperimentTarget::Skill => "skill",
        ExperimentTarget::Policy => "policy",
        ExperimentTarget::Workflow => "workflow",
        ExperimentTarget::CodePatch => "code_patch",
    }
}

fn promotion_decision_label(kind: PromotionDecisionKind) -> &'static str {
    match kind {
        PromotionDecisionKind::Promoted => "promoted",
        PromotionDecisionKind::Rejected => "rejected",
        PromotionDecisionKind::RolledBack => "rolled_back",
    }
}

fn artifact_kind_label(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Prompt => "prompt",
        ArtifactKind::Skill => "skill",
        ArtifactKind::Workflow => "workflow",
        ArtifactKind::Hook => "hook",
        ArtifactKind::Verifier => "verifier",
        ArtifactKind::RuntimePatch => "runtime_patch",
    }
}

fn artifact_decision_label(kind: ArtifactPromotionDecisionKind) -> &'static str {
    match kind {
        ArtifactPromotionDecisionKind::Promoted => "promoted",
        ArtifactPromotionDecisionKind::Rejected => "rejected",
        ArtifactPromotionDecisionKind::RolledBack => "rolled_back",
    }
}

fn format_experiment_event_line(event: &ExperimentEventEnvelope) -> TranscriptEntry {
    match &event.event {
        ExperimentEventKind::Started { spec } => info_summary_entry(
            format!(
                "Started {} experiment",
                experiment_target_label(spec.target)
            ),
            [
                format!("goal {}", preview_text(&spec.goal, 96)),
                spec.source_session_id
                    .as_ref()
                    .map(|session_id| format!("source session {}", preview_id(session_id.as_str())))
                    .unwrap_or_default(),
            ],
        ),
        ExperimentEventKind::BaselinePinned { baseline } => info_summary_entry(
            format!("Pinned baseline {}", baseline.label),
            [
                format!("baseline {}", baseline.baseline_id),
                format!("target {}", experiment_target_label(baseline.target)),
            ],
        ),
        ExperimentEventKind::CandidateGenerated { candidate } => info_summary_entry(
            format!("Generated candidate {}", candidate.label),
            [
                format!("candidate {}", candidate.candidate_id),
                format!("baseline {}", candidate.baseline_id),
            ],
        ),
        ExperimentEventKind::CandidateEvaluated { evaluation } => summary_entry(
            if evaluation.passed {
                SummaryTone::Success
            } else {
                SummaryTone::Error
            },
            format!("Evaluated candidate {}", evaluation.candidate_id),
            [
                format!(
                    "passed {} · score {}",
                    evaluation.passed,
                    format_optional_score(evaluation.score)
                ),
                preview_text(&evaluation.summary, 96),
            ],
        ),
        ExperimentEventKind::CandidatePromoted {
            candidate_id,
            decision,
        } => success_summary_entry(
            format!("Promoted candidate {}", candidate_id),
            [preview_text(&decision.reason, 96)],
        ),
        ExperimentEventKind::CandidateRejected {
            candidate_id,
            decision,
        } => error_summary_entry(
            format!("Rejected candidate {}", candidate_id),
            [preview_text(&decision.reason, 96)],
        ),
        ExperimentEventKind::CandidateRolledBack {
            candidate_id,
            decision,
        } => info_summary_entry(
            format!("Rolled back candidate {}", candidate_id),
            [preview_text(&decision.reason, 96)],
        ),
    }
}

fn format_artifact_event_line(event: &ArtifactLedgerEventEnvelope) -> TranscriptEntry {
    match &event.event {
        ArtifactLedgerEventKind::VersionProposed { version } => info_summary_entry(
            format!(
                "Proposed {} version {}",
                artifact_kind_label(version.kind),
                version.label
            ),
            [
                format!("version {}", version.version_id),
                version
                    .parent_version_id
                    .as_ref()
                    .map(|version_id| format!("parent {}", version_id))
                    .unwrap_or_default(),
            ],
        ),
        ArtifactLedgerEventKind::VersionEvaluated {
            version_id,
            evaluation,
        } => info_summary_entry(
            format!("Evaluated version {}", version_id),
            [preview_text(&evaluation.summary, 96)],
        ),
        ArtifactLedgerEventKind::VersionPromoted {
            version_id,
            decision,
        } => success_summary_entry(
            format!("Promoted version {}", version_id),
            [preview_text(&decision.reason, 96)],
        ),
        ArtifactLedgerEventKind::VersionRejected {
            version_id,
            decision,
        } => error_summary_entry(
            format!("Rejected version {}", version_id),
            [preview_text(&decision.reason, 96)],
        ),
        ArtifactLedgerEventKind::VersionRolledBack {
            version_id,
            decision,
        } => info_summary_entry(
            format!("Rolled back version {}", version_id),
            [preview_text(&decision.reason, 96)],
        ),
    }
}

fn format_optional_score(score: Option<f64>) -> String {
    score
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "n/a".to_string())
}

pub(crate) fn format_session_export_result(result: &SessionExportArtifact) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("Export"),
        InspectorEntry::field(
            "export",
            match result.kind {
                SessionExportKind::EventsJsonl => "events jsonl",
                SessionExportKind::TranscriptText => "transcript text",
            },
        ),
        InspectorEntry::field("session ref", result.session_id.to_string()),
        InspectorEntry::field("path", result.output_path.display().to_string()),
        InspectorEntry::field("items", result.item_count.to_string()),
    ]
}

pub(crate) fn format_benchmark_result(result: &BenchmarkExecutionOutcome) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Benchmark"),
        InspectorEntry::field("plan", result.plan_path.display().to_string()),
        InspectorEntry::field("experiment ref", result.result.experiment_id.to_string()),
        InspectorEntry::field(
            "decision",
            promotion_decision_label(result.result.decision.kind),
        ),
        InspectorEntry::field(
            "summary",
            preview_text(&result.result.evaluation.summary, 96),
        ),
    ];
    if let Some(score) = result.result.evaluation.score {
        lines.push(InspectorEntry::field("score", format!("{score:.3}")));
    }
    if !result.result.evaluation.evaluators.is_empty() {
        lines.push(InspectorEntry::section("Evaluators"));
        lines.extend(result.result.evaluation.evaluators.iter().map(|evaluator| {
            InspectorEntry::transcript(summary_entry(
                if evaluator.passed {
                    SummaryTone::Success
                } else {
                    SummaryTone::Error
                },
                format!("{} {}", evaluator.evaluator_name, evaluator.passed),
                [
                    format!("score {}", format_optional_score(evaluator.score)),
                    preview_text(&evaluator.summary, 96),
                ],
            ))
        }));
    }
    lines
}

pub(crate) fn format_improve_result(result: &ImproveExecutionOutcome) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Improve"),
        InspectorEntry::field("plan", result.plan_path.display().to_string()),
        InspectorEntry::field("experiment ref", result.result.experiment_id.to_string()),
        InspectorEntry::field(
            "winner",
            result
                .result
                .winner_candidate_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "none".to_string()),
        ),
        InspectorEntry::field("candidates", result.result.candidates.len().to_string()),
    ];
    if !result.result.candidates.is_empty() {
        lines.push(InspectorEntry::section("Candidate Outcomes"));
        lines.extend(result.result.candidates.iter().map(|outcome| {
            let tone = match outcome.decision.kind {
                PromotionDecisionKind::Promoted => SummaryTone::Success,
                PromotionDecisionKind::Rejected | PromotionDecisionKind::RolledBack => {
                    SummaryTone::Info
                }
            };
            InspectorEntry::transcript(summary_entry(
                tone,
                format!(
                    "{} {}",
                    outcome.candidate.label,
                    promotion_decision_label(outcome.decision.kind)
                ),
                [
                    format!("candidate {}", outcome.candidate.candidate_id),
                    format!("score {}", format_optional_score(outcome.evaluation.score)),
                    preview_text(&outcome.decision.reason, 96),
                ],
            ))
        }));
    }
    lines
}

pub(crate) fn format_proposal_result(
    result: &ArtifactProposalExecutionOutcome,
) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Proposal"),
        InspectorEntry::field("plan", result.plan_path.display().to_string()),
        InspectorEntry::field("artifact ref", result.result.artifact_id.to_string()),
        InspectorEntry::field("version ref", result.result.version.version_id.to_string()),
        InspectorEntry::field(
            "proposal",
            if result.result.proposal.ready {
                "pending promotion"
            } else {
                "rejected"
            },
        ),
        InspectorEntry::field("reason", preview_text(&result.result.proposal.reason, 96)),
        InspectorEntry::field(
            "verification",
            preview_text(&result.result.verification.summary, 96),
        ),
    ];
    if !result.result.run.trace.changed_paths.is_empty() {
        lines.push(InspectorEntry::section("Changed Paths"));
        lines.extend(
            result
                .result
                .run
                .trace
                .changed_paths
                .iter()
                .take(8)
                .map(|path| InspectorEntry::field("path", path.display().to_string())),
        );
    }
    if !result.result.verification.findings.is_empty() {
        lines.push(InspectorEntry::section("Verifier Findings"));
        lines.extend(result.result.verification.findings.iter().map(|finding| {
            let tone = match finding.severity {
                meta::VerificationFindingSeverity::Blocking => SummaryTone::Error,
                meta::VerificationFindingSeverity::Warning => SummaryTone::Info,
            };
            InspectorEntry::transcript(summary_entry(
                tone,
                format!("{} {}", finding.code, finding.summary),
                Vec::<String>::new(),
            ))
        }));
    }
    lines
}

pub(crate) fn format_artifact_decision_result(
    result: &ArtifactDecisionExecutionOutcome,
) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Artifact Decision"),
        InspectorEntry::field("artifact ref", result.artifact_id.to_string()),
        InspectorEntry::field("version ref", result.version_id.to_string()),
        InspectorEntry::field("decision", artifact_decision_label(result.decision.kind)),
        InspectorEntry::field("reason", preview_text(&result.decision.reason, 96)),
    ];
    if let Some(version_id) = &result.summary.active_version_id {
        lines.push(InspectorEntry::field(
            "active version",
            version_id.to_string(),
        ));
    }
    if let Some(version_id) = &result.summary.promoted_version_id {
        lines.push(InspectorEntry::field(
            "promoted version",
            version_id.to_string(),
        ));
    }
    lines
}

pub(crate) fn format_session_operation_outcome(
    outcome: &SessionOperationOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        SessionOperationAction::StartedFresh => "Started new session",
        SessionOperationAction::AlreadyAttached => "Agent session already attached",
        SessionOperationAction::Reattached => "Reattached session",
    };
    let mut details = vec![
        format!("session {}", outcome.session_ref),
        format!("agent session {}", outcome.active_agent_session_ref),
    ];
    if let Some(requested_agent_session_ref) = &outcome.requested_agent_session_ref {
        details.push(format!("requested {}", requested_agent_session_ref));
    }
    vec![InspectorEntry::transcript(match outcome.action {
        SessionOperationAction::StartedFresh | SessionOperationAction::Reattached => {
            success_summary_entry(headline, details)
        }
        SessionOperationAction::AlreadyAttached => info_summary_entry(headline, details),
    })]
}

pub(crate) fn format_live_task_control_outcome(
    outcome: &LiveTaskControlOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        LiveTaskControlAction::Cancelled => format!("Cancelled task {}", outcome.task_id),
        LiveTaskControlAction::AlreadyTerminal => {
            format!("Task {} was already terminal", outcome.task_id)
        }
    };
    vec![InspectorEntry::transcript(match outcome.action {
        LiveTaskControlAction::Cancelled => success_summary_entry(
            headline,
            [
                format!("requested {}", outcome.requested_ref),
                format!("agent {}", outcome.agent_id),
                format!("status {}", outcome.status),
            ],
        ),
        LiveTaskControlAction::AlreadyTerminal => info_summary_entry(
            headline,
            [
                format!("requested {}", outcome.requested_ref),
                format!("agent {}", outcome.agent_id),
                format!("status {}", outcome.status),
            ],
        ),
    })]
}

pub(crate) fn format_live_task_message_outcome(
    outcome: &LiveTaskMessageOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        LiveTaskMessageAction::Sent => format!("Sent steer message to task {}", outcome.task_id),
        LiveTaskMessageAction::AlreadyTerminal => {
            format!("Task {} was already terminal", outcome.task_id)
        }
    };
    vec![InspectorEntry::transcript(info_summary_entry(
        headline,
        [
            format!("requested {}", outcome.requested_ref),
            format!("agent {}", outcome.agent_id),
            format!("status {}", outcome.status),
            format!("message {}", preview_text(&outcome.message, 96)),
        ],
    ))]
}

pub(crate) fn format_live_task_wait_outcome(outcome: &LiveTaskWaitOutcome) -> Vec<InspectorEntry> {
    let (tone, headline) = match outcome.status {
        AgentStatus::Completed => (
            SummaryTone::Info,
            format!("Finished waiting for task {}", outcome.task_id),
        ),
        AgentStatus::Failed => (
            SummaryTone::Error,
            format!("Finished waiting for task {}", outcome.task_id),
        ),
        AgentStatus::Cancelled => (
            SummaryTone::Error,
            format!("Waiting cancelled for task {}", outcome.task_id),
        ),
        _ => (
            SummaryTone::Info,
            format!("Waiting finished for task {}", outcome.task_id),
        ),
    };
    let mut details = vec![
        format!("requested {}", outcome.requested_ref),
        format!("agent {}", outcome.agent_id),
        format!("status {}", outcome.status),
        format!("summary {}", preview_text(&outcome.summary, 96)),
    ];
    if !outcome.claimed_files.is_empty() {
        details.push(format!(
            "claimed files {}",
            preview_text(&outcome.claimed_files.join(", "), 96)
        ));
    }
    vec![InspectorEntry::transcript(summary_entry(
        tone, headline, details,
    ))]
}

pub(crate) fn format_startup_diagnostics(
    snapshot: &StartupDiagnosticsSnapshot,
) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Runtime"),
        InspectorEntry::field("local tools", snapshot.local_tool_count.to_string()),
        InspectorEntry::field("mcp tools", snapshot.mcp_tool_count.to_string()),
        InspectorEntry::field(
            "plugins",
            format!(
                "{} enabled / {} total",
                snapshot.enabled_plugin_count, snapshot.total_plugin_count
            ),
        ),
        InspectorEntry::field("mcp servers", snapshot.mcp_servers.len().to_string()),
    ];
    if !snapshot.plugin_details.is_empty() {
        lines.push(InspectorEntry::section("Plugins"));
        lines.extend(
            snapshot
                .plugin_details
                .iter()
                .cloned()
                .map(InspectorEntry::Plain),
        );
    }
    if !snapshot.mcp_servers.is_empty() {
        lines.push(InspectorEntry::section("MCP Servers"));
        lines.extend(
            snapshot
                .mcp_servers
                .iter()
                .map(format_mcp_server_summary_line),
        );
    }
    if !snapshot.warnings.is_empty() {
        lines.push(InspectorEntry::section("Warnings"));
        lines.extend(
            snapshot
                .warnings
                .iter()
                .map(|warning| InspectorEntry::Muted(format!("warning: {warning}"))),
        );
    }
    if !snapshot.diagnostics.is_empty() {
        lines.push(InspectorEntry::section("Diagnostics"));
        lines.extend(
            snapshot
                .diagnostics
                .iter()
                .map(|diagnostic| InspectorEntry::Plain(format!("diagnostic: {diagnostic}"))),
        );
    }
    lines
}

pub(crate) fn format_mcp_server_summary_line(summary: &McpServerSummary) -> InspectorEntry {
    InspectorEntry::collection(
        summary.server_name.clone(),
        Some(format!(
            "tools={} prompts={} resources={}",
            summary.tool_count, summary.prompt_count, summary.resource_count
        )),
    )
}

pub(crate) fn format_mcp_prompt_summary_line(summary: &McpPromptSummary) -> InspectorEntry {
    let suffix = if summary.argument_names.is_empty() {
        String::new()
    } else {
        format!(" ({})", summary.argument_names.join(", "))
    };
    InspectorEntry::collection(
        format!("{}:{}{}", summary.server_name, summary.prompt_name, suffix),
        (!summary.description.is_empty()).then_some(summary.description.clone()),
    )
}

pub(crate) fn format_mcp_resource_summary_line(summary: &McpResourceSummary) -> InspectorEntry {
    InspectorEntry::collection(
        format!(
            "{}:{}{}",
            summary.server_name,
            summary.uri,
            summary
                .mime_type
                .as_deref()
                .map(|mime| format!(" [{mime}]"))
                .unwrap_or_default(),
        ),
        (!summary.description.is_empty()).then_some(summary.description.clone()),
    )
}

fn format_token_usage_record_line(record: &TokenUsageRecord) -> TranscriptEntry {
    let name = record
        .agent_name
        .as_deref()
        .or(record.task_id.as_deref())
        .map(|value| preview_text(value, 20))
        .unwrap_or_else(|| preview_id(record.session_id.as_str()));
    TranscriptEntry::AssistantMessage(format!(
        "{} in={} out={} cache={}",
        name,
        record.ledger.cumulative_usage.input_tokens,
        record.ledger.cumulative_usage.output_tokens,
        record.ledger.cumulative_usage.cache_read_tokens,
    ))
}

fn format_loaded_subagent_line(subagent: &LoadedSubagentSession) -> TranscriptEntry {
    let token_summary = subagent
        .token_usage
        .as_ref()
        .map(|usage| {
            format!(
                " in={} out={} cache={}",
                usage.ledger.cumulative_usage.input_tokens,
                usage.ledger.cumulative_usage.output_tokens,
                usage.ledger.cumulative_usage.cache_read_tokens
            )
        })
        .unwrap_or_default();
    TranscriptEntry::AssistantMessage(format!(
        "{} role={} status={} {}{}",
        preview_id(subagent.handle.agent_session_id.as_str()),
        subagent.task.role,
        subagent.status,
        preview_text(&subagent.summary, 28),
        token_summary
    ))
}

fn format_task_message_line(message: &crate::backend::LoadedTaskMessage) -> TranscriptEntry {
    TranscriptEntry::AssistantMessage(format!(
        "{} {}",
        message.channel,
        preview_text(&message.payload.to_string(), 72)
    ))
}

fn format_reason_detail(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("reason {}", preview_text(value, 72)))
}

fn format_hook_event_label(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "session start",
        HookEvent::InstructionsLoaded => "instructions loaded",
        HookEvent::UserPromptSubmit => "prompt submit",
        HookEvent::PreToolUse => "pre-tool hook",
        HookEvent::PermissionRequest => "permission request",
        HookEvent::PostToolUse => "post-tool hook",
        HookEvent::PostToolUseFailure => "post-tool failure hook",
        HookEvent::Notification => "notification hook",
        HookEvent::SubagentStart => "subagent start",
        HookEvent::SubagentStop => "subagent stop",
        HookEvent::Stop => "stop hook",
        HookEvent::StopFailure => "stop failure hook",
        HookEvent::ConfigChange => "config change",
        HookEvent::PreCompact => "pre-compact hook",
        HookEvent::PostCompact => "post-compact hook",
        HookEvent::SessionEnd => "session end",
        HookEvent::Elicitation => "elicitation",
        HookEvent::ElicitationResult => "elicitation result",
    }
}

fn format_tool_origin(origin: &agent::types::ToolOrigin) -> String {
    match origin {
        agent::types::ToolOrigin::Local => "local".to_string(),
        agent::types::ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        agent::types::ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    }
}

fn task_status_summary(task_id: &str, status: &AgentStatus) -> (SummaryTone, String) {
    match status {
        AgentStatus::Completed => (SummaryTone::Success, format!("Task {task_id} completed")),
        AgentStatus::Failed => (SummaryTone::Error, format!("Task {task_id} failed")),
        AgentStatus::Cancelled => (SummaryTone::Error, format!("Task {task_id} cancelled")),
        AgentStatus::WaitingApproval => (
            SummaryTone::Info,
            format!("Task {task_id} is awaiting approval"),
        ),
        AgentStatus::WaitingMessage => (
            SummaryTone::Info,
            format!("Task {task_id} is waiting for a message"),
        ),
        AgentStatus::Queued => (SummaryTone::Info, format!("Task {task_id} is queued")),
        AgentStatus::Running => (SummaryTone::Info, format!("Task {task_id} is running")),
    }
}

fn format_agent_envelope_kind(kind: &AgentEnvelopeKind) -> TranscriptEntry {
    match kind {
        AgentEnvelopeKind::SpawnRequested { task } => info_summary_entry(
            format!("Requested {} task {}", task.role, task.task_id),
            [format!("prompt {}", preview_text(&task.prompt, 72))],
        ),
        AgentEnvelopeKind::Started { task } => info_summary_entry(
            format!("Started {} task {}", task.role, task.task_id),
            [format!("prompt {}", preview_text(&task.prompt, 72))],
        ),
        AgentEnvelopeKind::StatusChanged { status } => match status {
            AgentStatus::Completed => success_summary_entry("Agent completed", []),
            AgentStatus::Failed => error_summary_entry("Agent failed", []),
            AgentStatus::Cancelled => error_summary_entry("Agent cancelled", []),
            AgentStatus::WaitingApproval => info_summary_entry("Agent is awaiting approval", []),
            AgentStatus::WaitingMessage => info_summary_entry("Agent is waiting for a message", []),
            AgentStatus::Queued => info_summary_entry("Agent is queued", []),
            AgentStatus::Running => info_summary_entry("Agent is running", []),
        },
        AgentEnvelopeKind::Message { channel, payload } => info_summary_entry(
            format!("Agent message on {channel}"),
            [format!(
                "payload {}",
                preview_text(&payload.to_string(), 72)
            )],
        ),
        AgentEnvelopeKind::Artifact { artifact } => info_summary_entry(
            format!("Emitted {} artifact", artifact.kind),
            [format!("uri {}", preview_text(&artifact.uri, 72))],
        ),
        AgentEnvelopeKind::ClaimRequested { files } => info_summary_entry(
            "Requested file claim",
            [format!("files {}", preview_text(&files.join(", "), 72))],
        ),
        AgentEnvelopeKind::ClaimGranted { files } => success_summary_entry(
            "Claimed files",
            [format!("files {}", preview_text(&files.join(", "), 72))],
        ),
        AgentEnvelopeKind::ClaimRejected { files, owner } => error_summary_entry(
            "File claim rejected",
            [
                format!("files {}", preview_text(&files.join(", "), 72)),
                format!("owner {}", preview_id(owner.as_str())),
            ],
        ),
        AgentEnvelopeKind::Result { result } => {
            let (tone, headline) = task_status_summary(&result.task_id, &result.status);
            summary_entry(
                tone,
                headline,
                [
                    format!("summary {}", preview_text(&result.summary, 72)),
                    (!result.claimed_files.is_empty())
                        .then(|| {
                            format!(
                                "claimed files {}",
                                preview_text(&result.claimed_files.join(", "), 72)
                            )
                        })
                        .unwrap_or_default(),
                ],
            )
        }
        AgentEnvelopeKind::Failed { error } => error_summary_entry(
            "Agent failed",
            [format!("error {}", preview_text(error, 72))],
        ),
        AgentEnvelopeKind::Cancelled { reason } => error_summary_entry(
            "Agent cancelled",
            [format_reason_detail(reason.as_deref())
                .unwrap_or_else(|| "no reason recorded".to_string())],
        ),
        AgentEnvelopeKind::Heartbeat => info_summary_entry("Agent heartbeat", []),
    }
}

fn format_session_event_line(event: &SessionEventEnvelope) -> TranscriptEntry {
    match &event.event {
        SessionEventKind::SessionStart { reason } => info_summary_entry(
            "Started session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::InstructionsLoaded { count } => info_summary_entry(
            "Loaded instructions",
            [format!("{count} instruction block(s)")],
        ),
        SessionEventKind::SteerApplied { message, reason } => info_summary_entry(
            "Applied steer",
            [
                format!("message {}", preview_text(message, 72)),
                format_reason_detail(reason.as_deref()).unwrap_or_default(),
            ],
        ),
        SessionEventKind::UserPromptSubmit { prompt } => {
            TranscriptEntry::UserPrompt(preview_text(prompt, 96))
        }
        SessionEventKind::ModelRequestStarted { request } => info_summary_entry(
            "Requested model response",
            [
                format!("messages {}", request.messages.len()),
                format!("tools {}", request.tools.len()),
            ],
        ),
        SessionEventKind::CompactionCompleted {
            reason,
            source_message_count,
            retained_message_count,
            summary_chars,
            ..
        } => info_summary_entry(
            "Compacted session context",
            [
                format!("reason {}", preview_text(reason, 48)),
                format!(
                    "messages {} -> {}",
                    source_message_count, retained_message_count
                ),
                format!("summary chars {summary_chars}"),
            ],
        ),
        SessionEventKind::ModelResponseCompleted {
            assistant_text,
            tool_calls,
            ..
        } => info_summary_entry(
            "Finished model response",
            [
                (!assistant_text.trim().is_empty())
                    .then(|| format!("text {}", preview_text(assistant_text, 72)))
                    .unwrap_or_default(),
                (!tool_calls.is_empty())
                    .then(|| format!("tool calls {}", tool_calls.len()))
                    .unwrap_or_default(),
            ],
        ),
        SessionEventKind::TokenUsageUpdated { phase, ledger } => info_summary_entry(
            "Updated token usage",
            [
                format!("phase {:?}", phase),
                format!(
                    "context {}",
                    ledger
                        .context_window
                        .map(|usage| format!("{}/{}", usage.used_tokens, usage.max_tokens))
                        .unwrap_or_else(|| "unknown".to_string())
                ),
                format!(
                    "tokens in={} out={} cache={}",
                    ledger.cumulative_usage.input_tokens,
                    ledger.cumulative_usage.output_tokens,
                    ledger.cumulative_usage.cache_read_tokens
                ),
            ],
        ),
        SessionEventKind::HookInvoked { hook_name, event } => info_summary_entry(
            format!("Running hook {hook_name}"),
            [format!("event {}", format_hook_event_label(*event))],
        ),
        SessionEventKind::HookCompleted {
            hook_name, output, ..
        } => info_summary_entry(
            format!("Finished hook {hook_name}"),
            [format!("effects {}", output.effects.len())],
        ),
        SessionEventKind::TranscriptMessage { message } => {
            project_transcript_entry(&message_to_text(message))
        }
        SessionEventKind::TranscriptMessagePatched {
            message_id,
            message,
        } => info_summary_entry(
            "Updated transcript message",
            [
                format!("message {}", preview_id(message_id.as_str())),
                format!("content {}", preview_text(&message_to_text(message), 72)),
            ],
        ),
        SessionEventKind::TranscriptMessageRemoved { message_id } => info_summary_entry(
            "Removed transcript message",
            [format!("message {}", preview_id(message_id.as_str()))],
        ),
        SessionEventKind::HistoryRollbackApplied {
            anchor_message_id,
            removed_message_count,
        } => info_summary_entry(
            "Applied history rollback",
            [
                format!("anchor {}", preview_id(anchor_message_id.as_str())),
                format!("removed messages {}", removed_message_count),
            ],
        ),
        SessionEventKind::ToolApprovalRequested { call, reasons } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            let mut detail_lines = vec![ToolDetail::Meta(format!(
                "origin {}",
                format_tool_origin(&call.origin)
            ))];
            detail_lines.extend(tool_argument_details(&preview_lines));
            if let Some(reason) = reasons.first() {
                detail_lines.push(ToolDetail::Meta(format!(
                    "reason {}",
                    preview_text(reason, 72)
                )));
            }
            TranscriptEntry::shell_summary_tool_details(
                format!("Awaiting approval for {}", call.tool_name),
                detail_lines,
            )
        }
        SessionEventKind::ToolApprovalResolved {
            call,
            approved,
            reason,
        } => summary_entry(
            if *approved {
                SummaryTone::Success
            } else {
                SummaryTone::Error
            },
            if *approved {
                format!("Approved {}", call.tool_name)
            } else {
                format!("Denied {}", call.tool_name)
            },
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::ToolCallStarted { call } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            TranscriptEntry::shell_summary_tool_details(
                format!("Running {}", call.tool_name),
                tool_argument_details(&preview_lines),
            )
        }
        SessionEventKind::ToolCallCompleted { call, output } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            let mut detail_lines = tool_argument_details(&preview_lines);
            detail_lines.extend(tool_output_details(
                call.tool_name.as_str(),
                &output.text_content(),
                output.structured_content.as_ref(),
            ));
            TranscriptEntry::shell_summary_tool_details(
                format!("Finished {}", call.tool_name),
                detail_lines,
            )
        }
        SessionEventKind::ToolCallFailed { call, error } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            let mut detail_lines = tool_argument_details(&preview_lines);
            detail_lines.push(ToolDetail::Meta(format!(
                "error {}",
                preview_text(error, 72)
            )));
            TranscriptEntry::error_summary_tool_details(
                format!("{} failed", call.tool_name),
                detail_lines,
            )
        }
        SessionEventKind::Notification { source, message } => info_summary_entry(
            format!("Notification from {source}"),
            [format!("message {}", preview_text(message, 72))],
        ),
        SessionEventKind::TaskCreated { task, .. } => info_summary_entry(
            format!("Spawned task {}", task.task_id),
            [
                format!("role {}", task.role),
                format!("claims {}", task.requested_write_set.len()),
                format!("prompt {}", preview_text(&task.prompt, 72)),
            ],
        ),
        SessionEventKind::TaskCompleted {
            task_id,
            agent_id,
            status,
        } => {
            let (tone, headline) = task_status_summary(task_id, status);
            summary_entry(
                tone,
                headline,
                [format!("agent {}", preview_id(agent_id.as_str()))],
            )
        }
        SessionEventKind::SubagentStart { handle, .. } => info_summary_entry(
            format!(
                "Started {} agent {}",
                handle.role,
                preview_id(handle.agent_id.as_str())
            ),
            [format!("task {}", handle.task_id)],
        ),
        SessionEventKind::AgentEnvelope { envelope } => format_agent_envelope_kind(&envelope.kind),
        SessionEventKind::SubagentStop {
            handle,
            result,
            error,
        } => {
            let (tone, headline) = if error.is_some() {
                (
                    SummaryTone::Error,
                    format!("Stopped agent {}", preview_id(handle.agent_id.as_str())),
                )
            } else {
                (
                    SummaryTone::Success,
                    format!("Stopped agent {}", preview_id(handle.agent_id.as_str())),
                )
            };
            summary_entry(
                tone,
                headline,
                [
                    result
                        .as_ref()
                        .map(|value| format!("summary {}", preview_text(&value.summary, 72)))
                        .unwrap_or_default(),
                    error
                        .as_deref()
                        .map(|value| format!("error {}", preview_text(value, 72)))
                        .unwrap_or_default(),
                ],
            )
        }
        SessionEventKind::TurnFailed { stage, error } => error_summary_entry(
            format!("Turn failed in {stage}"),
            [format!("error {}", preview_text(error, 72))],
        ),
        SessionEventKind::Stop { reason } => info_summary_entry(
            "Stopped session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::StopFailure { reason } => error_summary_entry(
            "Failed to stop session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::SessionEnd { reason } => info_summary_entry(
            "Ended session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_agent_session_summary_line, format_benchmark_result, format_experiment_inspector,
        format_experiment_summary_line, format_improve_result, format_live_task_wait_outcome,
        format_session_event_line, format_session_export_result, format_session_operation_outcome,
        format_session_search_line, format_session_summary_line,
    };
    use crate::backend::{
        BenchmarkExecutionOutcome, ImproveExecutionOutcome, LiveTaskWaitOutcome, LoadedExperiment,
        PersistedAgentSessionSummary, PersistedExperimentSummary, PersistedSessionSearchMatch,
        PersistedSessionSummary, ResumeSupport, SessionExportArtifact, SessionExportKind,
        SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot,
    };
    use crate::frontend::tui::state::InspectorEntry;
    use agent::types::{
        AgentSessionId, AgentStatus, ExperimentEventEnvelope, ExperimentEventKind, ExperimentId,
        ExperimentTarget, Message, PromotionDecision, PromotionDecisionKind, SessionEventEnvelope,
        SessionEventKind, SessionId, ToolCall, ToolCallId, ToolOrigin, ToolResult,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use store::ExperimentSummary;

    #[test]
    fn export_result_includes_kind_path_and_item_count() {
        let lines = format_session_export_result(&SessionExportArtifact {
            kind: SessionExportKind::TranscriptText,
            session_id: SessionId::from("session-1"),
            output_path: PathBuf::from("/workspace/out.txt"),
            item_count: 4,
        });
        let lines = inspector_line_texts(&lines);

        assert!(lines.iter().any(|line| line == "export: transcript text"));
        assert!(lines.iter().any(|line| line == "path: /workspace/out.txt"));
        assert!(lines.iter().any(|line| line == "items: 4"));
    }

    #[test]
    fn session_operation_outcome_uses_shell_style_summary() {
        let lines = format_session_operation_outcome(&SessionOperationOutcome {
            action: SessionOperationAction::Reattached,
            session_ref: "session-1".to_string(),
            active_agent_session_ref: "agent-session-2".to_string(),
            requested_agent_session_ref: Some("agent-session-1".to_string()),
            startup: SessionStartupSnapshot::default(),
            transcript: Vec::new(),
        });
        let lines = inspector_line_texts(&lines);

        assert_eq!(lines[0], "✔ Reattached session");
        assert_eq!(lines[1], "  └ session session-1");
        assert_eq!(lines[2], "  └ agent session agent-session-2");
        assert_eq!(lines[3], "  └ requested agent-session-1");
    }

    #[test]
    fn session_summary_uses_two_line_shell_layout() {
        let line = format_session_summary_line(&PersistedSessionSummary {
            session_ref: "session_12345678".to_string(),
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            event_count: 40,
            worker_session_count: 2,
            transcript_message_count: 12,
            last_user_prompt: Some("Refine the approval preview".to_string()),
            resume_support: ResumeSupport::AttachedToActiveRuntime,
        });

        assert_eq!(
            line.serialized(),
            "• session_  Refine the approval preview\n  └ 12 messages · 40 events · 2 agent sessions · resume attached"
        );
    }

    #[test]
    fn agent_session_summary_is_kept_to_two_lines() {
        let line = format_agent_session_summary_line(&PersistedAgentSessionSummary {
            agent_session_ref: "agent_session_123456".to_string(),
            session_ref: "session_123456".to_string(),
            label: "planner".to_string(),
            event_count: 14,
            transcript_message_count: 6,
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            last_user_prompt: Some("Investigate flaky tests".to_string()),
            resume_support: ResumeSupport::AttachedToActiveRuntime,
        });

        assert_eq!(
            line.serialized(),
            "• agent_se  planner\n  └ session session_ · 6 messages · 14 events · resume attached · prompt Investigate flaky tests"
        );
    }

    #[test]
    fn session_search_summary_stays_compact() {
        let line = format_session_search_line(&PersistedSessionSearchMatch {
            summary: PersistedSessionSummary {
                session_ref: "session_12345678".to_string(),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 40,
                worker_session_count: 2,
                transcript_message_count: 12,
                last_user_prompt: Some("Refine the approval preview".to_string()),
                resume_support: ResumeSupport::AttachedToActiveRuntime,
            },
            matched_event_count: 3,
            preview_matches: vec!["bash approval".to_string(), "cargo test".to_string()],
        });

        assert_eq!(
            line.serialized(),
            "• session_  Refine the approval preview\n  └ 12 messages · 40 events · 2 agent sessions · resume attached · matched 3 event(s) · preview bash approval | cargo test"
        );
    }

    #[test]
    fn experiment_summary_stays_compact() {
        let line = format_experiment_summary_line(&PersistedExperimentSummary {
            experiment_ref: "experiment_12345678".to_string(),
            first_timestamp_ms: 1,
            last_timestamp_ms: 2,
            event_count: 7,
            target: Some(ExperimentTarget::Prompt),
            goal: Some("Reduce planner retry churn".to_string()),
            source_session_ref: Some("session_123".to_string()),
            source_agent_session_ref: None,
            baseline_count: 1,
            candidate_count: 2,
            promoted_candidate_ref: Some("candidate_123456".to_string()),
            last_decision: Some(PromotionDecisionKind::Promoted),
        });

        let rendered = line.serialized();
        assert!(rendered.contains("Reduce planner retry churn"));
        assert!(rendered.contains("2 candidates · 1 baselines · 7 events"));
        assert!(rendered.contains("target prompt"));
        assert!(rendered.contains("last decision promoted"));
    }

    #[test]
    fn experiment_inspector_lists_goal_and_recent_events() {
        let lines = format_experiment_inspector(&LoadedExperiment {
            summary: ExperimentSummary {
                experiment_id: ExperimentId::from("experiment-1"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 2,
                target: Some(ExperimentTarget::Workflow),
                goal: Some("Stabilize verifier path".to_string()),
                source_session_id: Some(SessionId::from("session-1")),
                source_agent_session_id: Some(AgentSessionId::from("agent-1")),
                baseline_count: 1,
                candidate_count: 1,
                promoted_candidate_id: None,
                last_decision: Some(PromotionDecisionKind::Rejected),
            },
            events: vec![
                ExperimentEventEnvelope::new(
                    ExperimentId::from("experiment-1"),
                    ExperimentEventKind::Started {
                        spec: agent::types::ExperimentSpec {
                            target: ExperimentTarget::Workflow,
                            goal: "Stabilize verifier path".to_string(),
                            source_session_id: Some(SessionId::from("session-1")),
                            source_agent_session_id: Some(AgentSessionId::from("agent-1")),
                            metadata: json!({}),
                        },
                    },
                ),
                ExperimentEventEnvelope::new(
                    ExperimentId::from("experiment-1"),
                    ExperimentEventKind::CandidateRejected {
                        candidate_id: "candidate-1".into(),
                        decision: PromotionDecision {
                            kind: PromotionDecisionKind::Rejected,
                            reason: "verifier coverage regressed".to_string(),
                        },
                    },
                ),
            ],
        });
        let lines = inspector_line_texts(&lines);

        assert!(lines.iter().any(|line| line == "target: workflow"));
        assert!(
            lines
                .iter()
                .any(|line| line == "goal: Stabilize verifier path")
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Rejected candidate candidate-1"))
        );
    }

    #[test]
    fn benchmark_result_lists_plan_decision_and_evaluators() {
        let lines = format_benchmark_result(&BenchmarkExecutionOutcome {
            plan_path: PathBuf::from("/workspace/plans/benchmark.json"),
            result: meta::BenchmarkRunOutcome {
                experiment_id: ExperimentId::from("experiment-1"),
                evaluation: evals::EvaluationReport::from_evaluator_outcomes(
                    "candidate-1".into(),
                    vec![evals::EvaluatorOutcome {
                        evaluator_name: "score_gate".to_string(),
                        passed: true,
                        score: Some(0.97),
                        summary: "candidate config meets minimum".to_string(),
                        details: None,
                    }],
                ),
                decision: PromotionDecision {
                    kind: PromotionDecisionKind::Promoted,
                    reason: "passed promotion gate".to_string(),
                },
                summary: ExperimentSummary {
                    experiment_id: ExperimentId::from("experiment-1"),
                    first_timestamp_ms: 1,
                    last_timestamp_ms: 2,
                    event_count: 5,
                    target: Some(ExperimentTarget::Policy),
                    goal: Some("stabilize score".to_string()),
                    source_session_id: None,
                    source_agent_session_id: None,
                    baseline_count: 1,
                    candidate_count: 1,
                    promoted_candidate_id: Some("candidate-1".into()),
                    last_decision: Some(PromotionDecisionKind::Promoted),
                },
            },
        });
        let lines = inspector_line_texts(&lines);

        assert!(lines.iter().any(|line| line == "decision: promoted"));
        assert!(
            lines
                .iter()
                .any(|line| line == "plan: /workspace/plans/benchmark.json")
        );
        assert!(lines.iter().any(|line| line.contains("score_gate true")));
    }

    #[test]
    fn improve_result_lists_winner_and_candidate_scores() {
        let lines = format_improve_result(&ImproveExecutionOutcome {
            plan_path: PathBuf::from("/workspace/plans/improve.json"),
            result: meta::ImproveRunOutcome {
                experiment_id: ExperimentId::from("experiment-2"),
                winner_candidate_id: Some("candidate-2".into()),
                candidates: vec![
                    meta::ImprovementCandidateOutcome {
                        candidate: agent::types::CandidateSpec {
                            candidate_id: "candidate-1".into(),
                            baseline_id: "baseline-1".into(),
                            target: ExperimentTarget::Policy,
                            label: "policy-v2".to_string(),
                            description: None,
                            config: json!({"metrics":{"score":0.91}}),
                        },
                        evaluation: evals::EvaluationReport::from_evaluator_outcomes(
                            "candidate-1".into(),
                            vec![evals::EvaluatorOutcome {
                                evaluator_name: "score_gate".to_string(),
                                passed: true,
                                score: Some(0.91),
                                summary: "candidate cleared minimum".to_string(),
                                details: None,
                            }],
                        ),
                        decision: PromotionDecision {
                            kind: PromotionDecisionKind::Rejected,
                            reason: "candidate candidate-1 passed promotion gate but was not the top-scoring promotable variant".to_string(),
                        },
                    },
                    meta::ImprovementCandidateOutcome {
                        candidate: agent::types::CandidateSpec {
                            candidate_id: "candidate-2".into(),
                            baseline_id: "baseline-1".into(),
                            target: ExperimentTarget::Policy,
                            label: "policy-v3".to_string(),
                            description: None,
                            config: json!({"metrics":{"score":0.97}}),
                        },
                        evaluation: evals::EvaluationReport::from_evaluator_outcomes(
                            "candidate-2".into(),
                            vec![evals::EvaluatorOutcome {
                                evaluator_name: "score_gate".to_string(),
                                passed: true,
                                score: Some(0.97),
                                summary: "candidate cleared minimum".to_string(),
                                details: None,
                            }],
                        ),
                        decision: PromotionDecision {
                            kind: PromotionDecisionKind::Promoted,
                            reason: "candidate candidate-2 passed promotion gate".to_string(),
                        },
                    },
                ],
                summary: ExperimentSummary {
                    experiment_id: ExperimentId::from("experiment-2"),
                    first_timestamp_ms: 1,
                    last_timestamp_ms: 3,
                    event_count: 7,
                    target: Some(ExperimentTarget::Policy),
                    goal: Some("pick the best policy".to_string()),
                    source_session_id: None,
                    source_agent_session_id: None,
                    baseline_count: 1,
                    candidate_count: 2,
                    promoted_candidate_id: Some("candidate-2".into()),
                    last_decision: Some(PromotionDecisionKind::Promoted),
                },
            },
        });
        let lines = inspector_line_texts(&lines);

        assert!(lines.iter().any(|line| line == "winner: candidate-2"));
        assert!(lines.iter().any(|line| line.contains("policy-v2 rejected")));
        assert!(lines.iter().any(|line| line.contains("score 0.910")));
        assert!(lines.iter().any(|line| line.contains("policy-v3 promoted")));
    }

    #[test]
    fn live_task_wait_outcome_uses_terminal_status_marker() {
        let lines = format_live_task_wait_outcome(&LiveTaskWaitOutcome {
            requested_ref: "task_1".to_string(),
            agent_id: "agent_1".to_string(),
            task_id: "task_1".to_string(),
            status: AgentStatus::Completed,
            summary: "Updated planner and wrote tests".to_string(),
            claimed_files: vec!["src/lib.rs".to_string()],
        });
        let lines = inspector_line_texts(&lines);

        assert_eq!(lines[0], "• Finished waiting for task task_1");
        assert_eq!(lines[1], "  └ requested task_1");
        assert_eq!(lines[4], "  └ summary Updated planner and wrote tests");
        assert_eq!(lines[5], "  └ claimed files src/lib.rs");
    }

    #[test]
    fn transcript_event_reuses_shell_transcript_prefixes() {
        let event = SessionEventEnvelope::new(
            SessionId::from("session-1"),
            AgentSessionId::from("agent-session-1"),
            None,
            None,
            SessionEventKind::TranscriptMessage {
                message: Message::user("Explain the failing test"),
            },
        );

        assert_eq!(
            format_session_event_line(&event).serialized(),
            "› Explain the failing test"
        );
    }

    #[test]
    fn tool_approval_event_uses_shell_summary_layout() {
        let call = ToolCall {
            id: ToolCallId::from("tool-call-1"),
            call_id: ToolCallId::from("tool-call-1").into(),
            tool_name: "bash".into(),
            arguments: json!({"command": "cargo test"}),
            origin: ToolOrigin::Local,
        };
        let event = SessionEventEnvelope::new(
            SessionId::from("session-1"),
            AgentSessionId::from("agent-session-1"),
            None,
            None,
            SessionEventKind::ToolApprovalRequested {
                call,
                reasons: vec!["sandbox policy requires approval".to_string()],
            },
        );

        assert_eq!(
            format_session_event_line(&event).serialized(),
            "• Awaiting approval for bash\n  └ origin local\n  └ $ cargo test\n  └ reason sandbox policy requires approval"
        );
    }

    #[test]
    fn tool_completion_event_includes_shell_summary_details() {
        let call = ToolCall {
            id: ToolCallId::from("tool-call-1"),
            call_id: ToolCallId::from("tool-call-1").into(),
            tool_name: "bash".into(),
            arguments: json!({"command": "cargo test"}),
            origin: ToolOrigin::Local,
        };
        let output = ToolResult::text(ToolCallId::from("tool-call-1"), "bash", "tests passed");
        let event = SessionEventEnvelope::new(
            SessionId::from("session-1"),
            AgentSessionId::from("agent-session-1"),
            None,
            None,
            SessionEventKind::ToolCallCompleted { call, output },
        );

        assert_eq!(
            format_session_event_line(&event).serialized(),
            "• Finished bash\n  └ $ cargo test\n  └ tests passed"
        );
    }

    #[test]
    fn file_tool_completion_event_includes_diff_block() {
        let call = ToolCall {
            id: ToolCallId::from("tool-call-2"),
            call_id: ToolCallId::from("tool-call-2").into(),
            tool_name: "write".into(),
            arguments: json!({"path": "src/lib.rs"}),
            origin: ToolOrigin::Local,
        };
        let output = ToolResult {
            id: ToolCallId::from("tool-call-2"),
            call_id: ToolCallId::from("tool-call-2").into(),
            tool_name: "write".into(),
            parts: vec![agent::types::MessagePart::text(
                "Wrote 18 bytes to src/lib.rs\n[diff_preview]\n--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()",
            )],
            attachments: Vec::new(),
            structured_content: Some(json!({
                "kind": "success",
                "summary": "Wrote 18 bytes to src/lib.rs",
                "snapshot_before": "snap_old",
                "snapshot_after": "snap_new",
                "file_diffs": [{
                    "path": "src/lib.rs",
                    "preview": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()"
                }]
            })),
            continuation: None,
            metadata: None,
            is_error: false,
        };
        let event = SessionEventEnvelope::new(
            SessionId::from("session-1"),
            AgentSessionId::from("agent-session-1"),
            None,
            None,
            SessionEventKind::ToolCallCompleted { call, output },
        );

        let rendered = format_session_event_line(&event).serialized();
        assert!(rendered.contains("• Finished write"));
        assert!(rendered.contains("  └ diff src/lib.rs"));
        assert!(rendered.contains("@@ -1,1 +1,1 @@"));
        assert!(rendered.contains("+new()"));
    }

    fn inspector_line_texts(lines: &[InspectorEntry]) -> Vec<String> {
        lines
            .iter()
            .flat_map(|line| match line {
                InspectorEntry::Section(text)
                | InspectorEntry::Plain(text)
                | InspectorEntry::Muted(text)
                | InspectorEntry::Command(text) => vec![text.clone()],
                InspectorEntry::Field { key, value } => vec![format!("{key}: {value}")],
                InspectorEntry::Transcript(entry) => entry
                    .serialized()
                    .lines()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>(),
                InspectorEntry::CollectionItem { primary, secondary } => vec![
                    secondary
                        .as_ref()
                        .map(|secondary| format!("{primary}  {secondary}"))
                        .unwrap_or_else(|| primary.clone()),
                ],
                InspectorEntry::Empty => vec![String::new()],
            })
            .collect()
    }
}
