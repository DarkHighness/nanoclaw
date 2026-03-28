use agent::mcp::{McpPrompt, McpResource};
use agent::skills::Skill;
use serde_json::Value;
use store::{RunSearchResult, RunSummary, RunTokenUsageReport, TokenUsageRecord};
use types::{
    Message, MessagePart, MessageRole, RunEventEnvelope, RunEventKind, SessionId, ToolOrigin,
    ToolSpec,
};

pub(super) fn format_tool_line(spec: &ToolSpec) -> String {
    let origin = match &spec.origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    };
    let title = spec
        .annotations
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(spec.name.as_str());
    format!(
        "{} [{}] ro={} destructive={} open_world={}",
        title,
        origin,
        tool_annotation_bool(spec, "readOnlyHint").unwrap_or(false),
        tool_annotation_bool(spec, "destructiveHint").unwrap_or(true),
        tool_annotation_bool(spec, "openWorldHint").unwrap_or(true),
    )
}

pub(super) fn format_run_summary_line(summary: &RunSummary) -> String {
    let prompt = summary
        .last_user_prompt
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!(
        "{}  msg={} ev={} sess={}  {}",
        preview_id(summary.run_id.as_str()),
        summary.transcript_message_count,
        summary.event_count,
        summary.session_count,
        prompt
    )
}

pub(super) fn format_run_search_line(result: &RunSearchResult) -> String {
    let base = format_run_summary_line(&result.summary);
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

pub(super) fn format_run_sidebar(
    summary: &RunSummary,
    session_ids: &[SessionId],
    events: &[RunEventEnvelope],
    token_usage: &RunTokenUsageReport,
) -> Vec<String> {
    let mut sidebar = vec![
        format!("run: {}", summary.run_id),
        format!("events: {}", summary.event_count),
        format!("messages: {}", summary.transcript_message_count),
        format!("sessions: {}", summary.session_count),
    ];
    if let Some(run_usage) = &token_usage.run {
        if let Some(window) = run_usage.ledger.context_window {
            sidebar.push(format!(
                "context: {} / {}",
                window.used_tokens, window.max_tokens
            ));
        }
        sidebar.push(format!(
            "run tokens: in={} out={} cache={}",
            run_usage.ledger.cumulative_usage.input_tokens,
            run_usage.ledger.cumulative_usage.output_tokens,
            run_usage.ledger.cumulative_usage.cache_read_tokens,
        ));
    }
    if !token_usage.aggregate_usage.is_zero() {
        sidebar.push(format!(
            "total tokens: in={} out={} prefill={} decode={} cache={}",
            token_usage.aggregate_usage.input_tokens,
            token_usage.aggregate_usage.output_tokens,
            token_usage.aggregate_usage.prefill_tokens,
            token_usage.aggregate_usage.decode_tokens,
            token_usage.aggregate_usage.cache_read_tokens,
        ));
    }
    if !token_usage.subagents.is_empty() {
        sidebar.push(format!("subagents: {}", token_usage.subagents.len()));
        sidebar.extend(
            token_usage
                .subagents
                .iter()
                .take(4)
                .map(format_token_usage_record_line),
        );
    }
    if let Some(prompt) = &summary.last_user_prompt {
        sidebar.push(format!("last prompt: {}", preview_text(prompt, 80)));
    }
    if !session_ids.is_empty() {
        sidebar.push(format!(
            "session ids: {}",
            session_ids
                .iter()
                .map(|session_id| preview_id(session_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !events.is_empty() {
        sidebar.push("recent events:".to_string());
        sidebar.extend(
            events
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(format_run_event_line),
        );
    }
    sidebar
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

pub(super) fn format_run_event_line(event: &RunEventEnvelope) -> String {
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
        RunEventKind::AgentEnvelope { envelope } => match &envelope.kind {
            types::AgentEnvelopeKind::SpawnRequested { task } => {
                format!("agent_spawn {}", task.task_id)
            }
            types::AgentEnvelopeKind::Started { task } => {
                format!("agent_started {}", task.task_id)
            }
            types::AgentEnvelopeKind::StatusChanged { status } => {
                format!("agent_status {}", status)
            }
            types::AgentEnvelopeKind::Message { channel, payload } => format!(
                "agent_message {} {}",
                channel,
                preview_text(&payload.to_string(), 24)
            ),
            types::AgentEnvelopeKind::Artifact { artifact } => {
                format!("agent_artifact {}", preview_text(&artifact.uri, 24))
            }
            types::AgentEnvelopeKind::ClaimRequested { files } => {
                format!("claim_requested {}", preview_text(&files.join(", "), 24))
            }
            types::AgentEnvelopeKind::ClaimGranted { files } => {
                format!("claim_granted {}", preview_text(&files.join(", "), 24))
            }
            types::AgentEnvelopeKind::ClaimRejected { owner, .. } => {
                format!("claim_rejected owner={}", preview_id(owner.as_str()))
            }
            types::AgentEnvelopeKind::Result { result } => format!(
                "agent_result {} {}",
                preview_id(result.agent_id.as_str()),
                preview_text(&result.summary, 24)
            ),
            types::AgentEnvelopeKind::Failed { error } => {
                format!("agent_failed {}", preview_text(error, 24))
            }
            types::AgentEnvelopeKind::Cancelled { reason } => format!(
                "agent_cancelled {}",
                preview_text(reason.as_deref().unwrap_or(""), 24)
            )
            .trim()
            .to_string(),
            types::AgentEnvelopeKind::Heartbeat => "agent_heartbeat".to_string(),
        },
        RunEventKind::SubagentStop { handle, result, .. } => format!(
            "subagent_stop {} status={} {}",
            preview_id(handle.agent_id.as_str()),
            handle.status,
            result
                .as_ref()
                .map(|result| preview_text(&result.summary, 24))
                .unwrap_or_default()
        )
        .trim()
        .to_string(),
        RunEventKind::Notification { source, message } => {
            format!("notification {source} {}", preview_text(message, 24))
        }
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

pub(super) fn build_turn_sidebar(events: &[RunEventEnvelope]) -> Vec<String> {
    let mut sidebar = Vec::new();
    if !events.is_empty() {
        sidebar.push("recent events:".to_string());
        sidebar.extend(
            events
                .iter()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(format_run_event_line),
        );
    }
    sidebar
}

pub(super) fn format_skill_line(skill: &Skill) -> String {
    let aliases = if skill.aliases.is_empty() {
        String::new()
    } else {
        format!(" aliases={}", skill.aliases.join(","))
    };
    let tags = if skill.tags.is_empty() {
        String::new()
    } else {
        format!(" tags={}", skill.tags.join(","))
    };
    format!(
        "{}{}{} hooks={} refs={} scripts={} assets={}  {}",
        skill.name,
        aliases,
        tags,
        skill.hooks.len(),
        skill.references.len(),
        skill.scripts.len(),
        skill.assets.len(),
        preview_text(&skill.description, 42)
    )
}

pub(super) fn format_skill_sidebar(skill: &Skill) -> Vec<String> {
    let mut sidebar = vec![
        format!("name: {}", skill.name),
        format!("root: {}", skill.root_dir.display()),
        format!("description: {}", skill.description),
    ];
    if !skill.aliases.is_empty() {
        sidebar.push(format!("aliases: {}", skill.aliases.join(", ")));
    }
    if !skill.tags.is_empty() {
        sidebar.push(format!("tags: {}", skill.tags.join(", ")));
    }
    sidebar.push(format!("hooks: {}", skill.hooks.len()));
    sidebar.push(format!("references: {}", skill.references.len()));
    sidebar.push(format!("scripts: {}", skill.scripts.len()));
    sidebar.push(format!("assets: {}", skill.assets.len()));
    sidebar.push(format!(
        "instruction: {}",
        preview_text(&skill.system_instruction(), 120)
    ));
    sidebar
}

pub(super) fn preview_text(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        format!(
            "{}...",
            collapsed
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>()
        )
    }
}

pub(super) fn preview_id(value: &str) -> String {
    value.chars().take(8).collect()
}

pub(super) fn prompt_to_text(prompt: &McpPrompt) -> String {
    if prompt.messages.is_empty() {
        return prompt.description.clone();
    }
    prompt
        .messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn resource_to_text(resource: &McpResource) -> String {
    let parts = resource
        .parts
        .iter()
        .map(message_part_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    if parts.is_empty() {
        resource.description.clone()
    } else {
        parts
    }
}

pub(super) fn message_to_text(message: &Message) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!(
        "{role}> {}",
        message
            .parts
            .iter()
            .map(message_part_to_text)
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub(super) fn message_part_to_text(part: &MessagePart) -> String {
    match part {
        MessagePart::Text { text } => text.clone(),
        MessagePart::Image { mime_type, .. } => format!("[image:{mime_type}]"),
        MessagePart::File {
            file_name,
            mime_type,
            uri,
            ..
        } => format!(
            "[file:{}{}{}]",
            file_name.clone().unwrap_or_else(|| "unnamed".to_string()),
            mime_type
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
            uri.as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default()
        ),
        MessagePart::ToolCall { call } => format!("[tool_call:{}]", call.tool_name),
        MessagePart::ToolResult { result } => result.text_content(),
        MessagePart::Reasoning { reasoning } => {
            let text = reasoning.display_text();
            if text.is_empty() {
                "[reasoning]".to_string()
            } else {
                format!("[reasoning:{}]", preview_text(&text, 48))
            }
        }
        MessagePart::Resource { uri, text, .. } => text.clone().unwrap_or_else(|| uri.clone()),
        MessagePart::Json { value } => value.to_string(),
        MessagePart::ProviderExtension { provider, kind, .. } => {
            format!("[provider_extension:{provider}:{kind}]")
        }
    }
}

fn tool_annotation_bool(spec: &ToolSpec, key: &str) -> Option<bool> {
    spec.annotations
        .get(key)
        .and_then(Value::as_bool)
        .or_else(|| {
            spec.annotations
                .get("mcp_annotations")
                .and_then(Value::as_object)
                .and_then(|value| value.get(key))
                .and_then(Value::as_bool)
        })
}
