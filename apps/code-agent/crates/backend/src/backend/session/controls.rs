use super::CodeAgentSession;
use crate::frontend_contract::pending_control_summary;
use crate::interaction::{PendingControlKind, PendingControlSummary};
use agent::RuntimeCommand;
use agent::runtime::RuntimeCommandId;
use agent::types::{Message, SubmittedPromptSnapshot};
use anyhow::Result;

impl CodeAgentSession {
    pub fn queued_command_count(&self) -> usize {
        self.control_plane.len()
    }

    pub fn pending_controls(&self) -> Vec<PendingControlSummary> {
        self.control_plane
            .snapshot()
            .into_iter()
            .map(|queued| pending_control_summary(queued.id.to_string(), queued.command))
            .collect()
    }

    pub fn update_pending_control(
        &self,
        control_ref: &str,
        content: &str,
    ) -> Result<PendingControlSummary> {
        let controls = self.pending_controls();
        let current = resolve_pending_control_reference(&controls, control_ref)?;
        let updated = self
            .control_plane
            .update(
                &RuntimeCommandId::from(current.id.clone()),
                match current.kind {
                    PendingControlKind::Prompt => RuntimeCommand::Prompt {
                        message: Message::user(content.to_string()),
                        submitted_prompt: Some(SubmittedPromptSnapshot::from_text(
                            content.to_string(),
                        )),
                    },
                    PendingControlKind::Steer => RuntimeCommand::Steer {
                        message: content.to_string(),
                        reason: current.reason.clone(),
                    },
                },
            )
            .ok_or_else(|| anyhow::anyhow!("pending control update failed for {control_ref}"))?;
        Ok(pending_control_summary(
            updated.id.to_string(),
            updated.command,
        ))
    }

    pub fn remove_pending_control(&self, control_ref: &str) -> Result<PendingControlSummary> {
        let controls = self.pending_controls();
        let current = resolve_pending_control_reference(&controls, control_ref)?;
        let removed = self
            .control_plane
            .remove(&RuntimeCommandId::from(current.id.clone()))
            .ok_or_else(|| anyhow::anyhow!("pending control removal failed for {control_ref}"))?;
        Ok(pending_control_summary(
            removed.id.to_string(),
            removed.command,
        ))
    }

    pub async fn clear_queued_commands(&self) -> usize {
        let mut runtime = self.runtime.lock().await;
        let cleared = runtime.clear_pending_runtime_commands_for_host();
        self.sync_runtime_session_refs(&runtime);
        cleared
    }

    pub async fn drain_queued_controls(&self) -> Result<bool> {
        let _turn_guard = self.begin_active_turn()?;
        let mut runtime = self.runtime.lock().await;
        let mut observer = crate::backend::SessionEventObserver::new(self.events.clone());
        // Frontends never pop queued prompts themselves. They only wake the
        // runtime at an idle edge so the runtime can drain its own queue and
        // emit one consistent event stream for every dequeued control.
        let drained = runtime
            .drain_queued_controls_with_observer(&mut observer)
            .await
            .map_err(anyhow::Error::from)?;
        let snapshot = self.latest_compaction_working_snapshot(&runtime, &observer);
        let refresh_context = self.session_memory_refresh_context(&runtime, &observer);
        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);
        self.store_side_question_context(side_question_context);
        self.sync_session_memory_after_runtime_activity(refresh_context, snapshot)
            .await;
        Ok(drained)
    }

    pub fn schedule_runtime_steer(
        &self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> Result<String> {
        // Active-turn steer must bypass the host prompt queue so the runtime can
        // merge it only at its own safe points between model/tool phases.
        let queued = self.control_plane.push_steer(message, reason);
        Ok(queued.id.to_string())
    }

    pub fn take_pending_steers(&self) -> Result<Vec<PendingControlSummary>> {
        let steers = self
            .pending_controls()
            .into_iter()
            .filter(|control| control.kind == PendingControlKind::Steer)
            .collect::<Vec<_>>();
        for steer in &steers {
            let _ = self.remove_pending_control(&steer.id)?;
        }
        Ok(steers)
    }
}

fn resolve_pending_control_reference<'a>(
    controls: &'a [PendingControlSummary],
    control_ref: &str,
) -> Result<&'a PendingControlSummary> {
    if let Some(control) = controls.iter().find(|control| control.id == control_ref) {
        return Ok(control);
    }

    let matches = controls
        .iter()
        .filter(|control| control.id.starts_with(control_ref))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(anyhow::anyhow!("unknown pending control: {control_ref}")),
        [control] => Ok(control),
        _ => Err(anyhow::anyhow!(
            "ambiguous pending control prefix {control_ref}: {}",
            matches
                .iter()
                .take(6)
                .map(|control| crate::backend::preview_id(&control.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}
