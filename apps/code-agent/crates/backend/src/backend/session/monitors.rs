use super::*;

impl CodeAgentSession {
    pub async fn list_live_monitors(
        &self,
        include_closed: bool,
    ) -> Result<Vec<LiveMonitorSummary>> {
        let monitors = self
            .monitor_manager
            .list_monitors(self.monitor_parent_context(), include_closed)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(monitors.into_iter().map(live_monitor_summary).collect())
    }

    pub async fn stop_live_monitor(
        &self,
        monitor_ref: &str,
        reason: Option<String>,
    ) -> Result<LiveMonitorControlOutcome> {
        let monitor_id = self.resolve_monitor_reference(monitor_ref).await?;
        let before = self
            .monitor_manager
            .list_monitors(self.monitor_parent_context(), true)
            .await
            .map_err(anyhow::Error::from)?;
        let action = if before
            .iter()
            .find(|summary| summary.monitor_id == monitor_id)
            .is_some_and(|summary| summary.status.is_terminal())
        {
            LiveMonitorControlAction::AlreadyTerminal
        } else {
            LiveMonitorControlAction::Stopped
        };
        let monitor = self
            .monitor_manager
            .stop_monitor(self.monitor_parent_context(), monitor_id, reason)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(LiveMonitorControlOutcome {
            requested_ref: monitor_ref.to_string(),
            action,
            monitor: live_monitor_summary(monitor),
        })
    }

    fn monitor_parent_context(&self) -> MonitorRuntimeContext {
        let startup = self.startup_snapshot();
        MonitorRuntimeContext {
            session_id: Some(SessionId::from(startup.active_session_ref)),
            agent_session_id: Some(AgentSessionId::from(startup.root_agent_session_id)),
            turn_id: None,
            parent_agent_id: None,
            task_id: None,
        }
    }

    async fn resolve_monitor_reference(
        &self,
        monitor_ref: &str,
    ) -> Result<agent::types::MonitorId> {
        let monitor_ref = monitor_ref.trim();
        if monitor_ref.is_empty() {
            return Err(anyhow::anyhow!("monitor id cannot be empty"));
        }

        let monitors = self.list_live_monitors(true).await?;
        if let Some(summary) = monitors
            .iter()
            .find(|summary| summary.monitor_id.as_str() == monitor_ref)
        {
            return Ok(summary.monitor_id.clone());
        }

        let matches = monitors
            .iter()
            .filter(|summary| summary.monitor_id.as_str().starts_with(monitor_ref))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Err(anyhow::anyhow!(
                "unknown monitor id or prefix: {monitor_ref}"
            )),
            [summary] => Ok(summary.monitor_id.clone()),
            _ => Err(anyhow::anyhow!(
                "ambiguous monitor prefix {monitor_ref}: {}",
                matches
                    .iter()
                    .take(6)
                    .map(|summary| summary.monitor_id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

fn live_monitor_summary(summary: agent::types::MonitorSummaryRecord) -> LiveMonitorSummary {
    LiveMonitorSummary {
        monitor_id: summary.monitor_id,
        task_id: summary.task_id,
        status: summary.status,
        command: summary.command,
        cwd: summary.cwd,
        shell: summary.shell,
        login: summary.login,
        started_at_unix_s: summary.started_at_unix_s,
        finished_at_unix_s: summary.finished_at_unix_s,
    }
}
