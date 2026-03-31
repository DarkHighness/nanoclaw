use super::state::{InspectorEntry, TranscriptEntry, preview_text};
use crate::backend::{
    LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction, LiveTaskMessageOutcome,
    LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome, LoadedAgentSession, LoadedSession,
    LoadedSubagentSession, LoadedTask, McpPromptSummary, McpResourceSummary, McpServerSummary,
    PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
    PersistedTaskSummary, SessionExportArtifact, SessionExportKind, SessionOperationAction,
    SessionOperationOutcome, StartupDiagnosticsSnapshot, message_to_text, preview_id,
};
use crate::tool_render::{
    prefixed_detail_lines, summarize_tool_entry, tool_arguments_preview_lines,
    tool_output_detail_lines,
};
use agent::types::{
    AgentEnvelopeKind, AgentSessionId, AgentStatus, HookEvent, Message, SessionEventEnvelope,
    SessionEventKind,
};
use store::TokenUsageRecord;

pub(crate) fn format_session_summary_line(summary: &PersistedSessionSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    shell_summary(
        format!("• {}  {}", preview_id(&summary.session_ref), prompt),
        [format!(
            "{} messages · {} events · {} agent sessions · resume {}",
            summary.transcript_message_count,
            summary.event_count,
            summary.worker_session_count,
            summary.resume_support.label()
        )],
    )
}

pub(crate) fn format_agent_session_summary_line(summary: &PersistedAgentSessionSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    shell_summary(
        format!(
            "• {}  {}",
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

pub(crate) fn format_task_summary_line(summary: &PersistedTaskSummary) -> String {
    shell_summary(
        format!("• {}  {}", summary.task_id, summary.status),
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

pub(crate) fn format_live_task_summary_line(summary: &LiveTaskSummary) -> String {
    shell_summary(
        format!("• {}  {}", summary.task_id, summary.status),
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
    vec![InspectorEntry::transcript(
        TranscriptEntry::shell_summary_entry(
            format!("Spawned task {}", outcome.task.task_id),
            &[
                format!("role {}", outcome.task.role),
                format!("status {}", outcome.task.status),
                format!("agent {}", outcome.task.agent_id),
                format!("session {}", outcome.task.session_ref),
                format!("agent session {}", outcome.task.agent_session_ref),
                format!("prompt {}", preview_text(&outcome.prompt, 96)),
            ],
        ),
    )]
}

pub(crate) fn format_session_search_line(result: &PersistedSessionSearchMatch) -> String {
    let prompt = result
        .summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    shell_summary(
        format!("• {}  {}", preview_id(&result.summary.session_ref), prompt),
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
        TranscriptEntry::error_summary_entry(body.to_string(), &[])
    } else {
        TranscriptEntry::AssistantMessage(raw.to_string())
    }
}

fn format_transcript_entry(raw: &str) -> String {
    project_transcript_entry(raw).serialized()
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

pub(crate) fn format_session_operation_outcome(
    outcome: &SessionOperationOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        SessionOperationAction::StartedFresh => "✔ Started new session",
        SessionOperationAction::AlreadyAttached => "• Agent session already attached",
        SessionOperationAction::Reattached => "✔ Reattached session",
    };
    let mut details = vec![
        format!("session {}", outcome.session_ref),
        format!("agent session {}", outcome.active_agent_session_ref),
    ];
    if let Some(requested_agent_session_ref) = &outcome.requested_agent_session_ref {
        details.push(format!("requested {}", requested_agent_session_ref));
    }
    vec![InspectorEntry::transcript(TranscriptEntry::from(
        shell_summary(headline, details),
    ))]
}

pub(crate) fn format_live_task_control_outcome(
    outcome: &LiveTaskControlOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        LiveTaskControlAction::Cancelled => format!("✔ Cancelled task {}", outcome.task_id),
        LiveTaskControlAction::AlreadyTerminal => {
            format!("• Task {} was already terminal", outcome.task_id)
        }
    };
    vec![InspectorEntry::transcript(TranscriptEntry::from(
        shell_summary(
            headline,
            [
                format!("requested {}", outcome.requested_ref),
                format!("agent {}", outcome.agent_id),
                format!("status {}", outcome.status),
            ],
        ),
    ))]
}

pub(crate) fn format_live_task_message_outcome(
    outcome: &LiveTaskMessageOutcome,
) -> Vec<InspectorEntry> {
    let headline = match outcome.action {
        LiveTaskMessageAction::Sent => format!("• Sent steer message to task {}", outcome.task_id),
        LiveTaskMessageAction::AlreadyTerminal => {
            format!("• Task {} was already terminal", outcome.task_id)
        }
    };
    vec![InspectorEntry::transcript(TranscriptEntry::from(
        shell_summary(
            headline,
            [
                format!("requested {}", outcome.requested_ref),
                format!("agent {}", outcome.agent_id),
                format!("status {}", outcome.status),
                format!("message {}", preview_text(&outcome.message, 96)),
            ],
        ),
    ))]
}

pub(crate) fn format_live_task_wait_outcome(outcome: &LiveTaskWaitOutcome) -> Vec<InspectorEntry> {
    let headline = match outcome.status {
        AgentStatus::Completed => format!("• Finished waiting for task {}", outcome.task_id),
        AgentStatus::Failed => format!("✗ Finished waiting for task {}", outcome.task_id),
        AgentStatus::Cancelled => format!("✗ Waiting cancelled for task {}", outcome.task_id),
        _ => format!("• Waiting finished for task {}", outcome.task_id),
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
    vec![InspectorEntry::transcript(TranscriptEntry::from(
        shell_summary(headline, details),
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

fn format_token_usage_record_line(record: &TokenUsageRecord) -> String {
    let name = record
        .agent_name
        .as_deref()
        .or(record.task_id.as_deref())
        .map(|value| preview_text(value, 20))
        .unwrap_or_else(|| preview_id(record.session_id.as_str()));
    format!(
        "{} in={} out={} cache={}",
        name,
        record.ledger.cumulative_usage.input_tokens,
        record.ledger.cumulative_usage.output_tokens,
        record.ledger.cumulative_usage.cache_read_tokens,
    )
}

fn format_loaded_subagent_line(subagent: &LoadedSubagentSession) -> String {
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
    format!(
        "{} role={} status={} {}{}",
        preview_id(subagent.handle.agent_session_id.as_str()),
        subagent.task.role,
        subagent.status,
        preview_text(&subagent.summary, 28),
        token_summary
    )
}

fn format_task_message_line(message: &crate::backend::LoadedTaskMessage) -> String {
    format!(
        "{} {}",
        message.channel,
        preview_text(&message.payload.to_string(), 72)
    )
}

fn shell_summary(headline: impl Into<String>, details: impl IntoIterator<Item = String>) -> String {
    let mut lines = vec![headline.into()];
    for detail in details.into_iter().filter(|detail| !detail.is_empty()) {
        lines.push(format!("  └ {detail}"));
    }
    lines.join("\n")
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

fn task_status_headline(task_id: &str, status: &AgentStatus) -> String {
    match status {
        AgentStatus::Completed => format!("✔ Task {task_id} completed"),
        AgentStatus::Failed => format!("✗ Task {task_id} failed"),
        AgentStatus::Cancelled => format!("✗ Task {task_id} cancelled"),
        AgentStatus::WaitingApproval => format!("• Task {task_id} is awaiting approval"),
        AgentStatus::WaitingMessage => format!("• Task {task_id} is waiting for a message"),
        AgentStatus::Queued => format!("• Task {task_id} is queued"),
        AgentStatus::Running => format!("• Task {task_id} is running"),
    }
}

fn format_agent_envelope_kind(kind: &AgentEnvelopeKind) -> String {
    match kind {
        AgentEnvelopeKind::SpawnRequested { task } => shell_summary(
            format!("• Requested {} task {}", task.role, task.task_id),
            [format!("prompt {}", preview_text(&task.prompt, 72))],
        ),
        AgentEnvelopeKind::Started { task } => shell_summary(
            format!("• Started {} task {}", task.role, task.task_id),
            [format!("prompt {}", preview_text(&task.prompt, 72))],
        ),
        AgentEnvelopeKind::StatusChanged { status } => match status {
            AgentStatus::Completed => "✔ Agent completed".to_string(),
            AgentStatus::Failed => "✗ Agent failed".to_string(),
            AgentStatus::Cancelled => "✗ Agent cancelled".to_string(),
            AgentStatus::WaitingApproval => "• Agent is awaiting approval".to_string(),
            AgentStatus::WaitingMessage => "• Agent is waiting for a message".to_string(),
            AgentStatus::Queued => "• Agent is queued".to_string(),
            AgentStatus::Running => "• Agent is running".to_string(),
        },
        AgentEnvelopeKind::Message { channel, payload } => shell_summary(
            format!("• Agent message on {channel}"),
            [format!(
                "payload {}",
                preview_text(&payload.to_string(), 72)
            )],
        ),
        AgentEnvelopeKind::Artifact { artifact } => shell_summary(
            format!("• Emitted {} artifact", artifact.kind),
            [format!("uri {}", preview_text(&artifact.uri, 72))],
        ),
        AgentEnvelopeKind::ClaimRequested { files } => shell_summary(
            "• Requested file claim",
            [format!("files {}", preview_text(&files.join(", "), 72))],
        ),
        AgentEnvelopeKind::ClaimGranted { files } => shell_summary(
            "✔ Claimed files",
            [format!("files {}", preview_text(&files.join(", "), 72))],
        ),
        AgentEnvelopeKind::ClaimRejected { files, owner } => shell_summary(
            "✗ File claim rejected",
            [
                format!("files {}", preview_text(&files.join(", "), 72)),
                format!("owner {}", preview_id(owner.as_str())),
            ],
        ),
        AgentEnvelopeKind::Result { result } => {
            let headline = task_status_headline(&result.task_id, &result.status);
            shell_summary(
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
        AgentEnvelopeKind::Failed { error } => shell_summary(
            "✗ Agent failed",
            [format!("error {}", preview_text(error, 72))],
        ),
        AgentEnvelopeKind::Cancelled { reason } => shell_summary(
            "✗ Agent cancelled",
            [format_reason_detail(reason.as_deref())
                .unwrap_or_else(|| "no reason recorded".to_string())],
        ),
        AgentEnvelopeKind::Heartbeat => "• Agent heartbeat".to_string(),
    }
}

fn format_session_event_line(event: &SessionEventEnvelope) -> String {
    match &event.event {
        SessionEventKind::SessionStart { reason } => shell_summary(
            "• Started session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::InstructionsLoaded { count } => shell_summary(
            "• Loaded instructions",
            [format!("{count} instruction block(s)")],
        ),
        SessionEventKind::SteerApplied { message, reason } => shell_summary(
            "• Applied steer",
            [
                format!("message {}", preview_text(message, 72)),
                format_reason_detail(reason.as_deref()).unwrap_or_default(),
            ],
        ),
        SessionEventKind::UserPromptSubmit { prompt } => {
            format!("› {}", preview_text(prompt, 96))
        }
        SessionEventKind::ModelRequestStarted { request } => shell_summary(
            "• Requested model response",
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
        } => shell_summary(
            "• Compacted session context",
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
        } => shell_summary(
            "• Finished model response",
            [
                (!assistant_text.trim().is_empty())
                    .then(|| format!("text {}", preview_text(assistant_text, 72)))
                    .unwrap_or_default(),
                (!tool_calls.is_empty())
                    .then(|| format!("tool calls {}", tool_calls.len()))
                    .unwrap_or_default(),
            ],
        ),
        SessionEventKind::TokenUsageUpdated { phase, ledger } => shell_summary(
            "• Updated token usage",
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
        SessionEventKind::HookInvoked { hook_name, event } => shell_summary(
            format!("• Running hook {hook_name}"),
            [format!("event {}", format_hook_event_label(*event))],
        ),
        SessionEventKind::HookCompleted {
            hook_name, output, ..
        } => shell_summary(
            format!("• Finished hook {hook_name}"),
            [format!("effects {}", output.effects.len())],
        ),
        SessionEventKind::TranscriptMessage { message } => {
            format_transcript_entry(&message_to_text(message))
        }
        SessionEventKind::TranscriptMessagePatched {
            message_id,
            message,
        } => shell_summary(
            "• Updated transcript message",
            [
                format!("message {}", preview_id(message_id.as_str())),
                format!("content {}", preview_text(&message_to_text(message), 72)),
            ],
        ),
        SessionEventKind::TranscriptMessageRemoved { message_id } => shell_summary(
            "• Removed transcript message",
            [format!("message {}", preview_id(message_id.as_str()))],
        ),
        SessionEventKind::ToolApprovalRequested { call, reasons } => {
            let mut detail_lines = vec![format!("  └ origin {}", format_tool_origin(&call.origin))];
            detail_lines.extend(prefixed_detail_lines(&tool_arguments_preview_lines(
                call.tool_name.as_str(),
                &call.arguments,
            )));
            if let Some(reason) = reasons.first() {
                detail_lines.push(format!("  └ reason {}", preview_text(reason, 72)));
            }
            summarize_tool_entry(
                format!("• Awaiting approval for {}", call.tool_name),
                detail_lines,
            )
        }
        SessionEventKind::ToolApprovalResolved {
            call,
            approved,
            reason,
        } => shell_summary(
            if *approved {
                format!("✔ Approved {}", call.tool_name)
            } else {
                format!("✗ Denied {}", call.tool_name)
            },
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::ToolCallStarted { call } => summarize_tool_entry(
            format!("• Running {}", call.tool_name),
            prefixed_detail_lines(&tool_arguments_preview_lines(
                call.tool_name.as_str(),
                &call.arguments,
            )),
        ),
        SessionEventKind::ToolCallCompleted { call, output } => {
            let mut detail_lines = prefixed_detail_lines(&tool_arguments_preview_lines(
                call.tool_name.as_str(),
                &call.arguments,
            ));
            detail_lines.extend(tool_output_detail_lines(
                call.tool_name.as_str(),
                &output.text_content(),
                output.structured_content.as_ref(),
            ));
            summarize_tool_entry(format!("• Finished {}", call.tool_name), detail_lines)
        }
        SessionEventKind::ToolCallFailed { call, error } => {
            let mut detail_lines = prefixed_detail_lines(&tool_arguments_preview_lines(
                call.tool_name.as_str(),
                &call.arguments,
            ));
            detail_lines.push(format!("  └ error {}", preview_text(error, 72)));
            summarize_tool_entry(format!("✗ {} failed", call.tool_name), detail_lines)
        }
        SessionEventKind::Notification { source, message } => shell_summary(
            format!("• Notification from {source}"),
            [format!("message {}", preview_text(message, 72))],
        ),
        SessionEventKind::TaskCreated { task, .. } => shell_summary(
            format!("• Spawned task {}", task.task_id),
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
        } => shell_summary(
            task_status_headline(task_id, status),
            [format!("agent {}", preview_id(agent_id.as_str()))],
        ),
        SessionEventKind::SubagentStart { handle, .. } => shell_summary(
            format!(
                "• Started {} agent {}",
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
            let headline = if error.is_some() {
                format!("✗ Stopped agent {}", preview_id(handle.agent_id.as_str()))
            } else {
                format!("✔ Stopped agent {}", preview_id(handle.agent_id.as_str()))
            };
            shell_summary(
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
        SessionEventKind::Stop { reason } => shell_summary(
            "• Stopped session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::StopFailure { reason } => shell_summary(
            "✗ Failed to stop session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
        SessionEventKind::SessionEnd { reason } => shell_summary(
            "• Ended session",
            [format_reason_detail(reason.as_deref()).unwrap_or_default()],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_agent_session_summary_line, format_live_task_wait_outcome,
        format_session_event_line, format_session_export_result, format_session_operation_outcome,
        format_session_search_line, format_session_summary_line,
    };
    use crate::backend::{
        LiveTaskWaitOutcome, PersistedAgentSessionSummary, PersistedSessionSearchMatch,
        PersistedSessionSummary, ResumeSupport, SessionExportArtifact, SessionExportKind,
        SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot,
    };
    use crate::frontend::tui::state::InspectorEntry;
    use agent::types::{
        AgentSessionId, AgentStatus, Message, SessionEventEnvelope, SessionEventKind, SessionId,
        ToolCall, ToolCallId, ToolOrigin, ToolResult,
    };
    use serde_json::json;
    use std::path::PathBuf;

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
            line,
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
            line,
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
            line,
            "• session_  Refine the approval preview\n  └ 12 messages · 40 events · 2 agent sessions · resume attached · matched 3 event(s) · preview bash approval | cargo test"
        );
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
            format_session_event_line(&event),
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
            format_session_event_line(&event),
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
            format_session_event_line(&event),
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

        let rendered = format_session_event_line(&event);
        assert!(rendered.contains("• Finished write"));
        assert!(rendered.contains("  └ diff src/lib.rs"));
        assert!(rendered.contains("@@ -1,1 +1,1 @@"));
        assert!(rendered.contains("+new()"));
    }

    fn inspector_line_texts(lines: &[InspectorEntry]) -> Vec<String> {
        lines
            .iter()
            .flat_map(|line| match line {
                InspectorEntry::Raw(text)
                | InspectorEntry::Section(text)
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
