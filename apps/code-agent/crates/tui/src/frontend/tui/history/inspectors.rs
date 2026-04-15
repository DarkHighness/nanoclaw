use super::*;
use crate::frontend::tui::state::InspectorAction;
pub(crate) fn format_session_inspector(session: &LoadedSession) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Session"),
        InspectorEntry::field("Session Ref", session.summary.session_id.to_string()),
        InspectorEntry::field("Event Count", session.summary.event_count.to_string()),
        InspectorEntry::field(
            "Message Count",
            session.summary.transcript_message_count.to_string(),
        ),
        InspectorEntry::field(
            "Worker Sessions",
            session.summary.agent_session_count.to_string(),
        ),
    ];
    if let Some(session_usage) = &session.token_usage.session {
        lines.push(InspectorEntry::section("Token Budget"));
        if let Some(window) = session_usage.ledger.context_window {
            lines.push(InspectorEntry::field(
                "Context",
                format!("{} / {}", window.used_tokens, window.max_tokens),
            ));
        }
        lines.push(InspectorEntry::field(
            "Session Tokens",
            format_token_usage_brief(session_usage.ledger.cumulative_usage),
        ));
    }
    if !session.token_usage.aggregate_usage.is_zero() {
        lines.push(InspectorEntry::field(
            "Total Tokens",
            format_token_usage_detailed(session.token_usage.aggregate_usage),
        ));
    }
    if !session.token_usage.subagents.is_empty() {
        lines.push(InspectorEntry::section("Subagents"));
        lines.push(InspectorEntry::field(
            "Subagent Count",
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
            "Last Prompt",
            preview_text(prompt, 80),
        ));
    }
    if !session.agent_session_ids.is_empty() {
        lines.push(InspectorEntry::section("Runtime IDs"));
        lines.push(InspectorEntry::field(
            "Runtime Sessions",
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
            "Agent Session Ref",
            session.summary.agent_session_ref.clone(),
        ),
        InspectorEntry::field("Session Ref", session.summary.session_ref.clone()),
        InspectorEntry::field("Label", session.summary.label.clone()),
        InspectorEntry::field("Event Count", session.summary.event_count.to_string()),
        InspectorEntry::field(
            "Message Count",
            session.summary.transcript_message_count.to_string(),
        ),
        InspectorEntry::field("Resume", session.summary.resume_support.label()),
    ];
    if let Some(session_title) = &session.summary.session_title {
        lines.push(InspectorEntry::field(
            "Session Title",
            preview_text(session_title, 80),
        ));
    }
    if let Some(token_usage) = &session.token_usage {
        lines.push(InspectorEntry::section("Token Budget"));
        if let Some(window) = token_usage.ledger.context_window {
            lines.push(InspectorEntry::field(
                "Context",
                format!("{} / {}", window.used_tokens, window.max_tokens),
            ));
        }
        lines.push(InspectorEntry::field(
            "Agent Tokens",
            format_token_usage_brief(token_usage.ledger.cumulative_usage),
        ));
    }
    if let Some(prompt) = &session.summary.last_user_prompt {
        lines.push(InspectorEntry::section("Prompt"));
        lines.push(InspectorEntry::field(
            "Last Prompt",
            preview_text(prompt, 80),
        ));
    }
    if !session.subagents.is_empty() {
        lines.push(InspectorEntry::section("Spawned Subagents"));
        lines.push(InspectorEntry::field(
            "Count",
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
        InspectorEntry::field("Task ID", task.summary.task_id.to_string()),
        InspectorEntry::field("Session Ref", task.summary.session_ref.clone()),
        InspectorEntry::field(
            "Parent Agent Session Ref",
            task.summary.parent_agent_session_ref.clone(),
        ),
        InspectorEntry::field("Role", task.summary.role.clone()),
        InspectorEntry::field("Status", task.summary.status.to_string()),
        InspectorEntry::field("Summary", task.summary.summary.clone()),
    ];
    if let Some(child_session_ref) = &task.summary.child_session_ref {
        lines.push(InspectorEntry::section("Runtime"));
        lines.push(InspectorEntry::field(
            "Child Session Ref",
            child_session_ref.clone(),
        ));
        if let Some(child_agent_session_ref) = &task.summary.child_agent_session_ref {
            lines.push(InspectorEntry::field(
                "Child Agent Session Ref",
                child_agent_session_ref.clone(),
            ));
        }
    }
    lines.push(InspectorEntry::section("Prompt"));
    lines.push(InspectorEntry::field(
        "Prompt",
        preview_text(&task.spec.prompt, 96),
    ));
    if let Some(steer) = &task.spec.steer {
        lines.push(InspectorEntry::field("Steer", preview_text(steer, 96)));
    }
    if !task.spec.requested_write_set.is_empty() {
        lines.push(InspectorEntry::field(
            "Writes",
            preview_text(&task.spec.requested_write_set.join(", "), 96),
        ));
    }
    if !task.spec.dependency_ids.is_empty() {
        lines.push(InspectorEntry::field(
            "Deps",
            preview_text(&task.spec.dependency_ids.join(", "), 96),
        ));
    }
    if let Some(token_usage) = &task.token_usage {
        lines.push(InspectorEntry::section("Token Budget"));
        if let Some(window) = token_usage.ledger.context_window {
            lines.push(InspectorEntry::field(
                "Context",
                format!("{} / {}", window.used_tokens, window.max_tokens),
            ));
        }
        lines.push(InspectorEntry::field(
            "Task Tokens",
            format_token_usage_brief(token_usage.ledger.cumulative_usage),
        ));
    }
    if let Some(result) = &task.result {
        lines.push(InspectorEntry::section("Result"));
        lines.push(InspectorEntry::field(
            "Result",
            preview_text(&result.summary, 96),
        ));
        if !result.claimed_files.is_empty() {
            lines.push(InspectorEntry::field(
                "Claimed Files",
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
        InspectorEntry::field("Local Tools", snapshot.local_tool_count.to_string()),
        InspectorEntry::field("MCP Tools", snapshot.mcp_tool_count.to_string()),
        InspectorEntry::field(
            "Plugins",
            format!(
                "{} Enabled / {} Total",
                snapshot.enabled_plugin_count, snapshot.total_plugin_count
            ),
        ),
        InspectorEntry::field("MCP Servers", snapshot.mcp_servers.len().to_string()),
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
                .map(|warning| InspectorEntry::Muted(format!("Warning: {warning}"))),
        );
    }
    if !snapshot.diagnostics.is_empty() {
        lines.push(InspectorEntry::section("Diagnostics"));
        lines.extend(
            snapshot
                .diagnostics
                .iter()
                .map(|diagnostic| InspectorEntry::Plain(format!("Diagnostic: {diagnostic}"))),
        );
    }
    lines
}

pub(crate) fn format_mcp_server_summary_line(summary: &McpServerSummary) -> InspectorEntry {
    InspectorEntry::collection(
        summary.server_name.clone(),
        Some(format!(
            "Tools={} Prompts={} Resources={}",
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
    InspectorEntry::actionable_collection(
        format!("{}:{}{}", summary.server_name, summary.prompt_name, suffix),
        (!summary.description.is_empty()).then_some(summary.description.clone()),
        InspectorAction::LoadMcpPrompt {
            server_name: summary.server_name.clone(),
            prompt_name: summary.prompt_name.clone(),
        },
    )
}

pub(crate) fn format_mcp_resource_summary_line(summary: &McpResourceSummary) -> InspectorEntry {
    InspectorEntry::actionable_collection(
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
        InspectorAction::LoadMcpResource {
            server_name: summary.server_name.clone(),
            uri: summary.uri.clone(),
        },
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
        "{} {}",
        name,
        format_token_usage_brief(record.ledger.cumulative_usage),
    ))
}

fn format_loaded_subagent_line(subagent: &LoadedSubagentSession) -> TranscriptEntry {
    let token_summary = subagent
        .token_usage
        .as_ref()
        .map(|usage| {
            format!(
                " {}",
                format_token_usage_brief(usage.ledger.cumulative_usage)
            )
        })
        .unwrap_or_default();
    TranscriptEntry::AssistantMessage(format!(
        "{} Role={} Status={} {}{}",
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

fn format_token_usage_brief(usage: TokenUsage) -> String {
    format!(
        "In={} Out={} Cache={}{}",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_tokens,
        format_reasoning_suffix(usage.reasoning_tokens),
    )
}

fn format_token_usage_detailed(usage: TokenUsage) -> String {
    format!(
        "In={} Out={} Prefill={} Decode={} Cache={}{}",
        usage.input_tokens,
        usage.output_tokens,
        usage.uncached_input_tokens(),
        usage.visible_decode_tokens(),
        usage.cache_read_tokens,
        format_reasoning_suffix(usage.reasoning_tokens),
    )
}

fn format_reasoning_suffix(reasoning_tokens: u64) -> String {
    if reasoning_tokens == 0 {
        String::new()
    } else {
        format!(" Reasoning={reasoning_tokens}")
    }
}
