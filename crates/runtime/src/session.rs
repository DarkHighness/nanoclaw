use std::collections::HashSet;
use types::{AgentSessionId, Message, MessageId, ProviderContinuation, RunId, TokenLedgerSnapshot};

#[derive(Clone, Debug)]
pub struct RuntimeSession {
    pub run_id: RunId,
    pub agent_session_id: AgentSessionId,
    pub transcript: Vec<Message>,
    pub provider_continuation: Option<ProviderContinuation>,
    pub provider_transcript_cursor: usize,
    pub compaction_summary_index: Option<usize>,
    pub retained_tail_indices: Vec<usize>,
    pub post_summary_start: usize,
    pub removed_message_ids: HashSet<MessageId>,
    pub agent_session_started: bool,
    pub token_ledger: TokenLedgerSnapshot,
}

impl Default for RuntimeSession {
    fn default() -> Self {
        Self::new(RunId::new(), AgentSessionId::new())
    }
}

impl RuntimeSession {
    #[must_use]
    pub fn new(run_id: RunId, agent_session_id: AgentSessionId) -> Self {
        Self {
            run_id,
            agent_session_id,
            transcript: Vec::new(),
            provider_continuation: None,
            provider_transcript_cursor: 0,
            compaction_summary_index: None,
            retained_tail_indices: Vec::new(),
            post_summary_start: 0,
            removed_message_ids: HashSet::new(),
            agent_session_started: false,
            token_ledger: TokenLedgerSnapshot::default(),
        }
    }
}
