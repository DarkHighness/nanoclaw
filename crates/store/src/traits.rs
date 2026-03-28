use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;
use types::{HookEffect, Message, RunEventEnvelope, RunEventKind, RunId, SessionId};

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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMemoryExportRequest {
    #[serde(default)]
    pub max_runs: Option<usize>,
    #[serde(default)]
    pub max_search_corpus_chars: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMemoryExportRecord {
    pub summary: RunSummary,
    pub session_ids: Vec<SessionId>,
    pub search_corpus: String,
}

#[must_use]
pub fn summarize_run_events(run_id: &RunId, events: &[RunEventEnvelope]) -> Option<RunSummary> {
    if events.is_empty() {
        return None;
    }

    let mut first_timestamp_ms = u128::MAX;
    let mut last_timestamp_ms = 0;
    let mut session_ids = HashSet::new();
    let mut transcript_message_count = 0;
    let mut last_user_prompt = None;

    for event in events {
        first_timestamp_ms = first_timestamp_ms.min(event.timestamp_ms);
        last_timestamp_ms = last_timestamp_ms.max(event.timestamp_ms);
        session_ids.insert(event.session_id.clone());
        if matches!(&event.event, RunEventKind::TranscriptMessage { .. }) {
            transcript_message_count += 1;
        }
        if let RunEventKind::UserPromptSubmit { prompt } = &event.event {
            last_user_prompt = Some(prompt.clone());
        }
    }

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
        RunEventKind::TranscriptMessage { message } => {
            values.push(message.text_content());
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
        RunEventKind::Notification { source, message } => {
            values.push(source.clone());
            values.push(message.clone());
        }
    }
    values.retain(|value| !value.trim().is_empty());
    values
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
}

#[async_trait]
pub trait RunStore: EventSink {
    async fn list_runs(&self) -> Result<Vec<RunSummary>>;
    async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>>;
    async fn events(&self, run_id: &RunId) -> Result<Vec<RunEventEnvelope>>;
    async fn session_ids(&self, run_id: &RunId) -> Result<Vec<SessionId>>;
    async fn replay_transcript(&self, run_id: &RunId) -> Result<Vec<Message>>;
    async fn export_for_memory(
        &self,
        request: RunMemoryExportRequest,
    ) -> Result<Vec<RunMemoryExportRecord>>;
}
