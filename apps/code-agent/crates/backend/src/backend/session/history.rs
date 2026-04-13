use super::*;
use crate::ui::{CheckpointRestoreOutcome, HistoryRollbackCheckpoint};
use agent::tools::CheckpointHandler;
use agent::types::{CheckpointRestoreMode, SessionEventKind, ToolName, TurnId, new_opaque_id};
use store::SessionStoreError;

fn history_checkpoint_from_record(
    checkpoint: &agent::types::CheckpointRecord,
) -> HistoryRollbackCheckpoint {
    HistoryRollbackCheckpoint {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        summary: checkpoint.summary.clone(),
        changed_file_count: checkpoint.changed_files.len(),
    }
}

fn history_rollback_round_from_snapshot(
    snapshot: VisibleHistoryRollbackRound,
    checkpoint: Option<HistoryRollbackCheckpoint>,
) -> Option<HistoryRollbackRound> {
    let prompt_message = snapshot
        .messages
        .iter()
        .find(|message| message.message_id == snapshot.prompt_message_id)
        .cloned()?;
    Some(HistoryRollbackRound {
        rollback_message_id: snapshot.rollback_message_id,
        prompt_message,
        round_messages: snapshot.messages,
        removed_turn_count: snapshot.removed_turn_count,
        removed_message_count: snapshot.removed_message_count,
        checkpoint,
    })
}

impl CodeAgentSession {
    pub async fn rollback_visible_history_to_message(
        &self,
        message_id: &str,
    ) -> Result<HistoryRollbackOutcome> {
        let mut runtime = self.runtime.lock().await;
        let RollbackVisibleHistoryOutcome {
            removed_message_ids,
        } = runtime
            .rollback_visible_history_to_message(message_id.into())
            .await
            .map_err(anyhow::Error::from)?;
        let transcript = runtime.visible_transcript_snapshot();
        self.sync_runtime_session_refs(&runtime);
        Ok(HistoryRollbackOutcome {
            transcript,
            removed_message_count: removed_message_ids.len(),
        })
    }

    pub async fn history_rollback_rounds(&self) -> Vec<HistoryRollbackRound> {
        let (session_id, snapshots) = {
            let runtime = self.runtime.lock().await;
            (
                runtime.session_id(),
                runtime.visible_history_rollback_rounds_snapshot(),
            )
        };
        let checkpoint_by_anchor = self
            .store
            .events(&session_id)
            .await
            .or_else(|error| match error {
                SessionStoreError::SessionNotFound(_) => Ok(Vec::new()),
                other => Err(other),
            })
            .map(|events| {
                let mut checkpoints = BTreeMap::new();
                for event in events {
                    let SessionEventKind::CheckpointCreated { checkpoint } = event.event else {
                        continue;
                    };
                    let Some(anchor) = checkpoint.rollback_message_id.clone() else {
                        continue;
                    };
                    // A rollback round should restore to the first checkpoint
                    // captured in that turn so "restore both" rewinds code to
                    // the start-of-turn boundary instead of a later mid-turn edit.
                    checkpoints
                        .entry(anchor)
                        .or_insert_with(|| history_checkpoint_from_record(&checkpoint));
                }
                checkpoints
            })
            .unwrap_or_default();

        snapshots
            .into_iter()
            .filter_map(|snapshot| {
                let checkpoint = checkpoint_by_anchor
                    .get(&snapshot.rollback_message_id)
                    .cloned();
                history_rollback_round_from_snapshot(snapshot, checkpoint)
            })
            .collect()
    }

    pub async fn restore_checkpoint(
        &self,
        checkpoint_id: &str,
        mode: CheckpointRestoreMode,
    ) -> Result<CheckpointRestoreOutcome> {
        let mut runtime = self.runtime.lock().await;
        let base_tool_context = self.session_tool_context.read().unwrap().clone();
        let session_id = runtime.session_id();
        let agent_session_id = runtime.agent_session_id();
        let turn_id = TurnId::new();
        let checkpoint_ref = agent::types::CheckpointId::from(checkpoint_id.to_string());
        let tool_context = base_tool_context
            .with_sandbox_policy(
                self.permission_grants
                    .effective_sandbox_policy(&base_tool_context.sandbox_policy())?,
            )
            .with_runtime_scope(
                session_id.clone(),
                agent_session_id,
                turn_id,
                ToolName::from("checkpoint_restore"),
                format!("host-checkpoint-restore-{}", new_opaque_id()),
            );

        if matches!(
            mode,
            CheckpointRestoreMode::ConversationOnly | CheckpointRestoreMode::Both
        ) {
            let checkpoint = self
                .checkpoint_manager
                .lookup_checkpoint(&tool_context, &checkpoint_ref)
                .await
                .map_err(anyhow::Error::from)?;
            let rollback_message_id = checkpoint.rollback_message_id.ok_or_else(|| {
                anyhow::anyhow!(
                    "checkpoint {} does not include a visible conversation boundary",
                    checkpoint.checkpoint_id
                )
            })?;
            if !runtime
                .visible_transcript_snapshot()
                .iter()
                .any(|message| message.message_id == rollback_message_id)
            {
                return Err(anyhow::anyhow!(
                    "checkpoint {} no longer maps to a visible transcript boundary",
                    checkpoint.checkpoint_id
                ));
            }
        }

        let restore = self
            .checkpoint_manager
            .restore_checkpoint(&tool_context, &checkpoint_ref, mode)
            .await
            .map_err(anyhow::Error::from)?;

        let removed_message_count = if matches!(
            mode,
            CheckpointRestoreMode::ConversationOnly | CheckpointRestoreMode::Both
        ) {
            let rollback_message_id = restore.rollback_message_id.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "checkpoint {} does not include a visible conversation boundary",
                    restore.restored_from
                )
            })?;
            runtime
                .rollback_visible_history_to_message(rollback_message_id)
                .await?
                .removed_message_ids
                .len()
        } else {
            0
        };

        let transcript = runtime.visible_transcript_snapshot();
        self.sync_runtime_session_refs(&runtime);
        Ok(CheckpointRestoreOutcome {
            restore,
            transcript,
            removed_message_count,
        })
    }
}
