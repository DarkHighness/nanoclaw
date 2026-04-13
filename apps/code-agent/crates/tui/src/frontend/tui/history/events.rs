use super::*;
use crate::frontend::tui::state::TranscriptShellStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersistedTranscriptPrefix {
    User,
    Assistant,
    System,
    Tool,
    Error,
}

impl PersistedTranscriptPrefix {
    fn marker(self) -> &'static str {
        match self {
            Self::User => "user> ",
            Self::Assistant => "assistant> ",
            Self::System => "system> ",
            Self::Tool => "tool> ",
            Self::Error => "error> ",
        }
    }

    fn parse(raw: &str) -> Option<(Self, &str)> {
        [
            Self::User,
            Self::Assistant,
            Self::System,
            Self::Tool,
            Self::Error,
        ]
        .into_iter()
        .find_map(|prefix| raw.strip_prefix(prefix.marker()).map(|body| (prefix, body)))
    }
}

fn completed_tool_entry(
    tool_name: &str,
    structured: Option<&serde_json::Value>,
) -> Option<TranscriptEntry> {
    let _ = (tool_name, structured);
    None
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
    match PersistedTranscriptPrefix::parse(raw) {
        Some((PersistedTranscriptPrefix::User, body)) => {
            TranscriptEntry::UserPrompt(body.to_string())
        }
        Some((
            PersistedTranscriptPrefix::Assistant
            | PersistedTranscriptPrefix::System
            | PersistedTranscriptPrefix::Tool,
            body,
        )) => TranscriptEntry::AssistantMessage(body.to_string()),
        Some((PersistedTranscriptPrefix::Error, body)) => {
            error_summary_entry(body.to_string(), std::iter::empty::<String>())
        }
        None => TranscriptEntry::AssistantMessage(raw.to_string()),
    }
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

fn task_status_summary(
    task_id: &agent::types::TaskId,
    status: &agent::types::TaskStatus,
) -> (SummaryTone, String) {
    match status {
        agent::types::TaskStatus::Completed => {
            (SummaryTone::Success, format!("Task {task_id} completed"))
        }
        agent::types::TaskStatus::Failed => (SummaryTone::Error, format!("Task {task_id} failed")),
        agent::types::TaskStatus::Cancelled => {
            (SummaryTone::Error, format!("Task {task_id} cancelled"))
        }
        agent::types::TaskStatus::WaitingApproval => (
            SummaryTone::Info,
            format!("Task {task_id} is awaiting approval"),
        ),
        agent::types::TaskStatus::WaitingMessage => (
            SummaryTone::Info,
            format!("Task {task_id} is waiting for a message"),
        ),
        agent::types::TaskStatus::Open => (SummaryTone::Info, format!("Task {task_id} is open")),
        agent::types::TaskStatus::Queued => {
            (SummaryTone::Info, format!("Task {task_id} is queued"))
        }
        agent::types::TaskStatus::Running => {
            (SummaryTone::Info, format!("Task {task_id} is running"))
        }
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
        AgentEnvelopeKind::Input { message, delivery } => {
            let headline = match delivery {
                agent::types::AgentInputDelivery::Queue => "Agent queued follow-up input",
                agent::types::AgentInputDelivery::Interrupt => {
                    "Agent interrupt restarted with new input"
                }
            };
            info_summary_entry(
                headline,
                [format!(
                    "content {}",
                    preview_text(&message_to_text(message), 72)
                )],
            )
        }
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
            let task_status = agent::types::TaskStatus::from(&result.status);
            let (tone, headline) = task_status_summary(&result.task_id, &task_status);
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

pub(crate) fn format_session_event_line(event: &SessionEventEnvelope) -> TranscriptEntry {
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
            TranscriptEntry::UserPrompt(preview_text(&prompt.preview_text(), 96))
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
        SessionEventKind::ToolApprovalRequested { call, reasons } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            let mut detail_lines = vec![ToolDetail::LabeledValue {
                label: crate::tool_render::ToolDetailLabel::Origin,
                value: format_tool_origin(&call.origin),
            }];
            detail_lines.extend(tool_argument_details(&preview_lines));
            if let Some(reason) = reasons.first() {
                detail_lines.push(ToolDetail::LabeledValue {
                    label: crate::tool_render::ToolDetailLabel::Reason,
                    value: preview_text(reason, 72),
                });
            }
            TranscriptEntry::tool(
                TranscriptToolStatus::WaitingApproval,
                call.tool_name.to_string(),
                detail_lines,
            )
        }
        SessionEventKind::ToolApprovalResolved {
            call,
            approved,
            reason,
        } => TranscriptEntry::tool(
            if *approved {
                TranscriptToolStatus::Approved
            } else {
                TranscriptToolStatus::Denied
            },
            call.tool_name.to_string(),
            format_reason_detail(reason.as_deref())
                .into_iter()
                .map(|value| ToolDetail::LabeledValue {
                    label: crate::tool_render::ToolDetailLabel::Reason,
                    value: value.trim_start_matches("reason ").to_string(),
                })
                .collect(),
        ),
        SessionEventKind::ToolCallStarted { call } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            TranscriptEntry::tool(
                TranscriptToolStatus::Running,
                call.tool_name.to_string(),
                tool_argument_details(&preview_lines),
            )
        }
        SessionEventKind::ToolCallCompleted { call, output } => {
            if let Some(plan_entry) =
                completed_tool_entry(call.tool_name.as_str(), output.structured_content.as_ref())
            {
                return plan_entry;
            }
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            let mut detail_lines = tool_argument_details(&preview_lines);
            detail_lines.extend(tool_output_details(
                call.tool_name.as_str(),
                &output.text_content(),
                output.structured_content.as_ref(),
            ));
            let completion = crate::tool_render::tool_completion_state(
                call.tool_name.as_str(),
                output.structured_content.as_ref(),
            );
            crate::tool_render::compact_successful_exploration_details(
                &mut detail_lines,
                completion,
            );
            TranscriptEntry::tool_with_completion(
                TranscriptToolStatus::Finished,
                call.tool_name.to_string(),
                detail_lines,
                completion,
            )
        }
        SessionEventKind::ToolCallFailed { call, error } => {
            let preview_lines =
                tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
            let mut detail_lines = tool_argument_details(&preview_lines);
            detail_lines.push(ToolDetail::LabeledValue {
                label: crate::tool_render::ToolDetailLabel::Result,
                value: format!("error {}", preview_text(error, 72)),
            });
            TranscriptEntry::tool(
                TranscriptToolStatus::Failed,
                call.tool_name.to_string(),
                detail_lines,
            )
        }
        SessionEventKind::Notification { source, message } => info_summary_entry(
            format!("Notification from {source}"),
            [format!("message {}", preview_text(message, 72))],
        ),
        SessionEventKind::BrowserOpened { summary } => info_summary_entry(
            format!("Opened browser {}", summary.browser_id),
            browser_summary_lines(summary),
        ),
        SessionEventKind::BrowserUpdated { summary } => info_summary_entry(
            format!(
                "{} browser {}",
                browser_status_label(summary.status),
                summary.browser_id
            ),
            browser_summary_lines(summary),
        ),
        SessionEventKind::MonitorStarted { summary } => {
            TranscriptEntry::shell_summary_status_details(
                TranscriptShellStatus::Running,
                format!("Running monitor {}", summary.monitor_id),
                monitor_summary_details(summary),
            )
        }
        SessionEventKind::MonitorEvent { event } => match &event.kind {
            agent::types::MonitorEventKind::Line { stream, text } => info_summary_entry(
                format!("Monitor {} {}", event.monitor_id, stream),
                [preview_text(text, 96)],
            ),
            agent::types::MonitorEventKind::StateChanged { status } => {
                TranscriptEntry::shell_summary_status_details(
                    monitor_shell_status(*status),
                    format!(
                        "{} monitor {}",
                        monitor_status_label(*status),
                        event.monitor_id
                    ),
                    Vec::new(),
                )
            }
            agent::types::MonitorEventKind::Completed { exit_code } => {
                TranscriptEntry::shell_summary_status_details(
                    TranscriptShellStatus::Completed,
                    format!("Completed monitor {}", event.monitor_id),
                    vec![TranscriptShellDetail::Raw {
                        text: format!("exit {}", exit_code),
                        continuation: false,
                    }],
                )
            }
            agent::types::MonitorEventKind::Failed { exit_code, error } => {
                let mut details = Vec::new();
                if let Some(exit_code) = exit_code {
                    details.push(TranscriptShellDetail::Raw {
                        text: format!("exit {}", exit_code),
                        continuation: false,
                    });
                }
                if let Some(error) = error.as_deref() {
                    details.push(TranscriptShellDetail::Raw {
                        text: format!("error {}", preview_text(error, 72)),
                        continuation: false,
                    });
                }
                TranscriptEntry::shell_summary_status_details(
                    TranscriptShellStatus::Failed,
                    format!("Failed monitor {}", event.monitor_id),
                    details,
                )
            }
            agent::types::MonitorEventKind::Cancelled { reason } => {
                TranscriptEntry::shell_summary_status_details(
                    TranscriptShellStatus::Cancelled,
                    format!("Cancelled monitor {}", event.monitor_id),
                    vec![TranscriptShellDetail::Raw {
                        text: reason
                            .as_deref()
                            .map(|value| format!("reason {}", preview_text(value, 72)))
                            .unwrap_or_else(|| "reason not recorded".to_string()),
                        continuation: false,
                    }],
                )
            }
        },
        SessionEventKind::MonitorUpdated { summary } => {
            TranscriptEntry::shell_summary_status_details(
                monitor_shell_status(summary.status),
                format!(
                    "{} monitor {}",
                    monitor_status_label(summary.status),
                    summary.monitor_id
                ),
                monitor_summary_details(summary),
            )
        }
        SessionEventKind::WorktreeEntered { summary } => info_summary_entry(
            format!("Entered worktree {}", summary.worktree_id),
            worktree_summary_lines(summary),
        ),
        SessionEventKind::WorktreeUpdated { summary } => info_summary_entry(
            format!(
                "{} worktree {}",
                worktree_status_label(summary.status),
                summary.worktree_id
            ),
            worktree_summary_lines(summary),
        ),
        SessionEventKind::CheckpointCreated { checkpoint } => info_summary_entry(
            format!("Saved checkpoint {}", checkpoint.checkpoint_id),
            checkpoint_summary_lines(checkpoint),
        ),
        SessionEventKind::CronCreated { summary, .. } => info_summary_entry(
            format!("Created automation {}", summary.cron_id),
            cron_summary_lines(summary),
        ),
        SessionEventKind::CronUpdated { summary } => info_summary_entry(
            format!(
                "{} automation {}",
                cron_status_label(summary.status),
                summary.cron_id
            ),
            cron_summary_lines(summary),
        ),
        SessionEventKind::TaskCreated { task, .. } => info_summary_entry(
            format!("Spawned task {}", task.task_id),
            [
                format!("role {}", task.role),
                format!("claims {}", task.requested_write_set.len()),
                format!("prompt {}", preview_text(&task.prompt, 72)),
            ],
        ),
        SessionEventKind::TaskUpdated {
            task_id,
            status,
            summary,
        } => {
            let (tone, headline) = task_status_summary(task_id, status);
            summary_entry(
                tone,
                headline,
                [summary
                    .as_deref()
                    .map(|summary| format!("summary {}", preview_text(summary, 72)))
                    .unwrap_or_else(|| "summary unchanged".to_string())],
            )
        }
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

fn monitor_shell_status(status: agent::types::MonitorStatus) -> TranscriptShellStatus {
    match status {
        agent::types::MonitorStatus::Running => TranscriptShellStatus::Running,
        agent::types::MonitorStatus::Completed => TranscriptShellStatus::Completed,
        agent::types::MonitorStatus::Failed => TranscriptShellStatus::Failed,
        agent::types::MonitorStatus::Cancelled => TranscriptShellStatus::Cancelled,
    }
}

fn monitor_status_label(status: agent::types::MonitorStatus) -> &'static str {
    match status {
        agent::types::MonitorStatus::Running => "Running",
        agent::types::MonitorStatus::Completed => "Completed",
        agent::types::MonitorStatus::Failed => "Failed",
        agent::types::MonitorStatus::Cancelled => "Cancelled",
    }
}

fn worktree_status_label(status: agent::types::WorktreeStatus) -> &'static str {
    match status {
        agent::types::WorktreeStatus::Active => "Updated active",
        agent::types::WorktreeStatus::Inactive => "Updated inactive",
        agent::types::WorktreeStatus::Removed => "Removed",
    }
}

fn checkpoint_summary_lines(checkpoint: &agent::types::CheckpointRecord) -> Vec<String> {
    let mut lines = vec![
        format!("scope {}", checkpoint.scope),
        format!("files {}", checkpoint.changed_files.len()),
        format!("summary {}", preview_text(&checkpoint.summary, 72)),
    ];
    match &checkpoint.origin {
        agent::types::CheckpointOrigin::FileTool { tool_name } => {
            lines.push(format!("tool {}", tool_name));
        }
        agent::types::CheckpointOrigin::Restore {
            restored_from,
            restore_mode,
        } => {
            lines.push(format!("restore {} {}", restored_from, restore_mode));
        }
    }
    lines
}

fn worktree_summary_lines(summary: &agent::types::WorktreeSummaryRecord) -> [String; 4] {
    [
        format!("scope {}", summary.scope),
        format!("status {}", summary.status),
        format!(
            "root {}",
            preview_text(&summary.root.display().to_string(), 72)
        ),
        summary
            .label
            .as_deref()
            .map(|label| format!("label {}", preview_text(label, 48)))
            .unwrap_or_default(),
    ]
}

fn cron_status_label(status: agent::types::CronStatus) -> &'static str {
    match status {
        agent::types::CronStatus::Scheduled => "Updated scheduled",
        agent::types::CronStatus::Completed => "Completed",
        agent::types::CronStatus::Cancelled => "Cancelled",
        agent::types::CronStatus::Failed => "Failed",
    }
}

fn cron_summary_lines(summary: &agent::types::CronSummaryRecord) -> [String; 4] {
    [
        format!("status {}", summary.status),
        format!("role {}", summary.role),
        format!(
            "schedule {}",
            preview_text(&render_cron_schedule_summary(&summary.schedule), 72)
        ),
        summary
            .latest_task_id
            .as_ref()
            .map(|task_id| format!("latest task {}", task_id))
            .unwrap_or_else(|| format!("summary {}", preview_text(&summary.prompt_summary, 72))),
    ]
}

fn render_cron_schedule_summary(schedule: &agent::types::CronScheduleRecord) -> String {
    match schedule {
        agent::types::CronScheduleRecord::Once { run_at_unix_s } => {
            format!("once at {run_at_unix_s}")
        }
        agent::types::CronScheduleRecord::Recurring {
            interval_seconds,
            next_run_unix_s,
            max_runs,
        } => {
            let mut summary = format!("every {interval_seconds}s next {next_run_unix_s}");
            if let Some(max_runs) = max_runs {
                summary.push_str(&format!(" max {max_runs}"));
            }
            summary
        }
    }
}

fn browser_summary_lines(summary: &agent::types::BrowserSummaryRecord) -> [String; 4] {
    [
        format!("status {}", summary.status),
        format!("url {}", preview_text(&summary.current_url, 72)),
        if summary.headless {
            "mode headless".to_string()
        } else {
            "mode headful".to_string()
        },
        summary
            .viewport
            .as_ref()
            .map(|viewport| format!("viewport {}x{}", viewport.width, viewport.height))
            .or_else(|| {
                summary
                    .title
                    .as_deref()
                    .map(|title| format!("title {}", preview_text(title, 72)))
            })
            .unwrap_or_else(|| "viewport default".to_string()),
    ]
}

fn browser_status_label(status: agent::types::BrowserStatus) -> &'static str {
    match status {
        agent::types::BrowserStatus::Open => "Opened",
        agent::types::BrowserStatus::Closed => "Closed",
        agent::types::BrowserStatus::Failed => "Failed",
    }
}

fn monitor_summary_details(
    summary: &agent::types::MonitorSummaryRecord,
) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("cwd {}", summary.cwd),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("command {}", preview_text(&summary.command, 96)),
            continuation: false,
        },
    ];
    if let Some(task_id) = summary.task_id.as_ref() {
        details.push(TranscriptShellDetail::Raw {
            text: format!("task {}", task_id),
            continuation: false,
        });
    }
    details
}
