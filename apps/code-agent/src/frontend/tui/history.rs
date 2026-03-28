use super::state::preview_text;
use crate::backend::{LoadedRun, RunExportArtifact, RunExportKind, message_to_text, preview_id};
use agent::types::{RunEventEnvelope, RunEventKind, SessionId};
use store::{RunSearchResult, RunSummary, TokenUsageRecord};

pub(crate) fn format_session_summary_line(summary: &RunSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!(
        "{}  msgs={} ev={} workers={}  {}",
        preview_id(summary.run_id.as_str()),
        summary.transcript_message_count,
        summary.event_count,
        summary.session_count,
        prompt
    )
}

pub(crate) fn format_session_search_line(result: &RunSearchResult) -> String {
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

pub(crate) fn format_session_inspector(run: &LoadedRun) -> Vec<String> {
    let mut lines = vec![
        "## Session".to_string(),
        format!("session ref: {}", run.summary.run_id),
        format!("event count: {}", run.summary.event_count),
        format!("message count: {}", run.summary.transcript_message_count),
        format!("worker sessions: {}", run.summary.session_count),
    ];
    if let Some(run_usage) = &run.token_usage.run {
        lines.push("## Token Budget".to_string());
        if let Some(window) = run_usage.ledger.context_window {
            lines.push(format!(
                "context: {} / {}",
                window.used_tokens, window.max_tokens
            ));
        }
        lines.push(format!(
            "session tokens: in={} out={} cache={}",
            run_usage.ledger.cumulative_usage.input_tokens,
            run_usage.ledger.cumulative_usage.output_tokens,
            run_usage.ledger.cumulative_usage.cache_read_tokens,
        ));
    }
    if !run.token_usage.aggregate_usage.is_zero() {
        lines.push(format!(
            "total tokens: in={} out={} prefill={} decode={} cache={}",
            run.token_usage.aggregate_usage.input_tokens,
            run.token_usage.aggregate_usage.output_tokens,
            run.token_usage.aggregate_usage.prefill_tokens,
            run.token_usage.aggregate_usage.decode_tokens,
            run.token_usage.aggregate_usage.cache_read_tokens,
        ));
    }
    if !run.token_usage.subagents.is_empty() {
        lines.push("## Subagents".to_string());
        lines.push(format!(
            "subagent count: {}",
            run.token_usage.subagents.len()
        ));
        lines.extend(
            run.token_usage
                .subagents
                .iter()
                .take(4)
                .map(format_token_usage_record_line),
        );
    }
    if let Some(prompt) = &run.summary.last_user_prompt {
        lines.push("## Prompt".to_string());
        lines.push(format!("last prompt: {}", preview_text(prompt, 80)));
    }
    if !run.session_ids.is_empty() {
        lines.push("## Runtime IDs".to_string());
        lines.push(format!(
            "runtime sessions: {}",
            run.session_ids
                .iter()
                .map(|session_id: &SessionId| preview_id(session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !run.events.is_empty() {
        lines.push("## Recent Events".to_string());
        lines.extend(
            run.events
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(format_run_event_line),
        );
    }
    lines
}

pub(crate) fn format_session_transcript_lines(run: &LoadedRun) -> Vec<String> {
    let transcript = run
        .transcript
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>();
    if transcript.is_empty() {
        vec!["No transcript messages recorded for this session.".to_string()]
    } else {
        transcript
    }
}

pub(crate) fn format_session_export_result(result: &RunExportArtifact) -> Vec<String> {
    vec![
        "## Export".to_string(),
        format!(
            "export: {}",
            match result.kind {
                RunExportKind::EventsJsonl => "events jsonl",
                RunExportKind::TranscriptText => "transcript text",
            }
        ),
        format!("session ref: {}", result.run_id),
        format!("path: {}", result.output_path.display()),
        format!("items: {}", result.item_count),
    ]
}

fn format_token_usage_record_line(record: &TokenUsageRecord) -> String {
    let name = record
        .agent_name
        .as_deref()
        .or(record.task_id.as_deref())
        .map(|value| preview_text(value, 20))
        .unwrap_or_else(|| preview_id(record.run_id.as_str()));
    format!(
        "{} in={} out={} cache={}",
        name,
        record.ledger.cumulative_usage.input_tokens,
        record.ledger.cumulative_usage.output_tokens,
        record.ledger.cumulative_usage.cache_read_tokens,
    )
}

fn format_run_event_line(event: &RunEventEnvelope) -> String {
    match &event.event {
        RunEventKind::SessionStart { reason } => {
            format!("session_start {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
        RunEventKind::InstructionsLoaded { count } => format!("instructions_loaded count={count}"),
        RunEventKind::SteerApplied { message, reason } => format!(
            "steer {} {}",
            reason.as_deref().unwrap_or(""),
            preview_text(message, 24)
        )
        .trim()
        .to_string(),
        RunEventKind::UserPromptSubmit { prompt } => {
            format!("user_prompt {}", preview_text(prompt, 42))
        }
        RunEventKind::ModelRequestStarted { request } => format!(
            "model_request messages={} tools={}",
            request.messages.len(),
            request.tools.len()
        ),
        RunEventKind::CompactionCompleted {
            reason,
            source_message_count,
            retained_message_count,
            summary_chars,
        } => format!(
            "compaction {} messages={} kept={} summary_chars={}",
            reason, source_message_count, retained_message_count, summary_chars
        ),
        RunEventKind::ModelResponseCompleted {
            assistant_text,
            tool_calls,
            ..
        } => format!(
            "model_response text={} tool_calls={}",
            preview_text(assistant_text, 24),
            tool_calls.len()
        ),
        RunEventKind::TokenUsageUpdated { phase, ledger } => format!(
            "token_usage {:?} context={} input={} output={}",
            phase,
            ledger
                .context_window
                .map(|usage| format!("{}/{}", usage.used_tokens, usage.max_tokens))
                .unwrap_or_else(|| "unknown".to_string()),
            ledger.cumulative_usage.input_tokens,
            ledger.cumulative_usage.output_tokens,
        ),
        RunEventKind::HookInvoked { hook_name, event } => {
            format!("hook_invoked {hook_name} {:?}", event)
        }
        RunEventKind::HookCompleted {
            hook_name, output, ..
        } => format!(
            "hook_completed {hook_name} effects={}",
            output.effects.len()
        ),
        RunEventKind::TranscriptMessage { message } => {
            format!("transcript {}", preview_text(&message_to_text(message), 42))
        }
        RunEventKind::TranscriptMessagePatched {
            message_id,
            message,
        } => format!(
            "transcript_patch {} {}",
            preview_id(message_id.as_str()),
            preview_text(&message_to_text(message), 32)
        ),
        RunEventKind::TranscriptMessageRemoved { message_id } => {
            format!("transcript_remove {}", preview_id(message_id.as_str()))
        }
        RunEventKind::ToolApprovalRequested { call, .. } => {
            format!("approval_requested {}", call.tool_name)
        }
        RunEventKind::ToolApprovalResolved { call, approved, .. } => {
            format!("approval_resolved {} approved={approved}", call.tool_name)
        }
        RunEventKind::ToolCallStarted { call } => format!("tool_started {}", call.tool_name),
        RunEventKind::ToolCallCompleted { call, output } => format!(
            "tool_completed {} {}",
            call.tool_name,
            preview_text(&output.text_content(), 24)
        ),
        RunEventKind::ToolCallFailed { call, error } => {
            format!("tool_failed {} {}", call.tool_name, preview_text(error, 24))
        }
        RunEventKind::Notification { source, message } => {
            format!("notification {source} {}", preview_text(message, 24))
        }
        RunEventKind::TaskCreated { task, .. } => format!(
            "task_created {} role={} claims={}",
            task.task_id,
            task.role,
            task.requested_write_set.len()
        ),
        RunEventKind::TaskCompleted {
            task_id, status, ..
        } => format!("task_completed {task_id} status={status}"),
        RunEventKind::SubagentStart { handle, .. } => format!(
            "subagent_start {} {}",
            preview_id(handle.agent_id.as_str()),
            handle.role
        ),
        RunEventKind::AgentEnvelope { envelope } => format!(
            "agent_envelope {}",
            preview_text(&format!("{:?}", envelope.kind), 40)
        ),
        RunEventKind::SubagentStop { handle, error, .. } => format!(
            "subagent_stop {} {}",
            preview_id(handle.agent_id.as_str()),
            error
                .as_deref()
                .map(|value| preview_text(value, 24))
                .unwrap_or_else(|| "ok".to_string())
        ),
        RunEventKind::Stop { reason } => format!("stop {}", reason.as_deref().unwrap_or(""))
            .trim()
            .to_string(),
        RunEventKind::StopFailure { reason } => {
            format!("stop_failure {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
        RunEventKind::SessionEnd { reason } => {
            format!("session_end {}", reason.as_deref().unwrap_or(""))
                .trim()
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_session_export_result;
    use crate::backend::{RunExportArtifact, RunExportKind};
    use agent::types::RunId;
    use std::path::PathBuf;

    #[test]
    fn export_result_includes_kind_path_and_item_count() {
        let lines = format_session_export_result(&RunExportArtifact {
            kind: RunExportKind::TranscriptText,
            run_id: RunId::from("run-1"),
            output_path: PathBuf::from("/workspace/out.txt"),
            item_count: 4,
        });

        assert!(lines.iter().any(|line| line == "export: transcript text"));
        assert!(lines.iter().any(|line| line == "path: /workspace/out.txt"));
        assert!(lines.iter().any(|line| line == "items: 4"));
    }
}
