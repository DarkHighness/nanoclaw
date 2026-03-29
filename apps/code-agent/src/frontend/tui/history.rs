use super::state::preview_text;
use crate::backend::{
    AgentSessionResumeResult, LoadedSession, McpPromptSummary, McpResourceSummary,
    McpServerSummary, PersistedAgentSessionSummary, PersistedSessionSearchMatch,
    PersistedSessionSummary, ResumeAction, SessionExportArtifact, SessionExportKind,
    StartupDiagnosticsSnapshot, message_to_text, preview_id,
};
use agent::types::{AgentSessionId, Message, SessionEventEnvelope, SessionEventKind};
use store::TokenUsageRecord;

pub(crate) fn format_session_summary_line(summary: &PersistedSessionSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!(
        "{}  msgs={} ev={} workers={} resume={}  {}",
        preview_id(&summary.session_ref),
        summary.transcript_message_count,
        summary.event_count,
        summary.worker_session_count,
        summary.resume_support.label(),
        prompt
    )
}

pub(crate) fn format_agent_session_summary_line(summary: &PersistedAgentSessionSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 28))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!(
        "{}  session={} label={} ev={} msgs={} resume={}  {}",
        preview_id(&summary.agent_session_ref),
        preview_id(&summary.session_ref),
        summary.label,
        summary.event_count,
        summary.transcript_message_count,
        summary.resume_support.label(),
        prompt
    )
}

pub(crate) fn format_session_search_line(result: &PersistedSessionSearchMatch) -> String {
    let base = format_session_summary_line(&result.summary);
    if result.preview_matches.is_empty() {
        format!("{base}  matches={}", result.matched_event_count)
    } else {
        format!(
            "{base}  matches={}  {}",
            result.matched_event_count,
            preview_text(&result.preview_matches.join(" | "), 80)
        )
    }
}

pub(crate) fn format_session_inspector(session: &LoadedSession) -> Vec<String> {
    let mut lines = vec![
        "## Session".to_string(),
        format!("session ref: {}", session.summary.session_id),
        format!("event count: {}", session.summary.event_count),
        format!(
            "message count: {}",
            session.summary.transcript_message_count
        ),
        format!("worker sessions: {}", session.summary.agent_session_count),
    ];
    if let Some(session_usage) = &session.token_usage.session {
        lines.push("## Token Budget".to_string());
        if let Some(window) = session_usage.ledger.context_window {
            lines.push(format!(
                "context: {} / {}",
                window.used_tokens, window.max_tokens
            ));
        }
        lines.push(format!(
            "session tokens: in={} out={} cache={}",
            session_usage.ledger.cumulative_usage.input_tokens,
            session_usage.ledger.cumulative_usage.output_tokens,
            session_usage.ledger.cumulative_usage.cache_read_tokens,
        ));
    }
    if !session.token_usage.aggregate_usage.is_zero() {
        lines.push(format!(
            "total tokens: in={} out={} prefill={} decode={} cache={}",
            session.token_usage.aggregate_usage.input_tokens,
            session.token_usage.aggregate_usage.output_tokens,
            session.token_usage.aggregate_usage.prefill_tokens,
            session.token_usage.aggregate_usage.decode_tokens,
            session.token_usage.aggregate_usage.cache_read_tokens,
        ));
    }
    if !session.token_usage.subagents.is_empty() {
        lines.push("## Subagents".to_string());
        lines.push(format!(
            "subagent count: {}",
            session.token_usage.subagents.len()
        ));
        lines.extend(
            session
                .token_usage
                .subagents
                .iter()
                .take(4)
                .map(format_token_usage_record_line),
        );
    }
    if let Some(prompt) = &session.summary.last_user_prompt {
        lines.push("## Prompt".to_string());
        lines.push(format!("last prompt: {}", preview_text(prompt, 80)));
    }
    if !session.agent_session_ids.is_empty() {
        lines.push("## Runtime IDs".to_string());
        lines.push(format!(
            "runtime sessions: {}",
            session
                .agent_session_ids
                .iter()
                .map(|agent_session_id: &AgentSessionId| preview_id(agent_session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !session.events.is_empty() {
        lines.push("## Recent Events".to_string());
        lines.extend(
            session
                .events
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(format_session_event_line),
        );
    }
    lines
}

pub(crate) fn format_session_transcript_lines(session: &LoadedSession) -> Vec<String> {
    format_transcript_lines(&session.transcript)
}

pub(crate) fn format_visible_transcript_lines(transcript: &[Message]) -> Vec<String> {
    format_transcript_lines(transcript)
}

fn format_transcript_lines(transcript: &[Message]) -> Vec<String> {
    let transcript = transcript.iter().map(message_to_text).collect::<Vec<_>>();
    if transcript.is_empty() {
        vec!["No transcript messages recorded for this session.".to_string()]
    } else {
        transcript
    }
}

pub(crate) fn format_session_export_result(result: &SessionExportArtifact) -> Vec<String> {
    vec![
        "## Export".to_string(),
        format!(
            "export: {}",
            match result.kind {
                SessionExportKind::EventsJsonl => "events jsonl",
                SessionExportKind::TranscriptText => "transcript text",
            }
        ),
        format!("session ref: {}", result.session_id),
        format!("path: {}", result.output_path.display()),
        format!("items: {}", result.item_count),
    ]
}

pub(crate) fn format_agent_session_resume_result(result: &AgentSessionResumeResult) -> Vec<String> {
    vec![
        "## Resume".to_string(),
        format!(
            "requested agent session ref: {}",
            result.requested_agent_session_ref
        ),
        format!("session ref: {}", result.session_ref),
        format!(
            "active agent session ref: {}",
            result.active_agent_session_ref
        ),
        format!(
            "action: {}",
            match result.action {
                ResumeAction::AlreadyAttached => "already_attached",
                ResumeAction::Reattached => "reattached",
            }
        ),
    ]
}

pub(crate) fn format_startup_diagnostics(snapshot: &StartupDiagnosticsSnapshot) -> Vec<String> {
    let mut lines = vec![
        "## Runtime".to_string(),
        format!("local tools: {}", snapshot.local_tool_count),
        format!("mcp tools: {}", snapshot.mcp_tool_count),
        format!(
            "plugins: {} enabled / {} total",
            snapshot.enabled_plugin_count, snapshot.total_plugin_count
        ),
        format!("mcp servers: {}", snapshot.mcp_servers.len()),
    ];
    if !snapshot.plugin_details.is_empty() {
        lines.push("## Plugins".to_string());
        lines.extend(snapshot.plugin_details.iter().cloned());
    }
    if !snapshot.mcp_servers.is_empty() {
        lines.push("## MCP Servers".to_string());
        lines.extend(
            snapshot
                .mcp_servers
                .iter()
                .map(format_mcp_server_summary_line),
        );
    }
    if !snapshot.warnings.is_empty() {
        lines.push("## Warnings".to_string());
        lines.extend(
            snapshot
                .warnings
                .iter()
                .map(|warning| format!("warning: {warning}")),
        );
    }
    if !snapshot.diagnostics.is_empty() {
        lines.push("## Diagnostics".to_string());
        lines.extend(
            snapshot
                .diagnostics
                .iter()
                .map(|diagnostic| format!("diagnostic: {diagnostic}")),
        );
    }
    lines
}

pub(crate) fn format_mcp_server_summary_line(summary: &McpServerSummary) -> String {
    format!(
        "{}  tools={} prompts={} resources={}",
        summary.server_name, summary.tool_count, summary.prompt_count, summary.resource_count
    )
}

pub(crate) fn format_mcp_prompt_summary_line(summary: &McpPromptSummary) -> String {
    let suffix = if summary.argument_names.is_empty() {
        String::new()
    } else {
        format!(" ({})", summary.argument_names.join(", "))
    };
    if summary.description.is_empty() {
        format!("{}:{}{}", summary.server_name, summary.prompt_name, suffix)
    } else {
        format!(
            "{}:{}{} - {}",
            summary.server_name, summary.prompt_name, suffix, summary.description
        )
    }
}

pub(crate) fn format_mcp_resource_summary_line(summary: &McpResourceSummary) -> String {
    format!(
        "{}:{}{}{}",
        summary.server_name,
        summary.uri,
        summary
            .mime_type
            .as_deref()
            .map(|mime| format!(" [{mime}]"))
            .unwrap_or_default(),
        if summary.description.is_empty() {
            String::new()
        } else {
            format!(" - {}", summary.description)
        }
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

fn format_session_event_line(event: &SessionEventEnvelope) -> String {
    match &event.event {
        SessionEventKind::SessionStart { reason } => {
            format!("session_start {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
        SessionEventKind::InstructionsLoaded { count } => {
            format!("instructions_loaded count={count}")
        }
        SessionEventKind::SteerApplied { message, reason } => format!(
            "steer {} {}",
            reason.as_deref().unwrap_or(""),
            preview_text(message, 24)
        )
        .trim()
        .to_string(),
        SessionEventKind::UserPromptSubmit { prompt } => {
            format!("user_prompt {}", preview_text(prompt, 42))
        }
        SessionEventKind::ModelRequestStarted { request } => format!(
            "model_request messages={} tools={}",
            request.messages.len(),
            request.tools.len()
        ),
        SessionEventKind::CompactionCompleted {
            reason,
            source_message_count,
            retained_message_count,
            summary_chars,
            ..
        } => format!(
            "compaction {} messages={} kept={} summary_chars={}",
            reason, source_message_count, retained_message_count, summary_chars
        ),
        SessionEventKind::ModelResponseCompleted {
            assistant_text,
            tool_calls,
            ..
        } => format!(
            "model_response text={} tool_calls={}",
            preview_text(assistant_text, 24),
            tool_calls.len()
        ),
        SessionEventKind::TokenUsageUpdated { phase, ledger } => format!(
            "token_usage {:?} context={} input={} output={}",
            phase,
            ledger
                .context_window
                .map(|usage| format!("{}/{}", usage.used_tokens, usage.max_tokens))
                .unwrap_or_else(|| "unknown".to_string()),
            ledger.cumulative_usage.input_tokens,
            ledger.cumulative_usage.output_tokens,
        ),
        SessionEventKind::HookInvoked { hook_name, event } => {
            format!("hook_invoked {hook_name} {:?}", event)
        }
        SessionEventKind::HookCompleted {
            hook_name, output, ..
        } => format!(
            "hook_completed {hook_name} effects={}",
            output.effects.len()
        ),
        SessionEventKind::TranscriptMessage { message } => {
            format!("transcript {}", preview_text(&message_to_text(message), 42))
        }
        SessionEventKind::TranscriptMessagePatched {
            message_id,
            message,
        } => format!(
            "transcript_patch {} {}",
            preview_id(message_id.as_str()),
            preview_text(&message_to_text(message), 32)
        ),
        SessionEventKind::TranscriptMessageRemoved { message_id } => {
            format!("transcript_remove {}", preview_id(message_id.as_str()))
        }
        SessionEventKind::ToolApprovalRequested { call, .. } => {
            format!("approval_requested {}", call.tool_name)
        }
        SessionEventKind::ToolApprovalResolved { call, approved, .. } => {
            format!("approval_resolved {} approved={approved}", call.tool_name)
        }
        SessionEventKind::ToolCallStarted { call } => format!("tool_started {}", call.tool_name),
        SessionEventKind::ToolCallCompleted { call, output } => format!(
            "tool_completed {} {}",
            call.tool_name,
            preview_text(&output.text_content(), 24)
        ),
        SessionEventKind::ToolCallFailed { call, error } => {
            format!("tool_failed {} {}", call.tool_name, preview_text(error, 24))
        }
        SessionEventKind::Notification { source, message } => {
            format!("notification {source} {}", preview_text(message, 24))
        }
        SessionEventKind::TaskCreated { task, .. } => format!(
            "task_created {} role={} claims={}",
            task.task_id,
            task.role,
            task.requested_write_set.len()
        ),
        SessionEventKind::TaskCompleted {
            task_id, status, ..
        } => format!("task_completed {task_id} status={status}"),
        SessionEventKind::SubagentStart { handle, .. } => format!(
            "subagent_start {} {}",
            preview_id(handle.agent_id.as_str()),
            handle.role
        ),
        SessionEventKind::AgentEnvelope { envelope } => format!(
            "agent_envelope {}",
            preview_text(&format!("{:?}", envelope.kind), 40)
        ),
        SessionEventKind::SubagentStop { handle, error, .. } => format!(
            "subagent_stop {} {}",
            preview_id(handle.agent_id.as_str()),
            error
                .as_deref()
                .map(|value| preview_text(value, 24))
                .unwrap_or_else(|| "ok".to_string())
        ),
        SessionEventKind::Stop { reason } => format!("stop {}", reason.as_deref().unwrap_or(""))
            .trim()
            .to_string(),
        SessionEventKind::StopFailure { reason } => {
            format!("stop_failure {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
        SessionEventKind::SessionEnd { reason } => {
            format!("session_end {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_session_export_result;
    use crate::backend::{SessionExportArtifact, SessionExportKind};
    use agent::types::SessionId;
    use std::path::PathBuf;

    #[test]
    fn export_result_includes_kind_path_and_item_count() {
        let lines = format_session_export_result(&SessionExportArtifact {
            kind: SessionExportKind::TranscriptText,
            session_id: SessionId::from("session-1"),
            output_path: PathBuf::from("/workspace/out.txt"),
            item_count: 4,
        });

        assert!(lines.iter().any(|line| line == "export: transcript text"));
        assert!(lines.iter().any(|line| line == "path: /workspace/out.txt"));
        assert!(lines.iter().any(|line| line == "items: 4"));
    }
}
