use agent_core_types::{Message, ProviderContinuation, RunId, SessionId};

#[derive(Clone, Debug)]
pub struct RuntimeSession {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub transcript: Vec<Message>,
    pub provider_continuation: Option<ProviderContinuation>,
    pub provider_transcript_cursor: usize,
    pub compaction_summary_index: Option<usize>,
    pub retained_tail_indices: Vec<usize>,
    pub post_summary_start: usize,
    pub session_started: bool,
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self {
            run_id: RunId::new(),
            session_id: SessionId::new(),
            transcript: Vec::new(),
            provider_continuation: None,
            provider_transcript_cursor: 0,
            compaction_summary_index: None,
            retained_tail_indices: Vec::new(),
            post_summary_start: 0,
            session_started: false,
        }
    }
}
