use super::*;

pub(crate) fn format_session_summary_line(summary: &PersistedSessionSummary) -> TranscriptEntry {
    let title_or_prompt = summary
        .session_title
        .as_deref()
        .or(summary.last_user_prompt.as_deref())
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    info_summary_entry(
        format!("{}  {}", preview_id(&summary.session_ref), title_or_prompt),
        [format!(
            "{} messages · {} events · {} agent sessions{} · resume {}",
            summary.transcript_message_count,
            summary.event_count,
            summary.worker_session_count,
            format_summary_token_usage(summary.token_usage.as_ref()),
            summary.resume_support.label()
        )],
    )
}

pub(crate) fn format_agent_session_summary_line(
    summary: &PersistedAgentSessionSummary,
) -> TranscriptEntry {
    let context = summary
        .session_title
        .as_deref()
        .map(|value| format!("title {}", preview_text(value, 36)))
        .or_else(|| {
            summary
                .last_user_prompt
                .as_deref()
                .map(|value| format!("prompt {}", preview_text(value, 36)))
        })
        .unwrap_or_else(|| "no prompt yet".to_string());
    info_summary_entry(
        format!(
            "{}  {}",
            preview_id(&summary.agent_session_ref),
            summary.label
        ),
        [format!(
            "session {} · {} messages · {} events · resume {} · {}",
            preview_id(&summary.session_ref),
            summary.transcript_message_count,
            summary.event_count,
            summary.resume_support.label(),
            context
        )],
    )
}

pub(crate) fn format_task_summary_line(summary: &PersistedTaskSummary) -> TranscriptEntry {
    info_summary_entry(
        format!("{}  {}", summary.task_id, summary.status),
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

pub(crate) fn format_live_task_summary_line(summary: &LiveTaskSummary) -> TranscriptEntry {
    info_summary_entry(
        format!("{}  {}", summary.task_id, summary.status),
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
    vec![InspectorEntry::transcript(info_summary_entry(
        format!("Spawned task {}", outcome.task.task_id),
        [
            format!("role {}", outcome.task.role),
            format!("status {}", outcome.task.status),
            format!("agent {}", outcome.task.agent_id),
            format!("session {}", outcome.task.session_ref),
            format!("agent session {}", outcome.task.agent_session_ref),
            format!("prompt {}", preview_text(&outcome.prompt, 96)),
        ],
    ))]
}

pub(crate) fn format_session_search_line(result: &PersistedSessionSearchMatch) -> TranscriptEntry {
    let title_or_prompt = result
        .summary
        .session_title
        .as_deref()
        .or(result.summary.last_user_prompt.as_deref())
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    info_summary_entry(
        format!(
            "{}  {}",
            preview_id(&result.summary.session_ref),
            title_or_prompt
        ),
        [format!(
            "{} messages · {} events · {} agent sessions{} · resume {} · matched {} event(s){}",
            result.summary.transcript_message_count,
            result.summary.event_count,
            result.summary.worker_session_count,
            format_summary_token_usage(result.summary.token_usage.as_ref()),
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

fn format_summary_token_usage(token_usage: Option<&SessionSummaryTokenUsage>) -> String {
    token_usage
        .filter(|token_usage| !token_usage.is_zero())
        .map(|token_usage| {
            format!(
                " · tokens in={} out={} cache={}",
                token_usage.cumulative_usage.input_tokens,
                token_usage.cumulative_usage.output_tokens,
                token_usage.cumulative_usage.cache_read_tokens,
            )
        })
        .unwrap_or_default()
}
