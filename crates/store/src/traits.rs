use crate::replay::replay_transcript;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use types::{
    AgentSessionId, HookEffect, Message, SessionEventEnvelope, SessionEventKind, SessionId,
    TokenLedgerSnapshot, TokenUsage,
};

const TOKEN_USAGE_CHILD_FETCH_CONCURRENCY_LIMIT: usize = 8;

#[derive(Debug, Error)]
pub enum SessionStoreError {
    #[error("session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("session store IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session store JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SessionStoreError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub first_timestamp_ms: u128,
    pub last_timestamp_ms: u128,
    pub event_count: usize,
    pub agent_session_count: usize,
    pub transcript_message_count: usize,
    pub last_user_prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSearchResult {
    pub summary: SessionSummary,
    pub matched_event_count: usize,
    pub preview_matches: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenUsageScope {
    Session,
    AgentSession,
    Subagent,
    Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub scope: TokenUsageScope,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<AgentSessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub ledger: TokenLedgerSnapshot,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTokenUsageReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<TokenUsageRecord>,
    #[serde(default)]
    pub agent_sessions: Vec<TokenUsageRecord>,
    #[serde(default)]
    pub subagents: Vec<TokenUsageRecord>,
    #[serde(default)]
    pub tasks: Vec<TokenUsageRecord>,
    #[serde(default)]
    pub aggregate_usage: TokenUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemoryExportRequest {
    #[serde(default)]
    pub max_sessions: Option<usize>,
    #[serde(default)]
    pub max_search_corpus_chars: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryExportScope {
    Session,
    AgentSession,
    Subagent,
    Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExportSummary {
    pub scope: MemoryExportScope,
    pub session_id: SessionId,
    pub agent_session_id: Option<AgentSessionId>,
    pub agent_name: Option<String>,
    pub task_id: Option<String>,
    pub first_timestamp_ms: u128,
    pub last_timestamp_ms: u128,
    pub event_count: usize,
    pub transcript_message_count: usize,
    pub last_user_prompt: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExportSections {
    #[serde(default)]
    pub tool_summary: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub failures: Vec<String>,
    #[serde(default)]
    pub produced_artifacts: Vec<String>,
    #[serde(default)]
    pub follow_up: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemoryExportRecord {
    pub summary: MemoryExportSummary,
    pub search_corpus: String,
    pub sections: MemoryExportSections,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemoryExportBundle {
    #[serde(default)]
    pub sessions: Vec<SessionMemoryExportRecord>,
    #[serde(default)]
    pub agent_sessions: Vec<SessionMemoryExportRecord>,
    #[serde(default)]
    pub subagents: Vec<SessionMemoryExportRecord>,
    #[serde(default)]
    pub tasks: Vec<SessionMemoryExportRecord>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroupedMemoryExportEvents {
    pub(crate) agent_sessions: Vec<(AgentSessionId, Vec<SessionEventEnvelope>)>,
    pub(crate) subagents: Vec<ScopedMemoryExportEvents>,
    pub(crate) tasks: Vec<ScopedMemoryExportEvents>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ScopedMemoryExportEvents {
    pub(crate) agent_session_id: Option<AgentSessionId>,
    pub(crate) agent_name: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) events: Vec<SessionEventEnvelope>,
}

#[derive(Clone, Debug, Default)]
struct AgentMemoryExportContext {
    agent_session_id: Option<AgentSessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct ChildSessionTokenUsageContext {
    session_id: Option<SessionId>,
    agent_session_id: Option<AgentSessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
}

#[derive(Clone, Debug)]
struct ResolvedChildSessionTokenUsageContext {
    session_id: SessionId,
    agent_session_id: Option<AgentSessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
}

#[must_use]
pub fn summarize_session_events(
    session_id: &SessionId,
    events: &[SessionEventEnvelope],
) -> Option<SessionSummary> {
    if events.is_empty() {
        return None;
    }

    let mut first_timestamp_ms = u128::MAX;
    let mut last_timestamp_ms = 0;
    let mut agent_session_ids = HashSet::new();
    let mut last_user_prompt = None;

    for event in events {
        first_timestamp_ms = first_timestamp_ms.min(event.timestamp_ms);
        last_timestamp_ms = last_timestamp_ms.max(event.timestamp_ms);
        agent_session_ids.insert(event.agent_session_id.clone());
        if let SessionEventKind::UserPromptSubmit { prompt } = &event.event {
            last_user_prompt = Some(prompt.clone());
        }
    }
    let transcript_message_count = replay_transcript(events).len();

    Some(SessionSummary {
        session_id: session_id.clone(),
        first_timestamp_ms,
        last_timestamp_ms,
        event_count: events.len(),
        agent_session_count: agent_session_ids.len(),
        transcript_message_count,
        last_user_prompt,
    })
}

#[must_use]
pub fn search_session_events(
    summary: &SessionSummary,
    events: &[SessionEventEnvelope],
    query: &str,
) -> Option<SessionSearchResult> {
    let query = query.trim();
    if query.is_empty() {
        return Some(SessionSearchResult {
            summary: summary.clone(),
            matched_event_count: 0,
            preview_matches: Vec::new(),
        });
    }

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();
    let mut matched_event_count = 0;

    if summary
        .session_id
        .as_str()
        .to_lowercase()
        .contains(&query_lower)
    {
        matches.push(format!("session id: {}", summary.session_id));
    }
    if let Some(prompt) = &summary.last_user_prompt {
        if prompt.to_lowercase().contains(&query_lower) {
            matches.push(format!("prompt: {}", preview_text(prompt, 80)));
        }
    }

    for message in replay_transcript(events) {
        let text = message.text_content();
        if !text.to_lowercase().contains(&query_lower) {
            continue;
        }
        matched_event_count += 1;
        if matches.len() < 3 {
            matches.push(preview_text(&text, 80));
        }
    }

    for event in events {
        let event_matches = searchable_event_strings(event)
            .into_iter()
            .filter(|candidate| candidate.to_lowercase().contains(&query_lower))
            .collect::<Vec<_>>();
        if event_matches.is_empty() {
            continue;
        }
        matched_event_count += 1;
        for candidate in event_matches {
            if matches.len() == 3 {
                break;
            }
            matches.push(preview_text(&candidate, 80));
        }
        if matches.len() == 3 {
            break;
        }
    }

    if matches.is_empty() {
        None
    } else {
        Some(SessionSearchResult {
            summary: summary.clone(),
            matched_event_count,
            preview_matches: matches,
        })
    }
}

pub(crate) fn searchable_event_strings(event: &SessionEventEnvelope) -> Vec<String> {
    let mut values = vec![event.agent_session_id.to_string()];
    match &event.event {
        SessionEventKind::SessionStart { reason }
        | SessionEventKind::Stop { reason }
        | SessionEventKind::StopFailure { reason }
        | SessionEventKind::SessionEnd { reason } => {
            if let Some(reason) = reason {
                values.push(reason.clone());
            }
        }
        SessionEventKind::InstructionsLoaded { count } => {
            values.push(format!("instructions {count}"));
        }
        SessionEventKind::SteerApplied { message, reason } => {
            values.push(message.clone());
            if let Some(reason) = reason {
                values.push(reason.clone());
            }
        }
        SessionEventKind::UserPromptSubmit { prompt } => {
            values.push(prompt.clone());
        }
        SessionEventKind::ModelRequestStarted { request } => {
            values.push(
                request
                    .messages
                    .iter()
                    .map(|message| message.text_content())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            values.extend(request.tools.iter().map(|tool| tool.name.to_string()));
        }
        SessionEventKind::CompactionCompleted {
            reason,
            source_message_count,
            retained_message_count,
            summary_chars,
            ..
        } => {
            values.push(reason.clone());
            values.push(format!(
                "compaction {source_message_count} {retained_message_count} {summary_chars}"
            ));
        }
        SessionEventKind::ModelResponseCompleted {
            assistant_text,
            tool_calls,
            ..
        } => {
            values.push(assistant_text.clone());
            values.extend(tool_calls.iter().map(|call| call.tool_name.to_string()));
        }
        SessionEventKind::TokenUsageUpdated { phase, ledger } => {
            values.push(format!(
                "token_usage {:?} context={} input={} output={} prefill={} decode={} cache_read={}",
                phase,
                ledger
                    .context_window
                    .map(|usage| format!("{}/{}", usage.used_tokens, usage.max_tokens))
                    .unwrap_or_else(|| "unknown".to_string()),
                ledger.cumulative_usage.input_tokens,
                ledger.cumulative_usage.output_tokens,
                ledger.cumulative_usage.prefill_tokens,
                ledger.cumulative_usage.decode_tokens,
                ledger.cumulative_usage.cache_read_tokens,
            ));
        }
        SessionEventKind::HookInvoked { hook_name, .. } => {
            values.push(hook_name.clone());
        }
        SessionEventKind::HookCompleted {
            hook_name, output, ..
        } => {
            values.push(hook_name.clone());
            for effect in &output.effects {
                match effect {
                    HookEffect::AppendMessage { parts, .. } => {
                        values.extend(parts.iter().filter_map(|part| match part {
                            types::MessagePart::Text { text } => Some(text.clone()),
                            _ => None,
                        }));
                    }
                    HookEffect::AddContext { text } | HookEffect::InjectInstruction { text } => {
                        values.push(text.clone());
                    }
                    HookEffect::Stop { reason } => {
                        values.push(reason.clone());
                    }
                    HookEffect::RewriteToolArgs {
                        tool_name,
                        arguments,
                    } => {
                        values.push(tool_name.to_string());
                        values.push(arguments.to_string());
                    }
                    HookEffect::SetGateDecision { reason, .. }
                    | HookEffect::SetPermissionDecision { reason, .. }
                    | HookEffect::SetPermissionBehavior { reason, .. } => {
                        if let Some(reason) = reason {
                            values.push(reason.clone());
                        }
                    }
                    HookEffect::ReplaceMessage { message, .. } => {
                        values.push(message.text_content());
                    }
                    HookEffect::PatchMessage { .. }
                    | HookEffect::RemoveMessage { .. }
                    | HookEffect::Elicitation { .. } => {}
                }
            }
        }
        SessionEventKind::TranscriptMessage { .. } => {}
        SessionEventKind::TranscriptMessagePatched { message_id, .. } => {
            values.push(message_id.to_string());
            values.push("transcript patched".to_string());
        }
        SessionEventKind::TranscriptMessageRemoved { message_id } => {
            values.push(message_id.to_string());
            values.push("transcript removed".to_string());
        }
        SessionEventKind::ToolApprovalRequested { call, reasons } => {
            values.push(call.tool_name.to_string());
            values.extend(reasons.clone());
        }
        SessionEventKind::ToolApprovalResolved { call, reason, .. } => {
            values.push(call.tool_name.to_string());
            if let Some(reason) = reason {
                values.push(reason.clone());
            }
        }
        SessionEventKind::ToolCallStarted { call } => {
            values.push(call.tool_name.to_string());
            values.push(call.arguments.to_string());
        }
        SessionEventKind::ToolCallCompleted { call, output } => {
            values.push(call.tool_name.to_string());
            values.push(output.text_content());
            if let Some(metadata) = &output.metadata {
                values.push(metadata.to_string());
            }
        }
        SessionEventKind::ToolCallFailed { call, error } => {
            values.push(call.tool_name.to_string());
            values.push(error.clone());
        }
        SessionEventKind::TaskCreated {
            task,
            parent_agent_id,
        } => {
            values.push(task.task_id.clone());
            values.push(task.role.clone());
            values.push(task.prompt.clone());
            values.extend(task.requested_write_set.clone());
            if let Some(parent_agent_id) = parent_agent_id {
                values.push(parent_agent_id.to_string());
            }
        }
        SessionEventKind::TaskCompleted {
            task_id,
            agent_id,
            status,
        } => {
            values.push(task_id.clone());
            values.push(agent_id.to_string());
            values.push(status.to_string());
        }
        SessionEventKind::SubagentStart { handle, task } => {
            values.push(handle.agent_id.to_string());
            values.push(handle.task_id.clone());
            values.push(handle.role.clone());
            values.push(task.prompt.clone());
            values.extend(task.requested_write_set.clone());
        }
        SessionEventKind::AgentEnvelope { envelope } => match &envelope.kind {
            types::AgentEnvelopeKind::SpawnRequested { task }
            | types::AgentEnvelopeKind::Started { task } => {
                values.push(task.task_id.clone());
                values.push(task.role.clone());
            }
            types::AgentEnvelopeKind::StatusChanged { status } => {
                values.push(status.to_string());
            }
            types::AgentEnvelopeKind::Input { message, delivery } => {
                values.push(delivery.to_string());
                values.push(message.text_content());
                values.push(serde_json::to_string(message).unwrap_or_default());
            }
            types::AgentEnvelopeKind::Artifact { artifact } => {
                values.push(artifact.kind.clone());
                values.push(artifact.uri.clone());
                if let Some(label) = &artifact.label {
                    values.push(label.clone());
                }
            }
            types::AgentEnvelopeKind::ClaimRequested { files }
            | types::AgentEnvelopeKind::ClaimGranted { files } => {
                values.extend(files.clone());
            }
            types::AgentEnvelopeKind::ClaimRejected { files, owner } => {
                values.extend(files.clone());
                values.push(owner.to_string());
            }
            types::AgentEnvelopeKind::Result { result } => {
                values.push(result.task_id.clone());
                values.push(result.agent_id.to_string());
                values.push(result.status.to_string());
                values.push(result.summary.clone());
                values.push(result.text.clone());
                values.extend(result.claimed_files.clone());
                values.extend(result.artifacts.iter().map(|artifact| artifact.uri.clone()));
            }
            types::AgentEnvelopeKind::Failed { error } => {
                values.push(error.clone());
            }
            types::AgentEnvelopeKind::Cancelled { reason } => {
                if let Some(reason) = reason {
                    values.push(reason.clone());
                }
            }
            types::AgentEnvelopeKind::Heartbeat => values.push("heartbeat".to_string()),
        },
        SessionEventKind::SubagentStop {
            handle,
            result,
            error,
        } => {
            values.push(handle.agent_id.to_string());
            values.push(handle.task_id.clone());
            values.push(handle.status.to_string());
            if let Some(result) = result {
                values.push(result.summary.clone());
                values.push(result.text.clone());
                values.extend(result.claimed_files.clone());
            }
            if let Some(error) = error {
                values.push(error.clone());
            }
        }
        SessionEventKind::Notification { source, message } => {
            values.push(source.clone());
            values.push(message.clone());
        }
    }
    values.retain(|value| !value.trim().is_empty());
    values
}

#[must_use]
pub fn latest_token_usage_snapshot(events: &[SessionEventEnvelope]) -> Option<TokenLedgerSnapshot> {
    events.iter().rev().find_map(|event| match &event.event {
        SessionEventKind::TokenUsageUpdated { ledger, .. } => Some(ledger.clone()),
        _ => None,
    })
}

#[must_use]
pub fn session_token_usage_snapshot(
    events: &[SessionEventEnvelope],
) -> Option<TokenLedgerSnapshot> {
    let mut latest_by_agent_session = BTreeMap::<AgentSessionId, TokenLedgerSnapshot>::new();
    let mut session_ledger = None;
    for event in events {
        if let SessionEventKind::TokenUsageUpdated { ledger, .. } = &event.event {
            latest_by_agent_session.insert(event.agent_session_id.clone(), ledger.clone());
            session_ledger = Some(ledger.clone());
        }
    }

    let mut session_ledger = session_ledger?;
    // A top-level Session can span multiple root AgentSessions after compaction.
    // The session-wide ledger must therefore aggregate the final cumulative
    // ledger from each root AgentSession instead of reusing only the latest one.
    session_ledger.cumulative_usage = TokenUsage::default();
    for ledger in latest_by_agent_session.values() {
        session_ledger
            .cumulative_usage
            .accumulate(&ledger.cumulative_usage);
    }
    Some(session_ledger)
}

#[must_use]
pub fn agent_session_token_usage_records(
    session_id: &SessionId,
    events: &[SessionEventEnvelope],
) -> Vec<TokenUsageRecord> {
    let mut by_session = BTreeMap::<AgentSessionId, TokenLedgerSnapshot>::new();
    for event in events {
        if let SessionEventKind::TokenUsageUpdated { ledger, .. } = &event.event {
            by_session.insert(event.agent_session_id.clone(), ledger.clone());
        }
    }
    by_session
        .into_iter()
        .map(|(agent_session_id, ledger)| TokenUsageRecord {
            scope: TokenUsageScope::AgentSession,
            session_id: session_id.clone(),
            agent_session_id: Some(agent_session_id),
            agent_name: None,
            task_id: None,
            ledger,
        })
        .collect()
}

fn collect_child_run_token_usage_contexts(
    events: &[SessionEventEnvelope],
) -> Vec<ResolvedChildSessionTokenUsageContext> {
    let mut by_agent = BTreeMap::<String, ChildSessionTokenUsageContext>::new();
    for event in events {
        match &event.event {
            SessionEventKind::SubagentStart { handle, task } => {
                let context = by_agent.entry(handle.agent_id.to_string()).or_default();
                context.session_id = Some(handle.session_id.clone());
                context.agent_session_id = Some(handle.agent_session_id.clone());
                if context.agent_name.is_none() {
                    context.agent_name = Some(task.role.clone());
                }
                if context.task_id.is_none() {
                    context.task_id = Some(task.task_id.clone());
                }
            }
            SessionEventKind::SubagentStop { handle, .. } => {
                let context = by_agent.entry(handle.agent_id.to_string()).or_default();
                context.session_id = Some(handle.session_id.clone());
                context.agent_session_id = Some(handle.agent_session_id.clone());
                if context.agent_name.is_none() {
                    context.agent_name = Some(handle.role.clone());
                }
                if context.task_id.is_none() {
                    context.task_id = Some(handle.task_id.clone());
                }
            }
            SessionEventKind::AgentEnvelope { envelope } => {
                let context = by_agent.entry(envelope.agent_id.to_string()).or_default();
                if context.session_id.is_none() {
                    context.session_id = Some(envelope.session_id.clone());
                }
                if context.agent_session_id.is_none() {
                    context.agent_session_id = Some(envelope.agent_session_id.clone());
                }
                match &envelope.kind {
                    types::AgentEnvelopeKind::SpawnRequested { task }
                    | types::AgentEnvelopeKind::Started { task } => {
                        if context.agent_name.is_none() {
                            context.agent_name = Some(task.role.clone());
                        }
                        if context.task_id.is_none() {
                            context.task_id = Some(task.task_id.clone());
                        }
                    }
                    types::AgentEnvelopeKind::Result { result } => {
                        if context.task_id.is_none() {
                            context.task_id = Some(result.task_id.clone());
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let mut contexts = by_agent
        .into_values()
        .filter_map(|context| {
            Some(ResolvedChildSessionTokenUsageContext {
                session_id: context.session_id?,
                agent_session_id: context.agent_session_id,
                agent_name: context.agent_name,
                task_id: context.task_id,
            })
        })
        .collect::<Vec<_>>();
    contexts.sort_by(|left, right| {
        left.agent_name
            .cmp(&right.agent_name)
            .then_with(|| left.task_id.cmp(&right.task_id))
            .then_with(|| left.session_id.as_str().cmp(right.session_id.as_str()))
    });
    contexts
}

pub(crate) fn build_search_corpus(events: &[SessionEventEnvelope]) -> String {
    let mut corpus = String::new();
    for event in events {
        for value in searchable_event_strings(event) {
            append_search_corpus_line(&mut corpus, &value);
        }
    }
    for message in replay_transcript(events) {
        append_search_corpus_line(&mut corpus, &message.text_content());
    }
    corpus
}

#[must_use]
pub(crate) fn group_events_for_memory_export(
    events: &[SessionEventEnvelope],
) -> GroupedMemoryExportEvents {
    let mut agent_sessions = BTreeMap::<AgentSessionId, Vec<SessionEventEnvelope>>::new();
    let mut subagents = BTreeMap::<String, ScopedMemoryExportEvents>::new();
    let mut tasks = BTreeMap::<String, ScopedMemoryExportEvents>::new();
    let mut agent_contexts = BTreeMap::<String, AgentMemoryExportContext>::new();

    for event in events {
        agent_sessions
            .entry(event.agent_session_id.clone())
            .or_default()
            .push(event.clone());

        match &event.event {
            // Task lifecycle lives on the parent session stream. We keep those
            // records under task scope and later overwrite the fallback session
            // with the child session once spawn/start events provide it.
            SessionEventKind::TaskCreated { task, .. } => {
                let group = tasks.entry(task.task_id.clone()).or_default();
                group.task_id = Some(task.task_id.clone());
                if group.agent_session_id.is_none() {
                    group.agent_session_id = Some(event.agent_session_id.clone());
                }
                group.events.push(event.clone());
            }
            SessionEventKind::TaskCompleted {
                task_id, agent_id, ..
            } => {
                let context = agent_contexts
                    .get(&agent_id.to_string())
                    .cloned()
                    .unwrap_or_default();
                push_task_event(
                    &mut tasks,
                    task_id.clone(),
                    Some(&context),
                    Some(&event.agent_session_id),
                    event,
                );
                if !context.agent_name.is_none() || subagents.contains_key(&agent_id.to_string()) {
                    push_subagent_event(&mut subagents, agent_id.to_string(), &context, event);
                }
            }
            SessionEventKind::SubagentStart { handle, task } => {
                let context = update_agent_memory_export_context(
                    &mut agent_contexts,
                    &handle.agent_id.to_string(),
                    Some(&handle.agent_session_id),
                    Some(handle.role.as_str()),
                    Some(task.task_id.as_str()),
                );
                push_subagent_event(&mut subagents, handle.agent_id.to_string(), &context, event);
                push_task_event(
                    &mut tasks,
                    task.task_id.clone(),
                    Some(&context),
                    Some(&handle.agent_session_id),
                    event,
                );
            }
            SessionEventKind::AgentEnvelope { envelope } => {
                let agent_key = envelope.agent_id.to_string();
                let context = match &envelope.kind {
                    types::AgentEnvelopeKind::SpawnRequested { task }
                    | types::AgentEnvelopeKind::Started { task } => {
                        let context = update_agent_memory_export_context(
                            &mut agent_contexts,
                            &agent_key,
                            Some(&envelope.agent_session_id),
                            Some(task.role.as_str()),
                            Some(task.task_id.as_str()),
                        );
                        push_task_event(
                            &mut tasks,
                            task.task_id.clone(),
                            Some(&context),
                            Some(&envelope.agent_session_id),
                            event,
                        );
                        context
                    }
                    types::AgentEnvelopeKind::Result { result } => {
                        let context = update_agent_memory_export_context(
                            &mut agent_contexts,
                            &agent_key,
                            Some(&envelope.agent_session_id),
                            None,
                            Some(result.task_id.as_str()),
                        );
                        push_task_event(
                            &mut tasks,
                            result.task_id.clone(),
                            Some(&context),
                            Some(&envelope.agent_session_id),
                            event,
                        );
                        context
                    }
                    _ => {
                        let context = update_agent_memory_export_context(
                            &mut agent_contexts,
                            &agent_key,
                            Some(&envelope.agent_session_id),
                            None,
                            None,
                        );
                        if let Some(task_id) = &context.task_id {
                            push_task_event(
                                &mut tasks,
                                task_id.clone(),
                                Some(&context),
                                Some(&envelope.agent_session_id),
                                event,
                            );
                        }
                        context
                    }
                };
                push_subagent_event(&mut subagents, agent_key, &context, event);
            }
            SessionEventKind::SubagentStop { handle, .. } => {
                let context = update_agent_memory_export_context(
                    &mut agent_contexts,
                    &handle.agent_id.to_string(),
                    Some(&handle.agent_session_id),
                    Some(handle.role.as_str()),
                    Some(handle.task_id.as_str()),
                );
                push_subagent_event(&mut subagents, handle.agent_id.to_string(), &context, event);
                push_task_event(
                    &mut tasks,
                    handle.task_id.clone(),
                    Some(&context),
                    Some(&handle.agent_session_id),
                    event,
                );
            }
            _ => {}
        }
    }

    GroupedMemoryExportEvents {
        agent_sessions: agent_sessions.into_iter().collect(),
        subagents: subagents.into_values().collect(),
        tasks: tasks.into_values().collect(),
    }
}

fn update_agent_memory_export_context(
    contexts: &mut BTreeMap<String, AgentMemoryExportContext>,
    agent_key: &str,
    agent_session_id: Option<&AgentSessionId>,
    agent_name: Option<&str>,
    task_id: Option<&str>,
) -> AgentMemoryExportContext {
    let context = contexts.entry(agent_key.to_string()).or_default();
    if let Some(agent_session_id) = agent_session_id {
        context.agent_session_id = Some(agent_session_id.clone());
    }
    if let Some(agent_name) = agent_name {
        let agent_name = agent_name.trim();
        if !agent_name.is_empty() {
            context.agent_name = Some(agent_name.to_string());
        }
    }
    if let Some(task_id) = task_id {
        let task_id = task_id.trim();
        if !task_id.is_empty() {
            context.task_id = Some(task_id.to_string());
        }
    }
    context.clone()
}

fn push_subagent_event(
    groups: &mut BTreeMap<String, ScopedMemoryExportEvents>,
    agent_key: String,
    context: &AgentMemoryExportContext,
    event: &SessionEventEnvelope,
) {
    let group = groups.entry(agent_key).or_default();
    apply_memory_export_context(group, context, None);
    group.events.push(event.clone());
}

fn push_task_event(
    groups: &mut BTreeMap<String, ScopedMemoryExportEvents>,
    task_key: String,
    context: Option<&AgentMemoryExportContext>,
    fallback_agent_session_id: Option<&AgentSessionId>,
    event: &SessionEventEnvelope,
) {
    let group = groups.entry(task_key.clone()).or_default();
    group.task_id = Some(task_key);
    if let Some(context) = context {
        apply_memory_export_context(group, context, fallback_agent_session_id);
    } else if group.agent_session_id.is_none() {
        group.agent_session_id = fallback_agent_session_id.cloned();
    }
    group.events.push(event.clone());
}

fn apply_memory_export_context(
    group: &mut ScopedMemoryExportEvents,
    context: &AgentMemoryExportContext,
    fallback_agent_session_id: Option<&AgentSessionId>,
) {
    if let Some(agent_session_id) = &context.agent_session_id {
        group.agent_session_id = Some(agent_session_id.clone());
    } else if group.agent_session_id.is_none() {
        group.agent_session_id = fallback_agent_session_id.cloned();
    }
    if let Some(agent_name) = &context.agent_name {
        group.agent_name = Some(agent_name.clone());
    }
    if let Some(task_id) = &context.task_id {
        group.task_id = Some(task_id.clone());
    }
}

#[must_use]
pub fn build_memory_export_record(
    scope: MemoryExportScope,
    session_id: &SessionId,
    agent_session_id: Option<AgentSessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
    events: &[SessionEventEnvelope],
) -> Option<SessionMemoryExportRecord> {
    if events.is_empty() {
        return None;
    }

    let mut first_timestamp_ms = u128::MAX;
    let mut last_timestamp_ms = 0;
    let mut last_user_prompt = None;

    for event in events {
        first_timestamp_ms = first_timestamp_ms.min(event.timestamp_ms);
        last_timestamp_ms = last_timestamp_ms.max(event.timestamp_ms);
        if let SessionEventKind::UserPromptSubmit { prompt } = &event.event {
            last_user_prompt = Some(prompt.clone());
        }
    }
    let transcript_message_count = replay_transcript(events).len();

    Some(SessionMemoryExportRecord {
        summary: MemoryExportSummary {
            scope,
            session_id: session_id.clone(),
            agent_session_id,
            agent_name,
            task_id,
            first_timestamp_ms,
            last_timestamp_ms,
            event_count: events.len(),
            transcript_message_count,
            last_user_prompt,
        },
        search_corpus: build_search_corpus(events),
        sections: collect_memory_export_sections(events),
    })
}

pub(crate) fn sort_memory_export_records(records: &mut [SessionMemoryExportRecord]) {
    records.sort_by(|left, right| {
        right
            .summary
            .last_timestamp_ms
            .cmp(&left.summary.last_timestamp_ms)
            .then_with(|| {
                left.summary
                    .session_id
                    .as_str()
                    .cmp(right.summary.session_id.as_str())
            })
            .then_with(|| {
                left.summary
                    .agent_session_id
                    .cmp(&right.summary.agent_session_id)
            })
            .then_with(|| left.summary.agent_name.cmp(&right.summary.agent_name))
            .then_with(|| left.summary.task_id.cmp(&right.summary.task_id))
    });
}

pub(crate) fn apply_memory_export_request(
    bundle: &mut SessionMemoryExportBundle,
    request: &SessionMemoryExportRequest,
) {
    if let Some(max_sessions) = request.max_sessions {
        bundle.sessions.truncate(max_sessions);
        bundle.agent_sessions.truncate(max_sessions);
        bundle.subagents.truncate(max_sessions);
        bundle.tasks.truncate(max_sessions);
    }
    if let Some(max_chars) = request.max_search_corpus_chars {
        for record in bundle
            .sessions
            .iter_mut()
            .chain(bundle.agent_sessions.iter_mut())
            .chain(bundle.subagents.iter_mut())
            .chain(bundle.tasks.iter_mut())
        {
            record.search_corpus = keep_recent_chars(&record.search_corpus, max_chars);
        }
    }
}

pub(crate) fn append_search_corpus_line(search_corpus: &mut String, value: &str) {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return;
    }
    if !search_corpus.is_empty() {
        search_corpus.push('\n');
    }
    search_corpus.push_str(&normalized);
}

pub(crate) fn keep_recent_chars(search_corpus: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let total_chars = search_corpus.chars().count();
    if total_chars <= max_chars {
        return search_corpus.to_string();
    }

    // Memory exports intentionally keep the newest tail because downstream
    // indexing should prefer the latest operational context over stale prelude.
    search_corpus
        .chars()
        .skip(total_chars - max_chars)
        .collect::<String>()
}

fn collect_memory_export_sections(events: &[SessionEventEnvelope]) -> MemoryExportSections {
    let mut sections = MemoryExportSections::default();

    for event in events {
        match &event.event {
            SessionEventKind::SteerApplied { message, reason } => {
                push_unique(&mut sections.decisions, preview_text(message, 120));
                if let Some(reason) = reason {
                    push_unique(&mut sections.decisions, preview_text(reason, 120));
                }
            }
            SessionEventKind::CompactionCompleted { reason, .. } => {
                push_unique(
                    &mut sections.follow_up,
                    format!("compaction: {}", preview_text(reason, 120)),
                );
            }
            SessionEventKind::HookCompleted {
                hook_name, output, ..
            } => {
                for effect in &output.effects {
                    match effect {
                        HookEffect::AppendMessage { parts, .. } => {
                            for text in parts.iter().filter_map(|part| match part {
                                types::MessagePart::Text { text } => Some(text.as_str()),
                                _ => None,
                            }) {
                                push_unique(
                                    &mut sections.decisions,
                                    format!("{hook_name}: {}", preview_text(text, 120)),
                                );
                            }
                        }
                        HookEffect::ReplaceMessage { message, .. } => {
                            push_unique(
                                &mut sections.decisions,
                                format!(
                                    "{hook_name}: {}",
                                    preview_text(&message.text_content(), 120)
                                ),
                            );
                        }
                        HookEffect::AddContext { text }
                        | HookEffect::InjectInstruction { text } => {
                            push_unique(
                                &mut sections.follow_up,
                                format!("{hook_name}: {}", preview_text(text, 120)),
                            );
                        }
                        HookEffect::Stop { reason } => {
                            push_unique(
                                &mut sections.failures,
                                format!("{hook_name}: {}", preview_text(reason, 120)),
                            );
                        }
                        HookEffect::SetGateDecision { reason, .. }
                        | HookEffect::SetPermissionDecision { reason, .. }
                        | HookEffect::SetPermissionBehavior { reason, .. } => {
                            if let Some(reason) = reason {
                                push_unique(
                                    &mut sections.decisions,
                                    format!("{hook_name}: {}", preview_text(reason, 120)),
                                );
                            }
                        }
                        HookEffect::RewriteToolArgs {
                            tool_name,
                            arguments,
                        } => {
                            push_unique(
                                &mut sections.follow_up,
                                format!(
                                    "{hook_name}: rewrite {} {}",
                                    tool_name,
                                    preview_text(&arguments.to_string(), 120)
                                ),
                            );
                        }
                        HookEffect::PatchMessage { .. }
                        | HookEffect::RemoveMessage { .. }
                        | HookEffect::Elicitation { .. } => {}
                    }
                }
            }
            SessionEventKind::ToolApprovalResolved {
                call,
                approved,
                reason,
            } => {
                let verdict = if *approved { "approved" } else { "denied" };
                let mut line = format!("{} {verdict}", call.tool_name);
                if let Some(reason) = reason {
                    line.push_str(": ");
                    line.push_str(&preview_text(reason, 80));
                }
                push_unique(&mut sections.decisions, line);
            }
            SessionEventKind::ToolCallCompleted { call, output } => {
                push_unique(
                    &mut sections.tool_summary,
                    format!("{} completed", call.tool_name),
                );
                for artifact in extract_artifacts(output.metadata.as_ref())
                    .into_iter()
                    .chain(extract_artifacts(output.structured_content.as_ref()))
                {
                    push_unique(&mut sections.produced_artifacts, artifact);
                }
            }
            SessionEventKind::ToolCallFailed { call, error } => {
                push_unique(
                    &mut sections.tool_summary,
                    format!("{} failed", call.tool_name),
                );
                push_unique(
                    &mut sections.failures,
                    format!("{}: {}", call.tool_name, preview_text(error, 120)),
                );
            }
            SessionEventKind::Notification { source, message } => {
                push_unique(
                    &mut sections.follow_up,
                    format!("{source}: {}", preview_text(message, 120)),
                );
            }
            SessionEventKind::StopFailure { reason } => {
                if let Some(reason) = reason {
                    push_unique(&mut sections.failures, preview_text(reason, 120));
                }
            }
            SessionEventKind::Stop { reason } | SessionEventKind::SessionEnd { reason } => {
                if let Some(reason) = reason {
                    push_unique(&mut sections.follow_up, preview_text(reason, 120));
                }
            }
            _ => {}
        }
    }

    sections
}

fn extract_artifacts(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(artifacts) = value.get("artifacts").and_then(Value::as_array) else {
        return Vec::new();
    };
    artifacts
        .iter()
        .filter_map(|artifact| artifact.as_str().map(|artifact| artifact.to_string()))
        .collect()
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|candidate| candidate == &value) {
        values.push(value);
    }
}

fn preview_text(value: &str, max_chars: usize) -> String {
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

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn append(&self, event: SessionEventEnvelope) -> Result<()>;

    async fn append_batch(&self, events: Vec<SessionEventEnvelope>) -> Result<()> {
        for event in events {
            self.append(event).await?;
        }
        Ok(())
    }
}

#[async_trait]
pub trait SessionStore: EventSink {
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;
    async fn search_sessions(&self, query: &str) -> Result<Vec<SessionSearchResult>>;
    async fn events(&self, session_id: &SessionId) -> Result<Vec<SessionEventEnvelope>>;
    async fn agent_session_ids(&self, session_id: &SessionId) -> Result<Vec<AgentSessionId>>;
    async fn replay_transcript(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    async fn token_usage(&self, session_id: &SessionId) -> Result<SessionTokenUsageReport> {
        let root_events = self.events(session_id).await?;
        let session = session_token_usage_snapshot(&root_events).map(|ledger| TokenUsageRecord {
            scope: TokenUsageScope::Session,
            session_id: session_id.clone(),
            agent_session_id: None,
            agent_name: None,
            task_id: None,
            ledger,
        });
        let agent_sessions = agent_session_token_usage_records(session_id, &root_events);
        let mut aggregate_usage = session
            .as_ref()
            .map(|record| record.ledger.cumulative_usage)
            .unwrap_or_default();

        let child_contexts = collect_child_run_token_usage_contexts(&root_events);
        let child_records = stream::iter(child_contexts.into_iter().map(|context| async move {
            let events = self.events(&context.session_id).await?;
            Ok::<_, SessionStoreError>((context, latest_token_usage_snapshot(&events)))
        }))
        .buffer_unordered(TOKEN_USAGE_CHILD_FETCH_CONCURRENCY_LIMIT)
        .collect::<Vec<_>>()
        .await;

        let mut subagents = Vec::new();
        let mut tasks = Vec::new();
        for child in child_records {
            let (context, ledger) = child?;
            let Some(ledger) = ledger else {
                continue;
            };
            aggregate_usage.accumulate(&ledger.cumulative_usage);
            subagents.push(TokenUsageRecord {
                scope: TokenUsageScope::Subagent,
                session_id: context.session_id.clone(),
                agent_session_id: context.agent_session_id.clone(),
                agent_name: context.agent_name.clone(),
                task_id: context.task_id.clone(),
                ledger: ledger.clone(),
            });
            tasks.push(TokenUsageRecord {
                scope: TokenUsageScope::Task,
                session_id: context.session_id,
                agent_session_id: context.agent_session_id,
                agent_name: context.agent_name,
                task_id: context.task_id,
                ledger,
            });
        }

        Ok(SessionTokenUsageReport {
            session,
            agent_sessions,
            subagents,
            tasks,
            aggregate_usage,
        })
    }
    async fn export_for_memory(
        &self,
        request: SessionMemoryExportRequest,
    ) -> Result<SessionMemoryExportBundle>;
}
