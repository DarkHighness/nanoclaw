use crate::replay::replay_transcript;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use types::{
    HookEffect, Message, RunEventEnvelope, RunEventKind, RunId, SessionId, TokenLedgerSnapshot,
    TokenUsage,
};

const TOKEN_USAGE_CHILD_FETCH_CONCURRENCY_LIMIT: usize = 8;

#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error("run not found: {0}")]
    RunNotFound(RunId),
    #[error("run store IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("run store JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, RunStoreError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: RunId,
    pub first_timestamp_ms: u128,
    pub last_timestamp_ms: u128,
    pub event_count: usize,
    pub session_count: usize,
    pub transcript_message_count: usize,
    pub last_user_prompt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSearchResult {
    pub summary: RunSummary,
    pub matched_event_count: usize,
    pub preview_matches: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenUsageScope {
    Run,
    Session,
    Subagent,
    Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub scope: TokenUsageScope,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub ledger: TokenLedgerSnapshot,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTokenUsageReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<TokenUsageRecord>,
    #[serde(default)]
    pub sessions: Vec<TokenUsageRecord>,
    #[serde(default)]
    pub subagents: Vec<TokenUsageRecord>,
    #[serde(default)]
    pub tasks: Vec<TokenUsageRecord>,
    #[serde(default)]
    pub aggregate_usage: TokenUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMemoryExportRequest {
    #[serde(default)]
    pub max_runs: Option<usize>,
    #[serde(default)]
    pub max_search_corpus_chars: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryExportScope {
    Run,
    Session,
    Subagent,
    Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExportSummary {
    pub scope: MemoryExportScope,
    pub run_id: RunId,
    pub session_id: Option<SessionId>,
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
pub struct RunMemoryExportRecord {
    pub summary: MemoryExportSummary,
    pub search_corpus: String,
    pub sections: MemoryExportSections,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMemoryExportBundle {
    #[serde(default)]
    pub runs: Vec<RunMemoryExportRecord>,
    #[serde(default)]
    pub sessions: Vec<RunMemoryExportRecord>,
    #[serde(default)]
    pub subagents: Vec<RunMemoryExportRecord>,
    #[serde(default)]
    pub tasks: Vec<RunMemoryExportRecord>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroupedMemoryExportEvents {
    pub(crate) sessions: Vec<(SessionId, Vec<RunEventEnvelope>)>,
    pub(crate) subagents: Vec<ScopedMemoryExportEvents>,
    pub(crate) tasks: Vec<ScopedMemoryExportEvents>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ScopedMemoryExportEvents {
    pub(crate) session_id: Option<SessionId>,
    pub(crate) agent_name: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) events: Vec<RunEventEnvelope>,
}

#[derive(Clone, Debug, Default)]
struct AgentMemoryExportContext {
    session_id: Option<SessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct ChildRunTokenUsageContext {
    run_id: Option<RunId>,
    session_id: Option<SessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
}

#[derive(Clone, Debug)]
struct ResolvedChildRunTokenUsageContext {
    run_id: RunId,
    session_id: Option<SessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
}

#[must_use]
pub fn summarize_run_events(run_id: &RunId, events: &[RunEventEnvelope]) -> Option<RunSummary> {
    if events.is_empty() {
        return None;
    }

    let mut first_timestamp_ms = u128::MAX;
    let mut last_timestamp_ms = 0;
    let mut session_ids = HashSet::new();
    let mut last_user_prompt = None;

    for event in events {
        first_timestamp_ms = first_timestamp_ms.min(event.timestamp_ms);
        last_timestamp_ms = last_timestamp_ms.max(event.timestamp_ms);
        session_ids.insert(event.session_id.clone());
        if let RunEventKind::UserPromptSubmit { prompt } = &event.event {
            last_user_prompt = Some(prompt.clone());
        }
    }
    let transcript_message_count = replay_transcript(events).len();

    Some(RunSummary {
        run_id: run_id.clone(),
        first_timestamp_ms,
        last_timestamp_ms,
        event_count: events.len(),
        session_count: session_ids.len(),
        transcript_message_count,
        last_user_prompt,
    })
}

#[must_use]
pub fn search_run_events(
    summary: &RunSummary,
    events: &[RunEventEnvelope],
    query: &str,
) -> Option<RunSearchResult> {
    let query = query.trim();
    if query.is_empty() {
        return Some(RunSearchResult {
            summary: summary.clone(),
            matched_event_count: 0,
            preview_matches: Vec::new(),
        });
    }

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();
    let mut matched_event_count = 0;

    if summary
        .run_id
        .as_str()
        .to_lowercase()
        .contains(&query_lower)
    {
        matches.push(format!("run id: {}", summary.run_id));
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
        Some(RunSearchResult {
            summary: summary.clone(),
            matched_event_count,
            preview_matches: matches,
        })
    }
}

pub(crate) fn searchable_event_strings(event: &RunEventEnvelope) -> Vec<String> {
    let mut values = vec![event.session_id.to_string()];
    match &event.event {
        RunEventKind::SessionStart { reason }
        | RunEventKind::Stop { reason }
        | RunEventKind::StopFailure { reason }
        | RunEventKind::SessionEnd { reason } => {
            if let Some(reason) = reason {
                values.push(reason.clone());
            }
        }
        RunEventKind::InstructionsLoaded { count } => {
            values.push(format!("instructions {count}"));
        }
        RunEventKind::SteerApplied { message, reason } => {
            values.push(message.clone());
            if let Some(reason) = reason {
                values.push(reason.clone());
            }
        }
        RunEventKind::UserPromptSubmit { prompt } => {
            values.push(prompt.clone());
        }
        RunEventKind::ModelRequestStarted { request } => {
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
        RunEventKind::CompactionCompleted {
            reason,
            source_message_count,
            retained_message_count,
            summary_chars,
        } => {
            values.push(reason.clone());
            values.push(format!(
                "compaction {source_message_count} {retained_message_count} {summary_chars}"
            ));
        }
        RunEventKind::ModelResponseCompleted {
            assistant_text,
            tool_calls,
            ..
        } => {
            values.push(assistant_text.clone());
            values.extend(tool_calls.iter().map(|call| call.tool_name.to_string()));
        }
        RunEventKind::TokenUsageUpdated { phase, ledger } => {
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
        RunEventKind::HookInvoked { hook_name, .. } => {
            values.push(hook_name.clone());
        }
        RunEventKind::HookCompleted {
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
        RunEventKind::TranscriptMessage { .. } => {}
        RunEventKind::TranscriptMessagePatched { message_id, .. } => {
            values.push(message_id.to_string());
            values.push("transcript patched".to_string());
        }
        RunEventKind::TranscriptMessageRemoved { message_id } => {
            values.push(message_id.to_string());
            values.push("transcript removed".to_string());
        }
        RunEventKind::ToolApprovalRequested { call, reasons } => {
            values.push(call.tool_name.to_string());
            values.extend(reasons.clone());
        }
        RunEventKind::ToolApprovalResolved { call, reason, .. } => {
            values.push(call.tool_name.to_string());
            if let Some(reason) = reason {
                values.push(reason.clone());
            }
        }
        RunEventKind::ToolCallStarted { call } => {
            values.push(call.tool_name.to_string());
            values.push(call.arguments.to_string());
        }
        RunEventKind::ToolCallCompleted { call, output } => {
            values.push(call.tool_name.to_string());
            values.push(output.text_content());
            if let Some(metadata) = &output.metadata {
                values.push(metadata.to_string());
            }
        }
        RunEventKind::ToolCallFailed { call, error } => {
            values.push(call.tool_name.to_string());
            values.push(error.clone());
        }
        RunEventKind::TaskCreated {
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
        RunEventKind::TaskCompleted {
            task_id,
            agent_id,
            status,
        } => {
            values.push(task_id.clone());
            values.push(agent_id.to_string());
            values.push(status.to_string());
        }
        RunEventKind::SubagentStart { handle, task } => {
            values.push(handle.agent_id.to_string());
            values.push(handle.task_id.clone());
            values.push(handle.role.clone());
            values.push(task.prompt.clone());
            values.extend(task.requested_write_set.clone());
        }
        RunEventKind::AgentEnvelope { envelope } => match &envelope.kind {
            types::AgentEnvelopeKind::SpawnRequested { task }
            | types::AgentEnvelopeKind::Started { task } => {
                values.push(task.task_id.clone());
                values.push(task.role.clone());
            }
            types::AgentEnvelopeKind::StatusChanged { status } => {
                values.push(status.to_string());
            }
            types::AgentEnvelopeKind::Message { channel, payload } => {
                values.push(channel.clone());
                values.push(payload.to_string());
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
        RunEventKind::SubagentStop {
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
        RunEventKind::Notification { source, message } => {
            values.push(source.clone());
            values.push(message.clone());
        }
    }
    values.retain(|value| !value.trim().is_empty());
    values
}

#[must_use]
pub fn latest_token_usage_snapshot(events: &[RunEventEnvelope]) -> Option<TokenLedgerSnapshot> {
    events.iter().rev().find_map(|event| match &event.event {
        RunEventKind::TokenUsageUpdated { ledger, .. } => Some(ledger.clone()),
        _ => None,
    })
}

#[must_use]
pub fn session_token_usage_records(
    run_id: &RunId,
    events: &[RunEventEnvelope],
) -> Vec<TokenUsageRecord> {
    let mut by_session = BTreeMap::<SessionId, TokenLedgerSnapshot>::new();
    for event in events {
        if let RunEventKind::TokenUsageUpdated { ledger, .. } = &event.event {
            by_session.insert(event.session_id.clone(), ledger.clone());
        }
    }
    by_session
        .into_iter()
        .map(|(session_id, ledger)| TokenUsageRecord {
            scope: TokenUsageScope::Session,
            run_id: run_id.clone(),
            session_id: Some(session_id),
            agent_name: None,
            task_id: None,
            ledger,
        })
        .collect()
}

fn collect_child_run_token_usage_contexts(
    events: &[RunEventEnvelope],
) -> Vec<ResolvedChildRunTokenUsageContext> {
    let mut by_agent = BTreeMap::<String, ChildRunTokenUsageContext>::new();
    for event in events {
        match &event.event {
            RunEventKind::SubagentStart { handle, task } => {
                let context = by_agent.entry(handle.agent_id.to_string()).or_default();
                context.run_id = Some(handle.run_id.clone());
                context.session_id = Some(handle.session_id.clone());
                if context.agent_name.is_none() {
                    context.agent_name = Some(task.role.clone());
                }
                if context.task_id.is_none() {
                    context.task_id = Some(task.task_id.clone());
                }
            }
            RunEventKind::SubagentStop { handle, .. } => {
                let context = by_agent.entry(handle.agent_id.to_string()).or_default();
                context.run_id = Some(handle.run_id.clone());
                context.session_id = Some(handle.session_id.clone());
                if context.agent_name.is_none() {
                    context.agent_name = Some(handle.role.clone());
                }
                if context.task_id.is_none() {
                    context.task_id = Some(handle.task_id.clone());
                }
            }
            RunEventKind::AgentEnvelope { envelope } => {
                let context = by_agent.entry(envelope.agent_id.to_string()).or_default();
                if context.run_id.is_none() {
                    context.run_id = Some(envelope.run_id.clone());
                }
                if context.session_id.is_none() {
                    context.session_id = Some(envelope.session_id.clone());
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
            Some(ResolvedChildRunTokenUsageContext {
                run_id: context.run_id?,
                session_id: context.session_id,
                agent_name: context.agent_name,
                task_id: context.task_id,
            })
        })
        .collect::<Vec<_>>();
    contexts.sort_by(|left, right| {
        left.agent_name
            .cmp(&right.agent_name)
            .then_with(|| left.task_id.cmp(&right.task_id))
            .then_with(|| left.run_id.as_str().cmp(right.run_id.as_str()))
    });
    contexts
}

pub(crate) fn build_search_corpus(events: &[RunEventEnvelope]) -> String {
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
    events: &[RunEventEnvelope],
) -> GroupedMemoryExportEvents {
    let mut sessions = BTreeMap::<SessionId, Vec<RunEventEnvelope>>::new();
    let mut subagents = BTreeMap::<String, ScopedMemoryExportEvents>::new();
    let mut tasks = BTreeMap::<String, ScopedMemoryExportEvents>::new();
    let mut agent_contexts = BTreeMap::<String, AgentMemoryExportContext>::new();

    for event in events {
        sessions
            .entry(event.session_id.clone())
            .or_default()
            .push(event.clone());

        match &event.event {
            // Task lifecycle lives on the parent session stream. We keep those
            // records under task scope and later overwrite the fallback session
            // with the child session once spawn/start events provide it.
            RunEventKind::TaskCreated { task, .. } => {
                let group = tasks.entry(task.task_id.clone()).or_default();
                group.task_id = Some(task.task_id.clone());
                if group.session_id.is_none() {
                    group.session_id = Some(event.session_id.clone());
                }
                group.events.push(event.clone());
            }
            RunEventKind::TaskCompleted {
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
                    Some(&event.session_id),
                    event,
                );
                if !context.agent_name.is_none() || subagents.contains_key(&agent_id.to_string()) {
                    push_subagent_event(&mut subagents, agent_id.to_string(), &context, event);
                }
            }
            RunEventKind::SubagentStart { handle, task } => {
                let context = update_agent_memory_export_context(
                    &mut agent_contexts,
                    &handle.agent_id.to_string(),
                    Some(&handle.session_id),
                    Some(handle.role.as_str()),
                    Some(task.task_id.as_str()),
                );
                push_subagent_event(&mut subagents, handle.agent_id.to_string(), &context, event);
                push_task_event(
                    &mut tasks,
                    task.task_id.clone(),
                    Some(&context),
                    Some(&handle.session_id),
                    event,
                );
            }
            RunEventKind::AgentEnvelope { envelope } => {
                let agent_key = envelope.agent_id.to_string();
                let context = match &envelope.kind {
                    types::AgentEnvelopeKind::SpawnRequested { task }
                    | types::AgentEnvelopeKind::Started { task } => {
                        let context = update_agent_memory_export_context(
                            &mut agent_contexts,
                            &agent_key,
                            Some(&envelope.session_id),
                            Some(task.role.as_str()),
                            Some(task.task_id.as_str()),
                        );
                        push_task_event(
                            &mut tasks,
                            task.task_id.clone(),
                            Some(&context),
                            Some(&envelope.session_id),
                            event,
                        );
                        context
                    }
                    types::AgentEnvelopeKind::Result { result } => {
                        let context = update_agent_memory_export_context(
                            &mut agent_contexts,
                            &agent_key,
                            Some(&envelope.session_id),
                            None,
                            Some(result.task_id.as_str()),
                        );
                        push_task_event(
                            &mut tasks,
                            result.task_id.clone(),
                            Some(&context),
                            Some(&envelope.session_id),
                            event,
                        );
                        context
                    }
                    _ => {
                        let context = update_agent_memory_export_context(
                            &mut agent_contexts,
                            &agent_key,
                            Some(&envelope.session_id),
                            None,
                            None,
                        );
                        if let Some(task_id) = &context.task_id {
                            push_task_event(
                                &mut tasks,
                                task_id.clone(),
                                Some(&context),
                                Some(&envelope.session_id),
                                event,
                            );
                        }
                        context
                    }
                };
                push_subagent_event(&mut subagents, agent_key, &context, event);
            }
            RunEventKind::SubagentStop { handle, .. } => {
                let context = update_agent_memory_export_context(
                    &mut agent_contexts,
                    &handle.agent_id.to_string(),
                    Some(&handle.session_id),
                    Some(handle.role.as_str()),
                    Some(handle.task_id.as_str()),
                );
                push_subagent_event(&mut subagents, handle.agent_id.to_string(), &context, event);
                push_task_event(
                    &mut tasks,
                    handle.task_id.clone(),
                    Some(&context),
                    Some(&handle.session_id),
                    event,
                );
            }
            _ => {}
        }
    }

    GroupedMemoryExportEvents {
        sessions: sessions.into_iter().collect(),
        subagents: subagents.into_values().collect(),
        tasks: tasks.into_values().collect(),
    }
}

fn update_agent_memory_export_context(
    contexts: &mut BTreeMap<String, AgentMemoryExportContext>,
    agent_key: &str,
    session_id: Option<&SessionId>,
    agent_name: Option<&str>,
    task_id: Option<&str>,
) -> AgentMemoryExportContext {
    let context = contexts.entry(agent_key.to_string()).or_default();
    if let Some(session_id) = session_id {
        context.session_id = Some(session_id.clone());
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
    event: &RunEventEnvelope,
) {
    let group = groups.entry(agent_key).or_default();
    apply_memory_export_context(group, context, None);
    group.events.push(event.clone());
}

fn push_task_event(
    groups: &mut BTreeMap<String, ScopedMemoryExportEvents>,
    task_key: String,
    context: Option<&AgentMemoryExportContext>,
    fallback_session_id: Option<&SessionId>,
    event: &RunEventEnvelope,
) {
    let group = groups.entry(task_key.clone()).or_default();
    group.task_id = Some(task_key);
    if let Some(context) = context {
        apply_memory_export_context(group, context, fallback_session_id);
    } else if group.session_id.is_none() {
        group.session_id = fallback_session_id.cloned();
    }
    group.events.push(event.clone());
}

fn apply_memory_export_context(
    group: &mut ScopedMemoryExportEvents,
    context: &AgentMemoryExportContext,
    fallback_session_id: Option<&SessionId>,
) {
    if let Some(session_id) = &context.session_id {
        group.session_id = Some(session_id.clone());
    } else if group.session_id.is_none() {
        group.session_id = fallback_session_id.cloned();
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
    run_id: &RunId,
    session_id: Option<SessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
    events: &[RunEventEnvelope],
) -> Option<RunMemoryExportRecord> {
    if events.is_empty() {
        return None;
    }

    let mut first_timestamp_ms = u128::MAX;
    let mut last_timestamp_ms = 0;
    let mut last_user_prompt = None;

    for event in events {
        first_timestamp_ms = first_timestamp_ms.min(event.timestamp_ms);
        last_timestamp_ms = last_timestamp_ms.max(event.timestamp_ms);
        if let RunEventKind::UserPromptSubmit { prompt } = &event.event {
            last_user_prompt = Some(prompt.clone());
        }
    }
    let transcript_message_count = replay_transcript(events).len();

    Some(RunMemoryExportRecord {
        summary: MemoryExportSummary {
            scope,
            run_id: run_id.clone(),
            session_id,
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

pub(crate) fn sort_memory_export_records(records: &mut [RunMemoryExportRecord]) {
    records.sort_by(|left, right| {
        right
            .summary
            .last_timestamp_ms
            .cmp(&left.summary.last_timestamp_ms)
            .then_with(|| {
                left.summary
                    .run_id
                    .as_str()
                    .cmp(right.summary.run_id.as_str())
            })
            .then_with(|| left.summary.session_id.cmp(&right.summary.session_id))
            .then_with(|| left.summary.agent_name.cmp(&right.summary.agent_name))
            .then_with(|| left.summary.task_id.cmp(&right.summary.task_id))
    });
}

pub(crate) fn apply_memory_export_request(
    bundle: &mut RunMemoryExportBundle,
    request: &RunMemoryExportRequest,
) {
    if let Some(max_runs) = request.max_runs {
        bundle.runs.truncate(max_runs);
        bundle.sessions.truncate(max_runs);
        bundle.subagents.truncate(max_runs);
        bundle.tasks.truncate(max_runs);
    }
    if let Some(max_chars) = request.max_search_corpus_chars {
        for record in bundle
            .runs
            .iter_mut()
            .chain(bundle.sessions.iter_mut())
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

fn collect_memory_export_sections(events: &[RunEventEnvelope]) -> MemoryExportSections {
    let mut sections = MemoryExportSections::default();

    for event in events {
        match &event.event {
            RunEventKind::SteerApplied { message, reason } => {
                push_unique(&mut sections.decisions, preview_text(message, 120));
                if let Some(reason) = reason {
                    push_unique(&mut sections.decisions, preview_text(reason, 120));
                }
            }
            RunEventKind::CompactionCompleted { reason, .. } => {
                push_unique(
                    &mut sections.follow_up,
                    format!("compaction: {}", preview_text(reason, 120)),
                );
            }
            RunEventKind::HookCompleted {
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
            RunEventKind::ToolApprovalResolved {
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
            RunEventKind::ToolCallCompleted { call, output } => {
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
            RunEventKind::ToolCallFailed { call, error } => {
                push_unique(
                    &mut sections.tool_summary,
                    format!("{} failed", call.tool_name),
                );
                push_unique(
                    &mut sections.failures,
                    format!("{}: {}", call.tool_name, preview_text(error, 120)),
                );
            }
            RunEventKind::Notification { source, message } => {
                push_unique(
                    &mut sections.follow_up,
                    format!("{source}: {}", preview_text(message, 120)),
                );
            }
            RunEventKind::StopFailure { reason } => {
                if let Some(reason) = reason {
                    push_unique(&mut sections.failures, preview_text(reason, 120));
                }
            }
            RunEventKind::Stop { reason } | RunEventKind::SessionEnd { reason } => {
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
    async fn append(&self, event: RunEventEnvelope) -> Result<()>;

    async fn append_batch(&self, events: Vec<RunEventEnvelope>) -> Result<()> {
        for event in events {
            self.append(event).await?;
        }
        Ok(())
    }
}

#[async_trait]
pub trait RunStore: EventSink {
    async fn list_runs(&self) -> Result<Vec<RunSummary>>;
    async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>>;
    async fn events(&self, run_id: &RunId) -> Result<Vec<RunEventEnvelope>>;
    async fn session_ids(&self, run_id: &RunId) -> Result<Vec<SessionId>>;
    async fn replay_transcript(&self, run_id: &RunId) -> Result<Vec<Message>>;
    async fn token_usage(&self, run_id: &RunId) -> Result<RunTokenUsageReport> {
        let root_events = self.events(run_id).await?;
        let run = latest_token_usage_snapshot(&root_events).map(|ledger| TokenUsageRecord {
            scope: TokenUsageScope::Run,
            run_id: run_id.clone(),
            session_id: None,
            agent_name: None,
            task_id: None,
            ledger,
        });
        let sessions = session_token_usage_records(run_id, &root_events);
        let mut aggregate_usage = run
            .as_ref()
            .map(|record| record.ledger.cumulative_usage)
            .unwrap_or_default();

        let child_contexts = collect_child_run_token_usage_contexts(&root_events);
        let child_records = stream::iter(child_contexts.into_iter().map(|context| async move {
            let events = self.events(&context.run_id).await?;
            Ok::<_, RunStoreError>((context, latest_token_usage_snapshot(&events)))
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
                run_id: context.run_id.clone(),
                session_id: context.session_id.clone(),
                agent_name: context.agent_name.clone(),
                task_id: context.task_id.clone(),
                ledger: ledger.clone(),
            });
            tasks.push(TokenUsageRecord {
                scope: TokenUsageScope::Task,
                run_id: context.run_id,
                session_id: context.session_id,
                agent_name: context.agent_name,
                task_id: context.task_id,
                ledger,
            });
        }

        Ok(RunTokenUsageReport {
            run,
            sessions,
            subagents,
            tasks,
            aggregate_usage,
        })
    }
    async fn export_for_memory(
        &self,
        request: RunMemoryExportRequest,
    ) -> Result<RunMemoryExportBundle>;
}
