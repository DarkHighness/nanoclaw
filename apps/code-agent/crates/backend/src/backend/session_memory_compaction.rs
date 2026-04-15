use crate::backend::session_memory_note::{
    default_session_memory_note, parse_session_memory_note_snapshot,
    truncate_session_memory_for_compaction,
};
use agent::runtime::{CompactionRequest, CompactionResult, ConversationCompactor, Result};
use agent::types::{MessageId, SessionId};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::time::sleep;
use tracing::{info, warn};

pub const SESSION_MEMORY_WAIT_TIMEOUT_MS: u64 = 15_000;
pub const SESSION_MEMORY_STALE_THRESHOLD_MS: u64 = 60_000;
const SESSION_MEMORY_WAIT_POLL_MS: u64 = 250;

#[derive(Clone, Debug, Default)]
pub struct SessionMemoryRefreshState {
    pub active_session_id: Option<SessionId>,
    pub initialized: bool,
    pub refresh_in_flight: bool,
    pub refresh_started_at: Option<Instant>,
    pub refresh_epoch: u64,
    pub last_summarized_message_id: Option<MessageId>,
}

pub type SharedSessionMemoryRefreshState = Arc<Mutex<SessionMemoryRefreshState>>;

pub fn session_memory_note_absolute_path(workspace_root: &Path, session_id: &SessionId) -> PathBuf {
    workspace_root.join(format!(
        ".nanoclaw/memory/working/sessions/{}.md",
        session_id.as_str()
    ))
}

pub struct SessionMemoryConversationCompactor {
    workspace_root: PathBuf,
    refresh_state: SharedSessionMemoryRefreshState,
    fallback: Arc<dyn ConversationCompactor>,
}

impl SessionMemoryConversationCompactor {
    #[must_use]
    pub fn new(
        workspace_root: PathBuf,
        refresh_state: SharedSessionMemoryRefreshState,
        fallback: Arc<dyn ConversationCompactor>,
    ) -> Self {
        Self {
            workspace_root,
            refresh_state,
            fallback,
        }
    }

    async fn try_compact_with_session_note(
        &self,
        request: &CompactionRequest,
    ) -> Option<CompactionResult> {
        // Manual `/compact` notes are explicit operator guidance, so keep the
        // model summary path when extra compaction instructions are present
        // instead of silently discarding them in favor of session memory.
        if request.instructions.is_some() {
            return None;
        }
        let source_last_message_id = request
            .messages
            .last()
            .map(|message| message.message_id.clone())?;
        self.wait_for_session_memory_refresh(&request.session_id)
            .await;

        let state = self.refresh_state.lock().unwrap().clone();
        if state.active_session_id.as_ref() != Some(&request.session_id) || !state.initialized {
            return None;
        }
        let note = self.load_session_memory_note(&request.session_id).await?;
        let last_summarized_message_id = note
            .last_summarized_message_id
            .or(state.last_summarized_message_id)?;
        let visible_source_end_index =
            find_message_index(&request.visible_messages, &source_last_message_id)?;
        let summarized_through_index =
            find_message_index(&request.visible_messages, &last_summarized_message_id)?;
        if summarized_through_index < visible_source_end_index {
            return None;
        }
        info!(
            session_id = %request.session_id,
            source_message_count = request.messages.len(),
            retained_message_count = request
                .visible_messages
                .len()
                .saturating_sub(request.messages.len()),
            "using structured session memory note for compaction continuity"
        );
        Some(CompactionResult {
            summary: self.render_compaction_summary(&request.session_id, &note.body),
        })
    }

    async fn wait_for_session_memory_refresh(&self, session_id: &SessionId) {
        let wait_started_at = Instant::now();
        loop {
            let should_wait = {
                let state = self.refresh_state.lock().unwrap();
                state.active_session_id.as_ref() == Some(session_id)
                    && state.refresh_in_flight
                    && state.refresh_started_at.is_none_or(|started_at| {
                        started_at.elapsed()
                            < Duration::from_millis(SESSION_MEMORY_STALE_THRESHOLD_MS)
                    })
                    && wait_started_at.elapsed()
                        < Duration::from_millis(SESSION_MEMORY_WAIT_TIMEOUT_MS)
            };
            if !should_wait {
                return;
            }
            sleep(Duration::from_millis(SESSION_MEMORY_WAIT_POLL_MS)).await;
        }
    }

    async fn load_session_memory_note(
        &self,
        session_id: &SessionId,
    ) -> Option<crate::backend::session_memory_note::SessionMemoryNoteSnapshot> {
        let path = session_memory_note_absolute_path(&self.workspace_root, session_id);
        let text = match fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                warn!(
                    session_id = %session_id,
                    path = %path.display(),
                    error = %error,
                    "failed to load session memory note for compaction continuity"
                );
                return None;
            }
        };
        let snapshot = parse_session_memory_note_snapshot(&text);
        if snapshot.body.is_empty() || snapshot.body == default_session_memory_note().trim() {
            return None;
        }
        Some(snapshot)
    }

    fn render_compaction_summary(&self, session_id: &SessionId, note_body: &str) -> String {
        let truncated = truncate_session_memory_for_compaction(note_body);
        if !truncated.was_truncated {
            return truncated.truncated_content;
        }

        format!(
            "{}\n\nSome session memory sections were truncated for length. The full session memory can be viewed at: {}",
            truncated.truncated_content,
            session_memory_note_absolute_path(&self.workspace_root, session_id).display(),
        )
    }
}

#[async_trait]
impl ConversationCompactor for SessionMemoryConversationCompactor {
    async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult> {
        if let Some(result) = self.try_compact_with_session_note(&request).await {
            return Ok(result);
        }
        self.fallback.compact(request).await
    }
}

fn find_message_index(messages: &[agent::types::Message], message_id: &MessageId) -> Option<usize> {
    messages
        .iter()
        .position(|message| message.message_id == *message_id)
}

#[cfg(test)]
mod tests {
    use super::{
        SESSION_MEMORY_STALE_THRESHOLD_MS, SESSION_MEMORY_WAIT_TIMEOUT_MS,
        SessionMemoryConversationCompactor, SessionMemoryRefreshState,
        SharedSessionMemoryRefreshState, session_memory_note_absolute_path,
    };
    use agent::runtime::{CompactionRequest, CompactionResult, ConversationCompactor};
    use agent::types::{AgentSessionId, Message, SessionId, TurnId};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Clone, Default)]
    struct RecordingFallbackCompactor {
        calls: Arc<Mutex<Vec<CompactionRequest>>>,
    }

    impl RecordingFallbackCompactor {
        fn calls(&self) -> Vec<CompactionRequest> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ConversationCompactor for RecordingFallbackCompactor {
        async fn compact(
            &self,
            request: CompactionRequest,
        ) -> agent::runtime::Result<CompactionResult> {
            self.calls.lock().unwrap().push(request);
            Ok(CompactionResult {
                summary: "fallback summary".to_string(),
            })
        }
    }

    fn shared_refresh_state(
        session_id: &SessionId,
        last_summarized: &agent::types::MessageId,
    ) -> SharedSessionMemoryRefreshState {
        Arc::new(Mutex::new(SessionMemoryRefreshState {
            active_session_id: Some(session_id.clone()),
            initialized: true,
            refresh_in_flight: false,
            refresh_started_at: None,
            refresh_epoch: 0,
            last_summarized_message_id: Some(last_summarized.clone()),
        }))
    }

    #[tokio::test]
    async fn uses_session_note_when_refresh_state_covers_compaction_source() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-session-1");
        let first = Message::user("first");
        let second = Message::assistant("second");
        let tail = Message::user("tail");
        std::fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions")).unwrap();
        std::fs::write(
            session_memory_note_absolute_path(dir.path(), &session_id),
            "---\nscope: working\n---\n\n# Current State\n\nUse the session note.",
        )
        .unwrap();
        let refresh_state = shared_refresh_state(&session_id, &tail.message_id);
        let fallback = Arc::new(RecordingFallbackCompactor::default());
        let compactor = SessionMemoryConversationCompactor::new(
            dir.path().to_path_buf(),
            refresh_state,
            fallback.clone(),
        );

        let result = compactor
            .compact(CompactionRequest {
                session_id,
                agent_session_id,
                turn_id: TurnId::new(),
                messages: vec![first.clone(), second.clone()],
                visible_messages: vec![first, second, tail],
                instructions: None,
            })
            .await
            .unwrap();

        assert!(result.summary.contains("Use the session note."));
        assert!(fallback.calls().is_empty());
    }

    #[tokio::test]
    async fn falls_back_when_session_note_boundary_is_behind_source_window() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-session-1");
        let first = Message::user("first");
        let second = Message::assistant("second");
        let third = Message::user("third");
        std::fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions")).unwrap();
        std::fs::write(
            session_memory_note_absolute_path(dir.path(), &session_id),
            "---\nscope: working\n---\n\n# Current State\n\nStale session note.",
        )
        .unwrap();
        let refresh_state = shared_refresh_state(&session_id, &first.message_id);
        let fallback = Arc::new(RecordingFallbackCompactor::default());
        let compactor = SessionMemoryConversationCompactor::new(
            dir.path().to_path_buf(),
            refresh_state,
            fallback.clone(),
        );

        let result = compactor
            .compact(CompactionRequest {
                session_id,
                agent_session_id,
                turn_id: TurnId::new(),
                messages: vec![first.clone(), second.clone()],
                visible_messages: vec![first, second, third],
                instructions: None,
            })
            .await
            .unwrap();

        assert_eq!(result.summary, "fallback summary");
        assert_eq!(fallback.calls().len(), 1);
    }

    #[tokio::test]
    async fn uses_persisted_note_boundary_when_runtime_state_was_lost() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-session-1");
        let first = Message::user("first");
        let second = Message::assistant("second");
        let tail = Message::user("tail");
        std::fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions")).unwrap();
        std::fs::write(
            session_memory_note_absolute_path(dir.path(), &session_id),
            "---\nscope: working\nlast_summarized_message_id: msg_tail\n---\n\n# Current State\n\nUse the persisted boundary.",
        )
        .unwrap();
        let refresh_state = Arc::new(Mutex::new(SessionMemoryRefreshState {
            active_session_id: Some(session_id.clone()),
            initialized: true,
            refresh_in_flight: false,
            refresh_started_at: None,
            refresh_epoch: 0,
            last_summarized_message_id: None,
        }));
        let fallback = Arc::new(RecordingFallbackCompactor::default());
        let compactor = SessionMemoryConversationCompactor::new(
            dir.path().to_path_buf(),
            refresh_state,
            fallback.clone(),
        );

        let result = compactor
            .compact(CompactionRequest {
                session_id,
                agent_session_id,
                turn_id: TurnId::new(),
                messages: vec![first.clone(), second.clone()],
                visible_messages: vec![first, second, tail.with_message_id("msg_tail")],
                instructions: None,
            })
            .await
            .unwrap();

        assert!(result.summary.contains("Use the persisted boundary."));
        assert!(fallback.calls().is_empty());
    }

    #[tokio::test]
    async fn waits_briefly_for_in_flight_refresh_before_using_session_note() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-session-1");
        let first = Message::user("first");
        let second = Message::assistant("second");
        std::fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions")).unwrap();
        std::fs::write(
            session_memory_note_absolute_path(dir.path(), &session_id),
            "---\nscope: working\n---\n\n# Current State\n\nFreshened session note.",
        )
        .unwrap();
        let refresh_state = Arc::new(Mutex::new(SessionMemoryRefreshState {
            active_session_id: Some(session_id.clone()),
            initialized: true,
            refresh_in_flight: true,
            refresh_started_at: Some(
                Instant::now()
                    - Duration::from_millis(
                        SESSION_MEMORY_STALE_THRESHOLD_MS
                            .min(SESSION_MEMORY_WAIT_TIMEOUT_MS)
                            .saturating_sub(500),
                    ),
            ),
            refresh_epoch: 0,
            last_summarized_message_id: Some(second.message_id.clone()),
        }));
        let refresh_state_writer = refresh_state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            refresh_state_writer.lock().unwrap().refresh_in_flight = false;
        });
        let fallback = Arc::new(RecordingFallbackCompactor::default());
        let compactor = SessionMemoryConversationCompactor::new(
            dir.path().to_path_buf(),
            refresh_state,
            fallback.clone(),
        );

        let result = compactor
            .compact(CompactionRequest {
                session_id,
                agent_session_id,
                turn_id: TurnId::new(),
                messages: vec![first.clone(), second.clone()],
                visible_messages: vec![first, second],
                instructions: None,
            })
            .await
            .unwrap();

        assert!(result.summary.contains("Freshened session note."));
        assert!(fallback.calls().is_empty());
    }

    #[tokio::test]
    async fn truncates_oversized_session_note_sections_for_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let session_id = SessionId::from("session-1");
        let agent_session_id = AgentSessionId::from("agent-session-1");
        let first = Message::user("first");
        let second = Message::assistant("second");
        let tail = Message::user("tail");
        let oversized = "line\n".repeat(9_000);
        std::fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions")).unwrap();
        std::fs::write(
            session_memory_note_absolute_path(dir.path(), &session_id),
            format!(
                "---\nscope: working\nlast_summarized_message_id: {}\n---\n\n# Current State\n_Current State_\n\n{oversized}",
                tail.message_id
            ),
        )
        .unwrap();
        let refresh_state = shared_refresh_state(&session_id, &tail.message_id);
        let fallback = Arc::new(RecordingFallbackCompactor::default());
        let compactor = SessionMemoryConversationCompactor::new(
            dir.path().to_path_buf(),
            refresh_state,
            fallback.clone(),
        );

        let result = compactor
            .compact(CompactionRequest {
                session_id: session_id.clone(),
                agent_session_id,
                turn_id: TurnId::new(),
                messages: vec![first.clone(), second.clone()],
                visible_messages: vec![first, second, tail],
                instructions: None,
            })
            .await
            .unwrap();

        assert!(
            result
                .summary
                .contains("[... section truncated for length ...]")
        );
        assert!(
            result.summary.contains(
                session_memory_note_absolute_path(dir.path(), &session_id)
                    .display()
                    .to_string()
                    .as_str()
            )
        );
        assert!(fallback.calls().is_empty());
    }
}
