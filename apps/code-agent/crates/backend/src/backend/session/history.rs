use super::*;

fn history_rollback_round_from_snapshot(
    snapshot: VisibleHistoryRollbackRound,
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
        self.runtime
            .lock()
            .await
            .visible_history_rollback_rounds_snapshot()
            .into_iter()
            .filter_map(history_rollback_round_from_snapshot)
            .collect()
    }
}
