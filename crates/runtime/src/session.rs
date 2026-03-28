use std::collections::HashSet;
use types::{Message, MessageId, ProviderContinuation, RunId, SessionId, TokenLedgerSnapshot};

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
    pub removed_message_ids: HashSet<MessageId>,
    pub session_started: bool,
    pub token_ledger: TokenLedgerSnapshot,
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self::new(RunId::new(), SessionId::new())
    }
}

impl RuntimeSession {
    #[must_use]
    pub fn new(run_id: RunId, session_id: SessionId) -> Self {
        Self {
            run_id,
            session_id,
            transcript: Vec::new(),
            provider_continuation: None,
            provider_transcript_cursor: 0,
            compaction_summary_index: None,
            retained_tail_indices: Vec::new(),
            post_summary_start: 0,
            removed_message_ids: HashSet::new(),
            session_started: false,
            token_ledger: TokenLedgerSnapshot::default(),
        }
    }
}
