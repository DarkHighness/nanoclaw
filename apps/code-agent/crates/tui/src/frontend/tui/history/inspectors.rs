use super::*;

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
    if let Some(session_title) = &session.summary.session_title {
        lines.push(InspectorEntry::field(
            "session title",
            preview_text(session_title, 80),
        ));
    }
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
        InspectorEntry::field("task id", task.summary.task_id.to_string()),
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

fn format_task_message_line(message: &LoadedTaskMessage) -> TranscriptEntry {
    TranscriptEntry::UserPrompt(preview_text(&message_to_text(&message.message), 72))
}
