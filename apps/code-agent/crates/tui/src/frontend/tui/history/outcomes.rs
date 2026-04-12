use super::*;

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

pub(crate) fn format_live_monitor_control_outcome(
    outcome: &LiveMonitorControlOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        LiveMonitorControlAction::Stopped => {
            format!("Stopped monitor {}", outcome.monitor.monitor_id)
        }
        LiveMonitorControlAction::AlreadyTerminal => {
            format!(
                "Monitor {} was already terminal",
                outcome.monitor.monitor_id
            )
        }
    };
    vec![InspectorEntry::transcript(match outcome.action {
        LiveMonitorControlAction::Stopped => success_summary_entry(
            headline,
            [
                format!("requested {}", outcome.requested_ref),
                format!("status {}", outcome.monitor.status),
                format!("cwd {}", outcome.monitor.cwd),
                format!("command {}", preview_text(&outcome.monitor.command, 96)),
            ],
        ),
        LiveMonitorControlAction::AlreadyTerminal => info_summary_entry(
            headline,
            [
                format!("requested {}", outcome.requested_ref),
                format!("status {}", outcome.monitor.status),
                format!("cwd {}", outcome.monitor.cwd),
                format!("command {}", preview_text(&outcome.monitor.command, 96)),
            ],
        ),
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
        agent::types::TaskStatus::Completed => (
            SummaryTone::Info,
            format!("Finished waiting for task {}", outcome.task_id),
        ),
        agent::types::TaskStatus::Failed => (
            SummaryTone::Error,
            format!("Finished waiting for task {}", outcome.task_id),
        ),
        agent::types::TaskStatus::Cancelled => (
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
    if !outcome.remaining_live_tasks.is_empty() {
        details.push(format!(
            "still running {}",
            preview_text(
                &outcome
                    .remaining_live_tasks
                    .iter()
                    .map(|task| format!("{} ({}, {})", task.task_id, task.role, task.status))
                    .collect::<Vec<_>>()
                    .join(", "),
                96
            )
        ));
    }
    vec![InspectorEntry::transcript(summary_entry(
        tone, headline, details,
    ))]
}
