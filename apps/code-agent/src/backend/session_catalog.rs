use store::{RunSearchResult, RunSummary};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionResumeSupport {
    AttachedToActiveRuntime,
    NotYetSupported { reason: String },
}

impl SessionResumeSupport {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::AttachedToActiveRuntime => "attached",
            Self::NotYetSupported { .. } => "history-only",
        }
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::AttachedToActiveRuntime => None,
            Self::NotYetSupported { reason } => Some(reason),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedSessionSummary {
    pub(crate) session_ref: String,
    pub(crate) first_timestamp_ms: u128,
    pub(crate) last_timestamp_ms: u128,
    pub(crate) event_count: usize,
    pub(crate) worker_session_count: usize,
    pub(crate) transcript_message_count: usize,
    pub(crate) last_user_prompt: Option<String>,
    pub(crate) resume_support: SessionResumeSupport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedSessionSearchMatch {
    pub(crate) summary: PersistedSessionSummary,
    pub(crate) matched_event_count: usize,
    pub(crate) preview_matches: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionResumeStatus {
    pub(crate) session_ref: String,
    pub(crate) support: SessionResumeSupport,
}

const HISTORY_ONLY_REASON: &str =
    "Persisted sessions can be replayed and exported, but runtime reattach is not implemented yet.";

pub(crate) fn persisted_session_summary(
    summary: &RunSummary,
    active_session_ref: &str,
) -> PersistedSessionSummary {
    PersistedSessionSummary {
        session_ref: summary.run_id.to_string(),
        first_timestamp_ms: summary.first_timestamp_ms,
        last_timestamp_ms: summary.last_timestamp_ms,
        event_count: summary.event_count,
        worker_session_count: summary.session_count,
        transcript_message_count: summary.transcript_message_count,
        last_user_prompt: summary.last_user_prompt.clone(),
        resume_support: resume_support_for(summary.run_id.as_str(), active_session_ref),
    }
}

pub(crate) fn persisted_session_search_match(
    result: &RunSearchResult,
    active_session_ref: &str,
) -> PersistedSessionSearchMatch {
    PersistedSessionSearchMatch {
        summary: persisted_session_summary(&result.summary, active_session_ref),
        matched_event_count: result.matched_event_count,
        preview_matches: result.preview_matches.clone(),
    }
}

pub(crate) fn resume_status(session_ref: &str, active_session_ref: &str) -> SessionResumeStatus {
    SessionResumeStatus {
        session_ref: session_ref.to_string(),
        support: resume_support_for(session_ref, active_session_ref),
    }
}

fn resume_support_for(session_ref: &str, active_session_ref: &str) -> SessionResumeSupport {
    if session_ref == active_session_ref {
        SessionResumeSupport::AttachedToActiveRuntime
    } else {
        SessionResumeSupport::NotYetSupported {
            reason: HISTORY_ONLY_REASON.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionResumeSupport, persisted_session_summary, resume_status};
    use agent::types::RunId;
    use store::RunSummary;

    #[test]
    fn active_runtime_session_reports_attached_resume_support() {
        let summary = persisted_session_summary(
            &RunSummary {
                run_id: RunId::from("run_active"),
                first_timestamp_ms: 1,
                last_timestamp_ms: 2,
                event_count: 3,
                session_count: 1,
                transcript_message_count: 4,
                last_user_prompt: Some("inspect".to_string()),
            },
            "run_active",
        );

        assert_eq!(
            summary.resume_support,
            SessionResumeSupport::AttachedToActiveRuntime
        );
    }

    #[test]
    fn persisted_session_resume_status_is_explicitly_history_only() {
        let status = resume_status("run_old", "run_active");
        assert_eq!(status.support.label(), "history-only");
        assert!(
            status
                .support
                .reason()
                .expect("history-only sessions should explain why resume is unavailable")
                .contains("runtime reattach is not implemented yet")
        );
    }
}
