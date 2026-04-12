use super::*;
use crate::frontend::tui::state::{InspectorAction, InspectorEntry, InspectorKeyAction};
use crate::ui::ResumeSupport;

#[cfg(test)]
pub(crate) fn format_session_summary_line(summary: &PersistedSessionSummary) -> TranscriptEntry {
    info_summary_entry(
        session_summary_primary(summary),
        [session_summary_secondary(summary)],
    )
}

pub(crate) fn format_session_summary_collection(
    summary: &PersistedSessionSummary,
) -> InspectorEntry {
    InspectorEntry::actionable_collection(
        session_summary_primary(summary),
        Some(session_summary_secondary(summary)),
        InspectorAction::RunCommand(format!("/session {}", summary.session_ref)),
    )
}

#[cfg(test)]
pub(crate) fn format_agent_session_summary_line(
    summary: &PersistedAgentSessionSummary,
) -> TranscriptEntry {
    info_summary_entry(
        agent_session_summary_primary(summary),
        [agent_session_summary_secondary(summary)],
    )
}

pub(crate) fn format_agent_session_summary_collection(
    summary: &PersistedAgentSessionSummary,
) -> InspectorEntry {
    let primary = agent_session_summary_primary(summary);
    let secondary = agent_session_summary_secondary(summary);
    let action =
        InspectorAction::RunCommand(format!("/agent_session {}", summary.agent_session_ref));
    if let Some(alternate_action) = agent_session_resume_action(summary) {
        InspectorEntry::actionable_collection_with_alt(
            primary,
            Some(secondary),
            action,
            alternate_action,
        )
    } else {
        InspectorEntry::actionable_collection(primary, Some(secondary), action)
    }
}

pub(crate) fn format_task_summary_collection(summary: &PersistedTaskSummary) -> InspectorEntry {
    let mut secondary = format!(
        "role {} · session {} · {}",
        summary.role,
        preview_id(&summary.session_ref),
        preview_text(&summary.summary, 72),
    );
    if let Some(worktree_id) = summary.worktree_id.as_ref() {
        secondary.push_str(&format!(" · worktree {}", worktree_id));
    }
    if let Some(worktree_root) = summary.worktree_root.as_ref() {
        secondary.push_str(&format!(
            " · {}",
            preview_text(&worktree_root.display().to_string(), 48)
        ));
    }
    InspectorEntry::actionable_collection(
        format!("{}  {}", summary.task_id, summary.status),
        Some(secondary),
        InspectorAction::RunCommand(format!("/task {}", summary.task_id)),
    )
}

pub(crate) fn format_live_task_summary_collection(summary: &LiveTaskSummary) -> InspectorEntry {
    let mut secondary = format!(
        "role {} · agent {} · session {}",
        summary.role,
        preview_id(&summary.agent_id),
        preview_id(&summary.session_ref)
    );
    if let Some(worktree_id) = summary.worktree_id.as_ref() {
        secondary.push_str(&format!(" · worktree {}", worktree_id));
    }
    if let Some(worktree_root) = summary.worktree_root.as_ref() {
        secondary.push_str(&format!(
            " · {}",
            preview_text(&worktree_root.display().to_string(), 48)
        ));
    }
    InspectorEntry::actionable_collection_with_alt(
        format!("{}  {}", summary.task_id, summary.status),
        Some(secondary),
        InspectorAction::WaitLiveTask {
            task_or_agent_ref: summary.task_id.to_string(),
        },
        InspectorKeyAction {
            key_hint: "c".to_string(),
            label: "cancel".to_string(),
            action: InspectorAction::CancelLiveTask {
                task_or_agent_ref: summary.task_id.to_string(),
            },
        },
    )
}

pub(crate) fn format_live_monitor_summary_line(summary: &LiveMonitorSummary) -> TranscriptEntry {
    info_summary_entry(
        format!("{}  {}", summary.monitor_id, summary.status),
        [
            format!("cwd {} · shell {}", summary.cwd, summary.shell),
            format!("command {}", preview_text(&summary.command, 96)),
        ],
    )
}

#[cfg(test)]
pub(crate) fn format_session_search_line(result: &PersistedSessionSearchMatch) -> TranscriptEntry {
    info_summary_entry(
        session_search_primary(result),
        [session_search_secondary(result)],
    )
}

pub(crate) fn format_session_search_collection(
    result: &PersistedSessionSearchMatch,
) -> InspectorEntry {
    InspectorEntry::actionable_collection(
        session_search_primary(result),
        Some(session_search_secondary(result)),
        InspectorAction::RunCommand(format!("/session {}", result.summary.session_ref)),
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

fn session_summary_primary(summary: &PersistedSessionSummary) -> String {
    let title_or_prompt = summary
        .session_title
        .as_deref()
        .or(summary.last_user_prompt.as_deref())
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!("{}  {}", preview_id(&summary.session_ref), title_or_prompt)
}

fn session_summary_secondary(summary: &PersistedSessionSummary) -> String {
    format!(
        "{} messages · {} events · {} agent sessions{} · resume {}",
        summary.transcript_message_count,
        summary.event_count,
        summary.worker_session_count,
        format_summary_token_usage(summary.token_usage.as_ref()),
        summary.resume_support.label()
    )
}

fn agent_session_summary_primary(summary: &PersistedAgentSessionSummary) -> String {
    format!(
        "{}  {}",
        preview_id(&summary.agent_session_ref),
        summary.label
    )
}

fn agent_session_summary_secondary(summary: &PersistedAgentSessionSummary) -> String {
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
    format!(
        "session {} · {} messages · {} events · resume {} · {}",
        preview_id(&summary.session_ref),
        summary.transcript_message_count,
        summary.event_count,
        summary.resume_support.label(),
        context
    )
}

fn agent_session_resume_action(
    summary: &PersistedAgentSessionSummary,
) -> Option<InspectorKeyAction> {
    match summary.resume_support {
        ResumeSupport::AttachedToActiveRuntime | ResumeSupport::Reattachable => {
            Some(InspectorKeyAction {
                key_hint: "r".to_string(),
                label: "resume".to_string(),
                action: InspectorAction::RunCommand(format!(
                    "/resume {}",
                    summary.agent_session_ref
                )),
            })
        }
        ResumeSupport::NotYetSupported { .. } => None,
    }
}

fn session_search_primary(result: &PersistedSessionSearchMatch) -> String {
    let title_or_prompt = result
        .summary
        .session_title
        .as_deref()
        .or(result.summary.last_user_prompt.as_deref())
        .as_deref()
        .map(|value| preview_text(value, 36))
        .unwrap_or_else(|| "no prompt yet".to_string());
    format!(
        "{}  {}",
        preview_id(&result.summary.session_ref),
        title_or_prompt
    )
}

fn session_search_secondary(result: &PersistedSessionSearchMatch) -> String {
    format!(
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
    )
}
