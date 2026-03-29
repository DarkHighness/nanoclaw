use std::collections::HashSet;
use types::{
    AgentSessionId, Message, MessageId, ProviderContinuation, SessionId, TokenLedgerSnapshot,
};

#[derive(Clone, Debug)]
pub struct RuntimeSession {
    pub session_id: SessionId,
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
        Self::new(SessionId::new(), AgentSessionId::new())
    }
}

impl RuntimeSession {
    #[must_use]
    pub fn new(session_id: SessionId, agent_session_id: AgentSessionId) -> Self {
        Self {
            session_id,
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

    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.agent_session_started
            || !self.transcript.is_empty()
            || self.provider_continuation.is_some()
            || self.provider_transcript_cursor > 0
            || self.compaction_summary_index.is_some()
            || !self.retained_tail_indices.is_empty()
            || self.post_summary_start > 0
            || !self.removed_message_ids.is_empty()
            || self.token_ledger != TokenLedgerSnapshot::default()
    }
}
