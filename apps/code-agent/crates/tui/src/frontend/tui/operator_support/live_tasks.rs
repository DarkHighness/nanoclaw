use super::*;

pub(crate) fn live_task_wait_notice_entry(outcome: &LiveTaskWaitOutcome) -> TranscriptEntry {
    let headline = format!("Background task {} finished", outcome.task_id);
    let details = live_task_wait_notice_details(outcome);
    match outcome.status {
        agent::types::TaskStatus::Completed => {
            TranscriptEntry::success_summary_details(headline, details)
        }
        agent::types::TaskStatus::Failed => {
            TranscriptEntry::error_summary_details(headline, details)
        }
        agent::types::TaskStatus::Cancelled => {
            TranscriptEntry::warning_summary_details(headline, details)
        }
        _ => TranscriptEntry::shell_summary_details(headline, details),
    }
}

pub(crate) fn live_task_wait_notice_details(
    outcome: &LiveTaskWaitOutcome,
) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("status {}", outcome.status),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("summary {}", state::preview_text(&outcome.summary, 96)),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: "next enter steer / tab queue / /task inspect".to_string(),
            continuation: false,
        },
    ];
    if !outcome.claimed_files.is_empty() {
        details.push(TranscriptShellDetail::Raw {
            text: format!(
                "claimed files {}",
                state::preview_text(&outcome.claimed_files.join(", "), 96)
            ),
            continuation: false,
        });
    }
    if !outcome.remaining_live_tasks.is_empty() {
        details.push(TranscriptShellDetail::Raw {
            text: format!(
                "still running {}",
                state::preview_text(
                    &outcome
                        .remaining_live_tasks
                        .iter()
                        .map(|task| format!("{} ({}, {})", task.task_id, task.role, task.status))
                        .collect::<Vec<_>>()
                        .join(", "),
                    96
                )
            ),
            continuation: false,
        });
    }
    details
}

pub(crate) fn live_task_wait_ui_toast_tone(outcome: &LiveTaskWaitOutcome) -> ToastTone {
    match outcome.status {
        agent::types::TaskStatus::Completed => ToastTone::Success,
        agent::types::TaskStatus::Failed => ToastTone::Error,
        agent::types::TaskStatus::Cancelled => ToastTone::Warning,
        _ => ToastTone::Info,
    }
}

pub(crate) fn live_task_wait_toast_message(
    outcome: &LiveTaskWaitOutcome,
    turn_running: bool,
) -> String {
    let next_step = if turn_running {
        "enter steer / tab queue / /task inspect"
    } else {
        "model follow-up queued / /task inspect"
    };
    let mut parts = vec![
        format!("task {} {}", outcome.task_id, outcome.status),
        state::preview_text(&outcome.summary, 64),
    ];
    if !outcome.remaining_live_tasks.is_empty() {
        parts.push(format!(
            "{} still running",
            outcome.remaining_live_tasks.len()
        ));
    }
    parts.push(next_step.to_string());
    parts.join(" · ")
}
