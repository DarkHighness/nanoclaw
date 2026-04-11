use super::state::{
    InspectorEntry, TranscriptEntry, TranscriptShellDetail, TranscriptToolStatus, preview_text,
};
use super::tool_state::{
    execution_update_entry_from_tool_output, plan_update_entry_from_tool_output,
};
use crate::backend::{message_to_text, preview_id};
use crate::tool_render::{
    ToolDetail, tool_argument_details, tool_arguments_preview_lines, tool_output_details,
};
use crate::ui::{
    LiveTaskControlAction, LiveTaskControlOutcome, LiveTaskMessageAction, LiveTaskMessageOutcome,
    LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome, LoadedAgentSession, LoadedSession,
    LoadedSubagentSession, LoadedTask, LoadedTaskMessage, McpPromptSummary, McpResourceSummary,
    McpServerSummary, PersistedAgentSessionSummary, PersistedSessionSearchMatch,
    PersistedSessionSummary, PersistedTaskSummary, SessionExportArtifact, SessionExportKind,
    SessionOperationAction, SessionOperationOutcome, StartupDiagnosticsSnapshot,
};
use agent::types::{
    AgentEnvelopeKind, AgentSessionId, AgentStatus, HookEvent, Message, SessionEventEnvelope,
    SessionEventKind, SessionSummaryTokenUsage,
};
use store::TokenUsageRecord;

mod events;
mod inspectors;
mod outcomes;
mod summaries;

pub(crate) use events::{
    format_session_event_line, format_session_transcript_lines, format_visible_transcript_lines,
    format_visible_transcript_preview_lines,
};
pub(crate) use inspectors::{
    format_agent_session_inspector, format_mcp_prompt_summary_line,
    format_mcp_resource_summary_line, format_mcp_server_summary_line, format_session_inspector,
    format_startup_diagnostics, format_task_inspector,
};
pub(crate) use outcomes::{
    format_live_task_control_outcome, format_live_task_message_outcome,
    format_live_task_wait_outcome, format_session_export_result, format_session_operation_outcome,
};
pub(crate) use summaries::{
    format_agent_session_summary_collection, format_agent_session_summary_line,
    format_live_task_spawn_outcome, format_live_task_summary_line,
    format_session_search_collection, format_session_search_line,
    format_session_summary_collection, format_session_summary_line, format_task_summary_collection,
};

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

#[cfg(test)]
mod tests;
