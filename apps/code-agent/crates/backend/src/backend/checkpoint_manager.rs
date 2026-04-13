use agent::tools::fs::{commit_text_file, load_optional_text_file};
use agent::tools::{
    CheckpointFileMutation, CheckpointHandler, CheckpointMutationRequest, Result as ToolResult,
    ToolError, ToolExecutionContext,
};
use agent::types::{
    AgentSessionId, CheckpointFileRecord, CheckpointId, CheckpointOrigin, CheckpointRecord,
    CheckpointRestoreMode, CheckpointRestoreRecord, CheckpointScope, MessageId, MessageRole,
    SessionEventEnvelope, SessionEventKind, SessionId, ToolName, TurnId, new_opaque_id,
};
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use store::{SessionStore, SessionStoreError, visible_transcript};

#[derive(Clone)]
pub struct SessionCheckpointManager {
    store: Arc<dyn SessionStore>,
}

#[derive(Clone, Debug)]
struct CheckpointTimelineEntry {
    checkpoint: CheckpointRecord,
}

impl SessionCheckpointManager {
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }

    fn require_attached_runtime(
        ctx: &ToolExecutionContext,
    ) -> ToolResult<(SessionId, AgentSessionId)> {
        let session_id = ctx.session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("checkpoint tools require an attached runtime session")
        })?;
        let agent_session_id = ctx.agent_session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("checkpoint tools require an attached runtime agent session")
        })?;
        Ok((session_id, agent_session_id))
    }

    async fn session_events(
        &self,
        session_id: &SessionId,
    ) -> ToolResult<Vec<SessionEventEnvelope>> {
        self.store
            .events(session_id)
            .await
            .or_else(|error| match error {
                SessionStoreError::SessionNotFound(_) => Ok(Vec::new()),
                other => Err(other),
            })
            .map_err(|error| ToolError::invalid_state(error.to_string()))
    }

    async fn load_timeline(
        &self,
        session_id: &SessionId,
    ) -> ToolResult<Vec<CheckpointTimelineEntry>> {
        let events = self.session_events(session_id).await?;
        Ok(events
            .into_iter()
            .filter_map(|event| match event.event {
                SessionEventKind::CheckpointCreated { checkpoint }
                    if checkpoint.session_id == *session_id =>
                {
                    Some(CheckpointTimelineEntry { checkpoint })
                }
                _ => None,
            })
            .collect())
    }

    pub async fn lookup_checkpoint(
        &self,
        ctx: &ToolExecutionContext,
        checkpoint_id: &CheckpointId,
    ) -> ToolResult<CheckpointRecord> {
        let (session_id, _) = Self::require_attached_runtime(ctx)?;
        let timeline = self.load_timeline(&session_id).await?;
        let (_, checkpoint) =
            Self::resolve_checkpoint_reference(&timeline, checkpoint_id.as_str())?;
        Ok(checkpoint.clone())
    }

    fn resolve_checkpoint_reference<'a>(
        timeline: &'a [CheckpointTimelineEntry],
        checkpoint_ref: &str,
    ) -> ToolResult<(usize, &'a CheckpointRecord)> {
        if let Some((index, entry)) = timeline
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.checkpoint.checkpoint_id.as_str() == checkpoint_ref)
        {
            return Ok((index, &entry.checkpoint));
        }

        let matches = timeline
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry
                    .checkpoint
                    .checkpoint_id
                    .as_str()
                    .starts_with(checkpoint_ref)
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Err(ToolError::invalid(format!(
                "unknown checkpoint id or prefix: {checkpoint_ref}"
            ))),
            [(index, entry)] => Ok((*index, &entry.checkpoint)),
            _ => Err(ToolError::invalid(format!(
                "ambiguous checkpoint prefix {checkpoint_ref}: {}",
                matches
                    .iter()
                    .take(6)
                    .map(|(_, entry)| entry.checkpoint.checkpoint_id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    async fn current_turn_boundary(
        &self,
        session_id: &SessionId,
        turn_id: Option<&TurnId>,
    ) -> ToolResult<(Option<MessageId>, Option<MessageId>)> {
        let Some(turn_id) = turn_id else {
            return Ok((None, None));
        };
        let events = self.session_events(session_id).await?;
        let visible_messages = visible_transcript(&events);
        let mut message_turns = BTreeMap::<MessageId, (Option<TurnId>, MessageRole)>::new();
        for event in events {
            match event.event {
                SessionEventKind::TranscriptMessage { message } => {
                    message_turns.insert(
                        message.message_id.clone(),
                        (event.turn_id.clone(), message.role),
                    );
                }
                SessionEventKind::TranscriptMessagePatched {
                    message_id,
                    message,
                } => {
                    message_turns.insert(message_id, (event.turn_id.clone(), message.role));
                }
                SessionEventKind::TranscriptMessageRemoved { message_id } => {
                    message_turns.remove(&message_id);
                }
                _ => {}
            }
        }

        // Checkpoints restore code to a pre-mutation boundary, so the matching
        // conversation anchor should resolve against the current visible window
        // instead of raw append-only transcript order.
        let turn_messages = visible_messages
            .into_iter()
            .filter(|message| {
                message_turns.get(&message.message_id).is_some_and(
                    |(event_turn_id, _): &(Option<TurnId>, MessageRole)| {
                        event_turn_id.as_ref() == Some(turn_id)
                    },
                )
            })
            .collect::<Vec<_>>();
        let Some(rollback_message_id) = turn_messages
            .first()
            .map(|message| message.message_id.clone())
        else {
            return Ok((None, None));
        };
        let request_side_end = turn_messages
            .iter()
            .position(|message| matches!(message.role, MessageRole::Assistant | MessageRole::Tool))
            .unwrap_or(turn_messages.len());
        let prompt_message_id = (0..request_side_end)
            .rev()
            .find(|index| turn_messages[*index].role == MessageRole::User)
            .map(|index| turn_messages[index].message_id.clone());
        Ok((Some(rollback_message_id), prompt_message_id))
    }

    async fn append_checkpoint(
        &self,
        ctx: &ToolExecutionContext,
        origin: CheckpointOrigin,
        summary: String,
        changed_files: Vec<CheckpointFileMutation>,
    ) -> ToolResult<CheckpointRecord> {
        let (session_id, agent_session_id) = Self::require_attached_runtime(ctx)?;
        let (rollback_message_id, prompt_message_id) = self
            .current_turn_boundary(&session_id, ctx.turn_id.as_ref())
            .await?;
        let scope = if rollback_message_id.is_some() {
            CheckpointScope::Both
        } else {
            CheckpointScope::Code
        };
        let checkpoint = CheckpointRecord {
            checkpoint_id: CheckpointId::from(format!("checkpoint_{}", new_opaque_id())),
            session_id: session_id.clone(),
            agent_session_id: agent_session_id.clone(),
            scope,
            origin,
            summary,
            created_at_unix_s: unix_timestamp_s(),
            rollback_message_id,
            prompt_message_id,
            changed_files: changed_files
                .into_iter()
                .map(|change| CheckpointFileRecord {
                    requested_path: change.requested_path,
                    resolved_path: change.resolved_path,
                    before_text: change.before_text,
                    after_text: change.after_text,
                })
                .collect(),
        };
        self.store
            .append(SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                ctx.turn_id.clone(),
                None,
                SessionEventKind::CheckpointCreated {
                    checkpoint: checkpoint.clone(),
                },
            ))
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        Ok(checkpoint)
    }

    fn desired_state_for_checkpoint(
        timeline: &[CheckpointTimelineEntry],
        checkpoint_index: usize,
    ) -> BTreeMap<PathBuf, Option<String>> {
        let all_paths = timeline
            .iter()
            .flat_map(|entry| entry.checkpoint.changed_files.iter())
            .map(|file| file.resolved_path.clone())
            .collect::<BTreeSet<_>>();
        let mut desired = BTreeMap::new();

        for path in all_paths {
            // Checkpoints name the state before their mutation. To reconstruct
            // that boundary, replay the last earlier mutation if one exists;
            // otherwise fall back to the first later mutation's `before_text`.
            let latest_before_target = timeline
                .iter()
                .take(checkpoint_index)
                .flat_map(|entry| entry.checkpoint.changed_files.iter())
                .filter(|file| file.resolved_path == path)
                .last();
            let first_from_target = timeline
                .iter()
                .skip(checkpoint_index)
                .flat_map(|entry| entry.checkpoint.changed_files.iter())
                .find(|file| file.resolved_path == path);
            let content = latest_before_target
                .map(|file| file.after_text.clone())
                .or_else(|| first_from_target.map(|file| file.before_text.clone()));
            if latest_before_target.is_some() || first_from_target.is_some() {
                desired.insert(path, content.flatten());
            }
        }

        desired
    }

    fn display_path(ctx: &ToolExecutionContext, path: &Path) -> String {
        path.strip_prefix(ctx.worktree_root())
            .or_else(|_| path.strip_prefix(&ctx.workspace_root))
            .map_or_else(
                |_| path.display().to_string(),
                |relative| relative.display().to_string(),
            )
    }
}

#[async_trait]
impl CheckpointHandler for SessionCheckpointManager {
    async fn record_mutation(
        &self,
        ctx: &ToolExecutionContext,
        request: CheckpointMutationRequest,
    ) -> ToolResult<CheckpointRecord> {
        if request.changed_files.is_empty() {
            return Err(ToolError::invalid(
                "checkpoint recording requires at least one changed file",
            ));
        }
        let tool_name = ctx
            .tool_name
            .clone()
            .unwrap_or_else(|| ToolName::from("unknown_tool"));
        self.append_checkpoint(
            ctx,
            CheckpointOrigin::FileTool { tool_name },
            request.summary,
            request.changed_files,
        )
        .await
    }

    async fn list_checkpoints(
        &self,
        ctx: &ToolExecutionContext,
    ) -> ToolResult<Vec<CheckpointRecord>> {
        let (session_id, _) = Self::require_attached_runtime(ctx)?;
        let mut checkpoints = self
            .load_timeline(&session_id)
            .await?
            .into_iter()
            .map(|entry| entry.checkpoint)
            .collect::<Vec<_>>();
        checkpoints.sort_by(|left, right| {
            right
                .created_at_unix_s
                .cmp(&left.created_at_unix_s)
                .then_with(|| left.checkpoint_id.cmp(&right.checkpoint_id))
        });
        Ok(checkpoints)
    }

    async fn restore_checkpoint(
        &self,
        ctx: &ToolExecutionContext,
        checkpoint_id: &CheckpointId,
        mode: CheckpointRestoreMode,
    ) -> ToolResult<CheckpointRestoreRecord> {
        let (session_id, _) = Self::require_attached_runtime(ctx)?;
        let timeline = self.load_timeline(&session_id).await?;
        let (checkpoint_index, target_checkpoint) =
            Self::resolve_checkpoint_reference(&timeline, checkpoint_id.as_str())?;
        let mut changed_files = Vec::new();
        let mut restored_files = Vec::new();
        if matches!(
            mode,
            CheckpointRestoreMode::CodeOnly | CheckpointRestoreMode::Both
        ) {
            let desired = Self::desired_state_for_checkpoint(&timeline, checkpoint_index);
            for (path, target_content) in desired {
                ctx.assert_path_write_allowed(&path)?;
                let current_content = load_optional_text_file(&path).await?;
                if current_content == target_content {
                    continue;
                }
                commit_text_file(&path, target_content.as_deref()).await?;
                restored_files.push(Self::display_path(ctx, &path));
                changed_files.push(CheckpointFileMutation {
                    requested_path: Self::display_path(ctx, &path),
                    resolved_path: path,
                    before_text: current_content,
                    after_text: target_content,
                });
            }
        }

        let restore_checkpoint_id = if changed_files.is_empty() {
            None
        } else {
            Some(
                self.append_checkpoint(
                    ctx,
                    CheckpointOrigin::Restore {
                        restored_from: target_checkpoint.checkpoint_id.clone(),
                        restore_mode: mode,
                    },
                    format!("Restored code to {}", target_checkpoint.checkpoint_id),
                    changed_files,
                )
                .await?
                .checkpoint_id,
            )
        };

        Ok(CheckpointRestoreRecord {
            restored_from: target_checkpoint.checkpoint_id.clone(),
            restore_mode: mode,
            restored_file_count: restored_files.len(),
            restored_files,
            restore_checkpoint_id,
            rollback_message_id: target_checkpoint.rollback_message_id.clone(),
            prompt_message_id: target_checkpoint.prompt_message_id.clone(),
        })
    }
}

fn unix_timestamp_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

#[cfg(test)]
mod tests {
    use super::SessionCheckpointManager;
    use agent::tools::{
        CheckpointFileMutation, CheckpointHandler, CheckpointMutationRequest, ToolExecutionContext,
    };
    use agent::types::{
        AgentSessionId, CheckpointRestoreMode, Message, SessionEventEnvelope, SessionEventKind,
        SessionId, TurnId,
    };
    use std::sync::Arc;
    use store::{EventSink, InMemorySessionStore};
    use tempfile::tempdir;

    #[tokio::test]
    async fn restore_checkpoint_recovers_pre_mutation_file_state() {
        let store = Arc::new(InMemorySessionStore::new());
        let manager = SessionCheckpointManager::new(store.clone());
        let session_id = SessionId::from("session_1");
        let agent_session_id = AgentSessionId::from("agent_1");
        let turn_id = TurnId::from("turn_1");
        let workspace = tempdir().unwrap();
        let file_path = workspace.path().join("sample.txt");

        store
            .append(SessionEventEnvelope::new(
                session_id.clone(),
                agent_session_id.clone(),
                Some(turn_id.clone()),
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("edit sample"),
                },
            ))
            .await
            .unwrap();

        let ctx = ToolExecutionContext {
            workspace_root: workspace.path().to_path_buf(),
            worktree_root: Some(workspace.path().to_path_buf()),
            session_id: Some(session_id),
            agent_session_id: Some(agent_session_id),
            turn_id: Some(turn_id),
            tool_name: Some("write".into()),
            ..Default::default()
        };

        manager
            .record_mutation(
                &ctx,
                CheckpointMutationRequest {
                    summary: "Created sample.txt".to_string(),
                    changed_files: vec![CheckpointFileMutation {
                        requested_path: "sample.txt".to_string(),
                        resolved_path: file_path.clone(),
                        before_text: None,
                        after_text: Some("created\n".to_string()),
                    }],
                },
            )
            .await
            .unwrap();
        tokio::fs::write(&file_path, "created\n").await.unwrap();

        manager
            .record_mutation(
                &ctx,
                CheckpointMutationRequest {
                    summary: "Updated sample.txt".to_string(),
                    changed_files: vec![CheckpointFileMutation {
                        requested_path: "sample.txt".to_string(),
                        resolved_path: file_path.clone(),
                        before_text: Some("created\n".to_string()),
                        after_text: Some("updated\n".to_string()),
                    }],
                },
            )
            .await
            .unwrap();
        tokio::fs::write(&file_path, "updated\n").await.unwrap();

        let checkpoints = manager.list_checkpoints(&ctx).await.unwrap();
        let first_checkpoint = checkpoints
            .iter()
            .find(|checkpoint| checkpoint.summary == "Created sample.txt")
            .unwrap();
        let restored = manager
            .restore_checkpoint(
                &ctx,
                &first_checkpoint.checkpoint_id,
                CheckpointRestoreMode::CodeOnly,
            )
            .await
            .unwrap();

        assert_eq!(restored.restored_file_count, 1);
        assert!(!file_path.exists());
        assert!(restored.restore_checkpoint_id.is_some());
    }
}
