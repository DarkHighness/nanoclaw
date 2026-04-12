use crate::backend::SessionEventStream;
use crate::ui::SessionEvent;
use agent::tools::{
    CronCreateRequest, CronManager, CronTaskTemplate, Result as ToolResult, SubagentParentContext,
    TaskManager, ToolError,
};
use agent::types::{
    AgentSessionId, AgentTaskSpec, CronId, CronScheduleRecord, CronStatus, CronSummaryRecord,
    SessionEventEnvelope, SessionEventKind, SessionId, TaskId, TaskOrigin, TaskStatus,
    new_opaque_id,
};
use async_trait::async_trait;
use code_agent_contracts::ui::SessionNotificationSource;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use store::SessionStore;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

struct SessionCron {
    parent: SubagentParentContext,
    summary: Mutex<CronSummaryRecord>,
    task_template: CronTaskTemplate,
    wake: Notify,
    run_loop: Mutex<Option<JoinHandle<()>>>,
}

impl SessionCron {
    fn summary(&self) -> CronSummaryRecord {
        self.summary.lock().expect("cron summary lock").clone()
    }

    fn install_run_loop(&self, handle: JoinHandle<()>) {
        *self.run_loop.lock().expect("cron run loop lock") = Some(handle);
    }

    fn abort_run_loop(&self) {
        if let Some(handle) = self.run_loop.lock().expect("cron run loop lock").take() {
            handle.abort();
        }
        self.wake.notify_waiters();
    }
}

#[derive(Clone)]
pub struct SessionCronManager {
    store: Arc<dyn SessionStore>,
    events: SessionEventStream,
    task_manager: Arc<dyn TaskManager>,
    crons: Arc<Mutex<BTreeMap<CronId, Arc<SessionCron>>>>,
}

impl SessionCronManager {
    #[must_use]
    pub fn new(
        store: Arc<dyn SessionStore>,
        events: SessionEventStream,
        task_manager: Arc<dyn TaskManager>,
    ) -> Self {
        Self {
            store,
            events,
            task_manager,
            crons: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn require_parent_session(
        parent: &SubagentParentContext,
    ) -> ToolResult<(SessionId, AgentSessionId)> {
        let session_id = parent.session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("cron tools require an attached runtime session")
        })?;
        let agent_session_id = parent.agent_session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("cron tools require an attached runtime agent session")
        })?;
        Ok((session_id, agent_session_id))
    }

    fn insert_cron(&self, state: Arc<SessionCron>) {
        self.crons
            .lock()
            .expect("cron registry lock")
            .insert(state.summary().cron_id.clone(), state);
    }

    fn cron_state(&self, cron_id: &CronId) -> Option<Arc<SessionCron>> {
        self.crons
            .lock()
            .expect("cron registry lock")
            .get(cron_id)
            .cloned()
    }

    fn list_cron_summaries(&self) -> Vec<CronSummaryRecord> {
        let mut crons = self
            .crons
            .lock()
            .expect("cron registry lock")
            .values()
            .map(|state| state.summary())
            .collect::<Vec<_>>();
        crons.sort_by(|left, right| {
            cron_sort_key(left)
                .cmp(&cron_sort_key(right))
                .then_with(|| left.created_at_unix_s.cmp(&right.created_at_unix_s))
                .then_with(|| left.cron_id.cmp(&right.cron_id))
        });
        crons
    }

    async fn append_notification(
        &self,
        parent: &SubagentParentContext,
        message: String,
    ) -> ToolResult<()> {
        let (session_id, agent_session_id) = Self::require_parent_session(parent)?;
        self.store
            .append(SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                parent.turn_id.clone(),
                None,
                SessionEventKind::Notification {
                    source: "automation".to_string(),
                    message: message.clone(),
                },
            ))
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.events.publish(SessionEvent::Notification {
            source: SessionNotificationSource::Other("automation".to_string()),
            message,
        });
        Ok(())
    }

    async fn run_schedule(&self, cron_id: CronId) {
        loop {
            let Some(state) = self.cron_state(&cron_id) else {
                return;
            };
            let current = state.summary();
            if current.status.is_terminal() {
                return;
            }
            let wait_seconds = wait_seconds_until_next_run(&current);
            if wait_seconds > 0 {
                tokio::select! {
                    _ = sleep(Duration::from_secs(wait_seconds)) => {}
                    _ = state.wake.notified() => continue,
                }
            }
            let should_continue = self.fire_schedule(state).await;
            if !should_continue {
                break;
            }
        }
    }

    async fn fire_schedule(&self, state: Arc<SessionCron>) -> bool {
        let current = state.summary();
        if current.status.is_terminal() {
            return false;
        }
        if state.summary().status.is_terminal() {
            return false;
        }

        let next_run_index = current.run_count.saturating_add(1);
        let task = task_from_template(&state.task_template, next_run_index);
        match self
            .task_manager
            .create_task(state.parent.clone(), task.clone(), TaskStatus::Open)
            .await
        {
            Ok(record) => {
                let message = format!(
                    "automation {} queued task {}",
                    current.cron_id, record.summary.task_id
                );
                let _ = self.append_notification(&state.parent, message).await;
                let keep_running = {
                    let mut summary = state.summary.lock().expect("cron summary lock");
                    update_summary_after_run(&mut summary, &record.summary.task_id);
                    !summary.status.is_terminal()
                };
                keep_running
            }
            Err(error) => {
                let _ = self
                    .append_notification(
                        &state.parent,
                        format!("automation {} failed: {}", current.cron_id, error),
                    )
                    .await;
                state.summary.lock().expect("cron summary lock").status = CronStatus::Failed;
                false
            }
        }
    }
}

#[async_trait]
impl CronManager for SessionCronManager {
    async fn create_schedule(
        &self,
        parent: SubagentParentContext,
        request: CronCreateRequest,
    ) -> ToolResult<CronSummaryRecord> {
        let (session_id, agent_session_id) = Self::require_parent_session(&parent)?;
        let created_at_unix_s = unix_timestamp_s();
        let summary = CronSummaryRecord {
            cron_id: CronId::from(format!("cron_{}", new_opaque_id())),
            session_id,
            agent_session_id,
            parent_agent_id: parent.parent_agent_id.clone(),
            latest_task_id: None,
            role: request.task_template.role.clone(),
            prompt_summary: request.task_template.summary.clone(),
            status: CronStatus::Scheduled,
            schedule: schedule_record_from_input(&request.schedule, created_at_unix_s),
            created_at_unix_s,
            last_run_at_unix_s: None,
            run_count: 0,
        };
        let state = Arc::new(SessionCron {
            parent,
            summary: Mutex::new(summary.clone()),
            task_template: request.task_template,
            wake: Notify::new(),
            run_loop: Mutex::new(None),
        });
        self.insert_cron(state.clone());
        let manager = self.clone();
        let cron_id = summary.cron_id.clone();
        let handle = tokio::spawn(async move {
            manager.run_schedule(cron_id).await;
        });
        state.install_run_loop(handle);
        Ok(summary)
    }

    async fn list_schedules(
        &self,
        parent: SubagentParentContext,
    ) -> ToolResult<Vec<CronSummaryRecord>> {
        let _ = Self::require_parent_session(&parent)?;
        Ok(self.list_cron_summaries())
    }

    async fn delete_schedule(
        &self,
        parent: SubagentParentContext,
        cron_id: &CronId,
    ) -> ToolResult<CronSummaryRecord> {
        let _ = Self::require_parent_session(&parent)?;
        let state = self
            .cron_state(cron_id)
            .ok_or_else(|| ToolError::invalid(format!("unknown automation {cron_id}")))?;
        let (summary, changed) = {
            let mut summary = state.summary.lock().expect("cron summary lock");
            let changed = !summary.status.is_terminal();
            if !summary.status.is_terminal() {
                summary.status = CronStatus::Cancelled;
            }
            (summary.clone(), changed)
        };
        if changed {
            state.abort_run_loop();
            let _ = self
                .append_notification(
                    &state.parent,
                    format!("automation {} cancelled", summary.cron_id),
                )
                .await;
        }
        Ok(summary)
    }
}

fn schedule_record_from_input(
    schedule: &agent::tools::CronScheduleInput,
    now_unix_s: u64,
) -> CronScheduleRecord {
    match schedule {
        agent::tools::CronScheduleInput::OnceAfter { delay_seconds } => CronScheduleRecord::Once {
            run_at_unix_s: now_unix_s.saturating_add(*delay_seconds),
        },
        agent::tools::CronScheduleInput::EverySeconds {
            interval_seconds,
            start_after_seconds,
            max_runs,
        } => CronScheduleRecord::Recurring {
            interval_seconds: *interval_seconds,
            next_run_unix_s: now_unix_s
                .saturating_add(start_after_seconds.unwrap_or(*interval_seconds)),
            max_runs: *max_runs,
        },
    }
}

fn wait_seconds_until_next_run(summary: &CronSummaryRecord) -> u64 {
    let now = unix_timestamp_s();
    match &summary.schedule {
        CronScheduleRecord::Once { run_at_unix_s } => run_at_unix_s.saturating_sub(now),
        CronScheduleRecord::Recurring {
            next_run_unix_s, ..
        } => next_run_unix_s.saturating_sub(now),
    }
}

fn update_summary_after_run(summary: &mut CronSummaryRecord, task_id: &TaskId) {
    let now = unix_timestamp_s();
    summary.last_run_at_unix_s = Some(now);
    summary.latest_task_id = Some(task_id.clone());
    summary.run_count = summary.run_count.saturating_add(1);
    match &mut summary.schedule {
        CronScheduleRecord::Once { .. } => {
            summary.status = CronStatus::Completed;
        }
        CronScheduleRecord::Recurring {
            interval_seconds,
            next_run_unix_s,
            max_runs,
        } => {
            if max_runs.is_some_and(|max_runs| summary.run_count >= max_runs) {
                summary.status = CronStatus::Completed;
            } else {
                *next_run_unix_s = now.saturating_add(*interval_seconds);
            }
        }
    }
}

fn task_from_template(template: &CronTaskTemplate, run_index: u32) -> AgentTaskSpec {
    AgentTaskSpec {
        task_id: TaskId::from(match template.task_id_prefix.as_deref() {
            Some(prefix) => format!("{prefix}_run_{run_index}"),
            None => format!("task_{}", new_opaque_id()),
        }),
        role: template.role.clone(),
        prompt: template.prompt.clone(),
        origin: TaskOrigin::AutomationBacked,
        steer: template.steer.clone(),
        allowed_tools: template.allowed_tools.clone(),
        requested_write_set: template.requested_write_set.clone(),
        dependency_ids: Vec::new(),
        timeout_seconds: template.timeout_seconds,
    }
}

fn cron_sort_key(summary: &CronSummaryRecord) -> (u8, u64) {
    if summary.status.is_terminal() {
        return match &summary.schedule {
            CronScheduleRecord::Recurring {
                next_run_unix_s, ..
            } => (1, *next_run_unix_s),
            CronScheduleRecord::Once { run_at_unix_s } => (1, *run_at_unix_s),
        };
    }
    match &summary.schedule {
        CronScheduleRecord::Recurring {
            next_run_unix_s, ..
        } => (0, *next_run_unix_s),
        CronScheduleRecord::Once { run_at_unix_s } => (0, *run_at_unix_s),
    }
}

fn unix_timestamp_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::SessionEventStream;
    use crate::ui::SessionEvent;
    use agent::tools::CronScheduleInput;
    use agent::{TaskRecord, TaskSummaryRecord};
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;
    use store::{InMemorySessionStore, SessionStore};

    #[derive(Default)]
    struct RecordingTaskManager {
        created: StdMutex<Vec<TaskRecord>>,
    }

    #[async_trait]
    impl TaskManager for RecordingTaskManager {
        async fn create_task(
            &self,
            parent: SubagentParentContext,
            task: AgentTaskSpec,
            status: TaskStatus,
        ) -> ToolResult<TaskRecord> {
            let record = TaskRecord {
                summary: TaskSummaryRecord {
                    task_id: task.task_id.clone(),
                    session_id: parent
                        .session_id
                        .unwrap_or_else(|| SessionId::from("session_1")),
                    agent_session_id: parent
                        .agent_session_id
                        .unwrap_or_else(|| AgentSessionId::from("agent_session_1")),
                    role: task.role.clone(),
                    origin: task.origin,
                    status,
                    parent_agent_id: parent.parent_agent_id,
                    child_agent_id: None,
                    summary: Some(task.prompt.clone()),
                    worktree_id: parent.active_worktree_id,
                    worktree_root: parent.worktree_root,
                },
                spec: task,
                claimed_files: Vec::new(),
                result: None,
                error: None,
            };
            self.created.lock().unwrap().push(record.clone());
            Ok(record)
        }

        async fn get_task(
            &self,
            _parent: SubagentParentContext,
            _task_id: &TaskId,
        ) -> ToolResult<TaskRecord> {
            Err(ToolError::invalid_state("unused in test"))
        }

        async fn list_tasks(
            &self,
            _parent: SubagentParentContext,
            _include_closed: bool,
        ) -> ToolResult<Vec<TaskSummaryRecord>> {
            Ok(Vec::new())
        }

        async fn update_task(
            &self,
            _parent: SubagentParentContext,
            _task_id: TaskId,
            _status: Option<TaskStatus>,
            _summary: Option<String>,
        ) -> ToolResult<TaskRecord> {
            Err(ToolError::invalid_state("unused in test"))
        }

        async fn stop_task(
            &self,
            _parent: SubagentParentContext,
            _task_id: TaskId,
            _reason: Option<String>,
        ) -> ToolResult<TaskRecord> {
            Err(ToolError::invalid_state("unused in test"))
        }
    }

    fn parent() -> SubagentParentContext {
        SubagentParentContext {
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cron_manager_materializes_immediate_automation_task() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let task_manager = Arc::new(RecordingTaskManager::default());
        let manager = SessionCronManager::new(store.clone(), events.clone(), task_manager.clone());

        let summary = manager
            .create_schedule(
                parent(),
                CronCreateRequest {
                    schedule: CronScheduleInput::OnceAfter { delay_seconds: 0 },
                    task_template: CronTaskTemplate {
                        role: "reviewer".to_string(),
                        prompt: "Review the latest task queue".to_string(),
                        steer: None,
                        allowed_tools: Vec::new(),
                        requested_write_set: Vec::new(),
                        timeout_seconds: None,
                        summary: "Review the latest task queue".to_string(),
                        task_id_prefix: Some("nightly_review".to_string()),
                    },
                },
            )
            .await
            .unwrap();

        assert_eq!(summary.status, CronStatus::Scheduled);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let created = task_manager.created.lock().unwrap().clone();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].summary.origin, TaskOrigin::AutomationBacked);
        assert_eq!(created[0].summary.task_id.as_str(), "nightly_review_run_1");

        let drained = events.drain();
        assert!(drained.iter().any(|event| matches!(
            event,
            SessionEvent::Notification { message, .. }
                if message.contains("queued task nightly_review_run_1")
        )));

        let persisted = store.events(&SessionId::from("session_1")).await.unwrap();
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::Notification { source, message }
                if source == "automation" && message.contains("nightly_review_run_1")
        )));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cron_manager_lists_session_automations_in_next_run_order() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let task_manager = Arc::new(RecordingTaskManager::default());
        let manager = SessionCronManager::new(store, events, task_manager);

        let later = manager
            .create_schedule(
                parent(),
                CronCreateRequest {
                    schedule: CronScheduleInput::OnceAfter { delay_seconds: 120 },
                    task_template: CronTaskTemplate {
                        role: "reviewer".to_string(),
                        prompt: "Review the weekly triage queue".to_string(),
                        steer: None,
                        allowed_tools: Vec::new(),
                        requested_write_set: Vec::new(),
                        timeout_seconds: None,
                        summary: "Review the weekly triage queue".to_string(),
                        task_id_prefix: Some("triage".to_string()),
                    },
                },
            )
            .await
            .unwrap();
        let earlier = manager
            .create_schedule(
                parent(),
                CronCreateRequest {
                    schedule: CronScheduleInput::EverySeconds {
                        interval_seconds: 60,
                        start_after_seconds: Some(30),
                        max_runs: Some(2),
                    },
                    task_template: CronTaskTemplate {
                        role: "general-purpose".to_string(),
                        prompt: "Summarize new issue labels".to_string(),
                        steer: None,
                        allowed_tools: Vec::new(),
                        requested_write_set: Vec::new(),
                        timeout_seconds: None,
                        summary: "Summarize new issue labels".to_string(),
                        task_id_prefix: None,
                    },
                },
            )
            .await
            .unwrap();

        let listed = manager.list_schedules(parent()).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].cron_id, earlier.cron_id);
        assert_eq!(listed[1].cron_id, later.cron_id);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cron_manager_delete_cancels_future_runs_and_keeps_tombstone() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let task_manager = Arc::new(RecordingTaskManager::default());
        let manager = SessionCronManager::new(store.clone(), events.clone(), task_manager.clone());

        let summary = manager
            .create_schedule(
                parent(),
                CronCreateRequest {
                    schedule: CronScheduleInput::OnceAfter {
                        delay_seconds: 3600,
                    },
                    task_template: CronTaskTemplate {
                        role: "reviewer".to_string(),
                        prompt: "Review the delayed automation queue".to_string(),
                        steer: None,
                        allowed_tools: Vec::new(),
                        requested_write_set: Vec::new(),
                        timeout_seconds: None,
                        summary: "Review the delayed automation queue".to_string(),
                        task_id_prefix: Some("delayed_review".to_string()),
                    },
                },
            )
            .await
            .unwrap();

        let deleted = manager
            .delete_schedule(parent(), &summary.cron_id)
            .await
            .unwrap();
        assert_eq!(deleted.status, CronStatus::Cancelled);

        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(task_manager.created.lock().unwrap().is_empty());

        let listed = manager.list_schedules(parent()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].cron_id, summary.cron_id);
        assert_eq!(listed[0].status, CronStatus::Cancelled);

        let drained = events.drain();
        assert!(drained.iter().any(|event| matches!(
            event,
            SessionEvent::Notification { message, .. }
                if message.contains("cancelled")
        )));

        let persisted = store.events(&SessionId::from("session_1")).await.unwrap();
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::Notification { source, message }
                if source == "automation" && message.contains("cancelled")
        )));
    }
}
