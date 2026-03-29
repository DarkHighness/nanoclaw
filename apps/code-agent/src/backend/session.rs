use crate::backend::session_catalog;
use crate::backend::session_history::{
    self, LoadedAgentSession, LoadedSession, SessionExportArtifact, preview_id,
};
use crate::backend::session_resume;
use crate::backend::task_history::{self, LoadedTask, PersistedTaskSummary};
use crate::backend::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, LoadedMcpPrompt, LoadedMcpResource,
    McpPromptSummary, McpResourceSummary, McpServerSummary, ResumeSupport, SessionEvent,
    SessionEventObserver, SessionEventStream, StartupDiagnosticsSnapshot, list_mcp_prompts,
    list_mcp_resources, list_mcp_servers, load_mcp_prompt, load_mcp_resource,
};
use agent::mcp::ConnectedMcpServer;
use agent::runtime::{Result as RuntimeResult, RunTurnOutcome};
use agent::tools::{SubagentExecutor, SubagentParentContext};
use agent::types::{
    AgentSessionId, AgentTaskSpec, AgentWaitMode, AgentWaitRequest, Message, SessionId,
    new_opaque_id,
};
use agent::{AgentRuntime, RuntimeCommand, Skill};
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use store::SessionStore;
use tokio::sync::Mutex as AsyncMutex;

/// This snapshot is the frontend-facing startup contract. It keeps stable host
/// facts separate from the mutable runtime handle so new frontends can render
/// the same session metadata without reconstructing boot logic locally.
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionStartupSnapshot {
    pub(crate) workspace_name: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) active_session_ref: String,
    pub(crate) root_agent_session_id: String,
    pub(crate) provider_label: String,
    pub(crate) model: String,
    pub(crate) summary_model: String,
    pub(crate) memory_model: String,
    pub(crate) tool_names: Vec<String>,
    pub(crate) skill_names: Vec<String>,
    pub(crate) store_label: String,
    pub(crate) store_warning: Option<String>,
    pub(crate) stored_session_count: usize,
    pub(crate) sandbox_summary: String,
    pub(crate) startup_diagnostics: StartupDiagnosticsSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionOperation {
    StartFresh,
    ResumeAgentSession { agent_session_ref: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionOperationAction {
    StartedFresh,
    AlreadyAttached,
    Reattached,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionOperationOutcome {
    pub(crate) action: SessionOperationAction,
    pub(crate) session_ref: String,
    pub(crate) active_agent_session_ref: String,
    pub(crate) requested_agent_session_ref: Option<String>,
    pub(crate) startup: SessionStartupSnapshot,
    pub(crate) transcript: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskSummary {
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) role: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) session_ref: String,
    pub(crate) agent_session_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskSpawnOutcome {
    pub(crate) task: LiveTaskSummary,
    pub(crate) prompt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveTaskControlAction {
    Cancelled,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskControlOutcome {
    pub(crate) requested_ref: String,
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) action: LiveTaskControlAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveTaskMessageAction {
    Sent,
    AlreadyTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskMessageOutcome {
    pub(crate) requested_ref: String,
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) action: LiveTaskMessageAction,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveTaskWaitOutcome {
    pub(crate) requested_ref: String,
    pub(crate) agent_id: String,
    pub(crate) task_id: String,
    pub(crate) status: agent::types::AgentStatus,
    pub(crate) summary: String,
    pub(crate) claimed_files: Vec<String>,
}

/// The backend session owns runtime state so frontends can speak to a stable
/// host contract instead of sharing `AgentRuntime` directly.
#[derive(Clone)]
pub(crate) struct CodeAgentSession {
    runtime: Arc<AsyncMutex<AgentRuntime>>,
    subagent_executor: Arc<dyn SubagentExecutor>,
    store: Arc<dyn SessionStore>,
    mcp_servers: Arc<Vec<ConnectedMcpServer>>,
    approvals: ApprovalCoordinator,
    events: SessionEventStream,
    workspace_root: PathBuf,
    startup: Arc<RwLock<SessionStartupSnapshot>>,
    skills: Arc<Vec<Skill>>,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        subagent_executor: Arc<dyn SubagentExecutor>,
        store: Arc<dyn SessionStore>,
        mcp_servers: Vec<ConnectedMcpServer>,
        approvals: ApprovalCoordinator,
        events: SessionEventStream,
        startup: SessionStartupSnapshot,
        skills: Vec<Skill>,
    ) -> Self {
        let workspace_root = startup.workspace_root.clone();
        Self {
            runtime: Arc::new(AsyncMutex::new(runtime)),
            subagent_executor,
            store,
            mcp_servers: Arc::new(mcp_servers),
            approvals,
            events,
            workspace_root,
            startup: Arc::new(RwLock::new(startup)),
            skills: Arc::new(skills),
        }
    }

    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub(crate) fn startup_snapshot(&self) -> SessionStartupSnapshot {
        self.startup.read().unwrap().clone()
    }

    pub(crate) fn skills(&self) -> &[Skill] {
        self.skills.as_slice()
    }

    pub(crate) fn startup_diagnostics(&self) -> StartupDiagnosticsSnapshot {
        self.startup.read().unwrap().startup_diagnostics.clone()
    }

    pub(crate) async fn end_session(&self, reason: Option<String>) -> RuntimeResult<()> {
        let mut runtime = self.runtime.lock().await;
        runtime.end_session(reason).await
    }

    pub(crate) async fn apply_control(&self, command: RuntimeCommand) -> Result<()> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        runtime
            .apply_control_with_observer(command, &mut observer)
            .await
            .map_err(anyhow::Error::from)?;
        self.sync_runtime_session_refs(&runtime);
        Ok(())
    }

    pub(crate) async fn run_one_shot_prompt(&self, prompt: &str) -> Result<RunTurnOutcome> {
        let mut runtime = self.runtime.lock().await;
        let outcome = runtime
            .run_user_prompt(prompt)
            .await
            .map_err(anyhow::Error::from)?;
        self.sync_runtime_session_refs(&runtime);
        Ok(outcome)
    }

    pub(crate) async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        let compacted = runtime
            .compact_now_with_observer(notes, &mut observer)
            .await?;
        self.sync_runtime_session_refs(&runtime);
        Ok(compacted)
    }

    pub(crate) async fn apply_session_operation(
        &self,
        operation: SessionOperation,
    ) -> Result<SessionOperationOutcome> {
        match operation {
            SessionOperation::StartFresh => self.start_fresh_session().await,
            SessionOperation::ResumeAgentSession { agent_session_ref } => {
                self.resume_existing_agent_session(&agent_session_ref).await
            }
        }
    }

    async fn start_fresh_session(&self) -> Result<SessionOperationOutcome> {
        let (session_ref, agent_session_ref) = {
            let mut runtime = self.runtime.lock().await;
            runtime
                .start_new_session()
                .await
                .map_err(anyhow::Error::from)?;
            (
                runtime.session_id().to_string(),
                runtime.agent_session_id().to_string(),
            )
        };
        self.set_runtime_session_refs(session_ref, agent_session_ref);
        self.refresh_stored_session_count().await?;
        Ok(self
            .build_session_operation_outcome(SessionOperationAction::StartedFresh, None)
            .await)
    }

    pub(crate) fn approval_prompt(&self) -> Option<ApprovalPrompt> {
        self.approvals.snapshot()
    }

    pub(crate) fn resolve_approval(&self, decision: ApprovalDecision) -> bool {
        self.approvals.resolve(decision)
    }

    pub(crate) fn drain_events(&self) -> Vec<SessionEvent> {
        self.events.drain()
    }

    pub(crate) async fn list_sessions(
        &self,
    ) -> Result<Vec<crate::backend::PersistedSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        self.set_stored_session_count(sessions.len());
        let active_session_ref = self.startup_snapshot().active_session_ref;
        Ok(sessions
            .iter()
            .map(|summary| session_catalog::persisted_session_summary(summary, &active_session_ref))
            .collect())
    }

    pub(crate) async fn search_sessions(
        &self,
        query: &str,
    ) -> Result<Vec<crate::backend::PersistedSessionSearchMatch>> {
        let matches = session_history::search_sessions(&self.store, query).await?;
        let active_session_ref = self.startup_snapshot().active_session_ref;
        Ok(matches
            .iter()
            .map(|result| {
                session_catalog::persisted_session_search_match(result, &active_session_ref)
            })
            .collect())
    }

    pub(crate) async fn list_agent_sessions(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<crate::backend::PersistedAgentSessionSummary>> {
        let sessions = session_history::list_sessions(&self.store).await?;
        let filtered_session_id = session_ref
            .map(|session_ref| session_history::resolve_session_reference(&sessions, session_ref))
            .transpose()?;
        let active_agent_session_ref = self.startup_snapshot().root_agent_session_id;
        let mut agent_sessions = Vec::new();
        for summary in sessions.into_iter().filter(|summary| {
            filtered_session_id
                .as_ref()
                .is_none_or(|session_id| summary.session_id == *session_id)
        }) {
            let events = self.store.events(&summary.session_id).await?;
            agent_sessions.extend(session_catalog::persisted_agent_session_summaries(
                summary.session_id.as_str(),
                &events,
                &active_agent_session_ref,
            ));
        }
        agent_sessions.sort_by(|left, right| {
            right
                .last_timestamp_ms
                .cmp(&left.last_timestamp_ms)
                .then_with(|| left.agent_session_ref.cmp(&right.agent_session_ref))
        });
        Ok(agent_sessions)
    }

    pub(crate) async fn list_tasks(
        &self,
        session_ref: Option<&str>,
    ) -> Result<Vec<PersistedTaskSummary>> {
        task_history::list_tasks(&self.store, session_ref).await
    }

    pub(crate) async fn list_live_tasks(&self) -> Result<Vec<LiveTaskSummary>> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(live_task_summaries(&handles))
    }

    pub(crate) async fn spawn_live_task(
        &self,
        role: &str,
        prompt: &str,
    ) -> Result<LiveTaskSpawnOutcome> {
        let role = role.trim();
        if role.is_empty() {
            return Err(anyhow::anyhow!("live task role cannot be empty"));
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(anyhow::anyhow!("live task prompt cannot be empty"));
        }

        let parent = self.live_task_parent_context();
        let task = AgentTaskSpec {
            task_id: new_live_task_id(),
            role: role.to_string(),
            prompt: prompt.to_string(),
            steer: None,
            allowed_tools: Vec::new(),
            requested_write_set: Vec::new(),
            dependency_ids: Vec::new(),
            timeout_seconds: None,
        };
        let mut handles = self
            .subagent_executor
            .spawn(parent, vec![task])
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let handle = handles
            .pop()
            .ok_or_else(|| anyhow::anyhow!("live task spawn returned no child handle"))?;
        Ok(LiveTaskSpawnOutcome {
            task: live_task_summary(&handle),
            prompt: prompt.to_string(),
        })
    }

    pub(crate) async fn send_live_task(
        &self,
        task_or_agent_ref: &str,
        message: &str,
    ) -> Result<LiveTaskMessageOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let updated = self
            .subagent_executor
            .send(
                parent,
                handle.agent_id.clone(),
                "steer".to_string(),
                json!({ "text": message }),
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(LiveTaskMessageOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: updated.agent_id.to_string(),
            task_id: updated.task_id,
            status: updated.status.clone(),
            action: if handle.status.is_terminal() {
                LiveTaskMessageAction::AlreadyTerminal
            } else {
                LiveTaskMessageAction::Sent
            },
            message: message.to_string(),
        })
    }

    pub(crate) async fn wait_live_task(
        &self,
        task_or_agent_ref: &str,
    ) -> Result<LiveTaskWaitOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let response = self
            .subagent_executor
            .wait(
                parent,
                AgentWaitRequest {
                    agent_ids: vec![handle.agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let completed = response
            .completed
            .into_iter()
            .find(|candidate| candidate.agent_id == handle.agent_id)
            .unwrap_or(handle);
        let result = response
            .results
            .into_iter()
            .find(|candidate| candidate.agent_id.as_str() == completed.agent_id.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing live task result for {}", completed.task_id))?;
        Ok(LiveTaskWaitOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: completed.agent_id.to_string(),
            task_id: completed.task_id,
            status: completed.status,
            summary: result.summary,
            claimed_files: result.claimed_files,
        })
    }

    pub(crate) async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        session_history::load_session(&self.store, session_ref).await
    }

    pub(crate) async fn load_agent_session(
        &self,
        agent_session_ref: &str,
    ) -> Result<LoadedAgentSession> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        let summary =
            session_catalog::resolve_agent_session_reference(&agent_sessions, agent_session_ref)?
                .clone();
        session_history::load_agent_session(&self.store, summary).await
    }

    pub(crate) async fn load_task(&self, task_ref: &str) -> Result<LoadedTask> {
        let tasks = self.list_tasks(None).await?;
        let summary = task_history::resolve_task_reference(&tasks, task_ref)?.clone();
        task_history::load_task(&self.store, summary).await
    }

    pub(crate) async fn cancel_live_task(
        &self,
        task_or_agent_ref: &str,
        reason: Option<String>,
    ) -> Result<LiveTaskControlOutcome> {
        let parent = self.live_task_parent_context();
        let handles = self
            .subagent_executor
            .list(parent.clone())
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let handle = resolve_live_task_reference(&handles, task_or_agent_ref)?.clone();
        let updated = self
            .subagent_executor
            .cancel(parent, handle.agent_id.clone(), reason)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(LiveTaskControlOutcome {
            requested_ref: task_or_agent_ref.to_string(),
            agent_id: updated.agent_id.to_string(),
            task_id: updated.task_id,
            status: updated.status.clone(),
            action: if handle.status.is_terminal() {
                LiveTaskControlAction::AlreadyTerminal
            } else {
                LiveTaskControlAction::Cancelled
            },
        })
    }

    pub(crate) async fn export_session(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        session_history::export_session_events(
            &self.store,
            self.workspace_root(),
            session_ref,
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn export_session_transcript(
        &self,
        session_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<SessionExportArtifact> {
        session_history::export_session_transcript(
            &self.store,
            self.workspace_root(),
            session_ref,
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn refresh_stored_session_count(&self) -> Result<usize> {
        let count = session_history::list_sessions(&self.store).await?.len();
        self.set_stored_session_count(count);
        Ok(count)
    }

    async fn resume_existing_agent_session(
        &self,
        agent_session_ref: &str,
    ) -> Result<SessionOperationOutcome> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        let summary =
            session_catalog::resolve_agent_session_reference(&agent_sessions, agent_session_ref)?;
        match &summary.resume_support {
            ResumeSupport::AttachedToActiveRuntime => {
                return Ok(self
                    .build_session_operation_outcome(
                        SessionOperationAction::AlreadyAttached,
                        Some(summary.agent_session_ref.clone()),
                    )
                    .await);
            }
            ResumeSupport::NotYetSupported { reason } => {
                return Err(anyhow::anyhow!(reason.clone()));
            }
            ResumeSupport::Reattachable => {}
        }

        let session_id = SessionId::from(summary.session_ref.clone());
        let target_agent_session_id = AgentSessionId::from(summary.agent_session_ref.clone());
        let events = self.store.events(&session_id).await?;
        let runtime_session =
            session_resume::reconstruct_runtime_session(&events, &target_agent_session_id)?;
        let (active_session_ref, active_agent_session_ref) = {
            let mut runtime = self.runtime.lock().await;
            runtime
                .resume_session(runtime_session)
                .await
                .map_err(anyhow::Error::from)?;
            (
                runtime.session_id().to_string(),
                runtime.agent_session_id().to_string(),
            )
        };
        self.set_runtime_session_refs(active_session_ref.clone(), active_agent_session_ref.clone());
        self.refresh_stored_session_count().await?;
        Ok(self
            .build_session_operation_outcome(
                SessionOperationAction::Reattached,
                Some(summary.agent_session_ref.clone()),
            )
            .await)
    }

    pub(crate) async fn active_visible_transcript(&self) -> Vec<Message> {
        self.runtime.lock().await.visible_transcript_snapshot()
    }

    pub(crate) async fn list_mcp_servers(&self) -> Vec<McpServerSummary> {
        list_mcp_servers(self.mcp_servers.as_slice())
    }

    pub(crate) async fn list_mcp_prompts(&self) -> Vec<McpPromptSummary> {
        list_mcp_prompts(self.mcp_servers.as_slice())
    }

    pub(crate) async fn list_mcp_resources(&self) -> Vec<McpResourceSummary> {
        list_mcp_resources(self.mcp_servers.as_slice())
    }

    pub(crate) async fn load_mcp_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
    ) -> Result<LoadedMcpPrompt> {
        load_mcp_prompt(self.mcp_servers.as_slice(), server_name, prompt_name).await
    }

    pub(crate) async fn load_mcp_resource(
        &self,
        server_name: &str,
        uri: &str,
    ) -> Result<LoadedMcpResource> {
        load_mcp_resource(self.mcp_servers.as_slice(), server_name, uri).await
    }

    fn set_stored_session_count(&self, count: usize) {
        self.startup.write().unwrap().stored_session_count = count;
    }

    fn sync_runtime_session_refs(&self, runtime: &AgentRuntime) {
        self.set_runtime_session_refs(
            runtime.session_id().to_string(),
            runtime.agent_session_id().to_string(),
        );
    }

    fn set_runtime_session_refs(&self, session_ref: String, agent_session_ref: String) {
        let mut startup = self.startup.write().unwrap();
        startup.active_session_ref = session_ref;
        startup.root_agent_session_id = agent_session_ref;
    }

    // Host-initiated live task operations should still append their lifecycle
    // into the active top-level session, otherwise operator-side spawn/send/
    // cancel actions disappear from durable task history.
    fn live_task_parent_context(&self) -> SubagentParentContext {
        let startup = self.startup_snapshot();
        SubagentParentContext {
            session_id: Some(SessionId::from(startup.active_session_ref)),
            agent_session_id: Some(AgentSessionId::from(startup.root_agent_session_id)),
            turn_id: None,
            parent_agent_id: None,
        }
    }

    async fn build_session_operation_outcome(
        &self,
        action: SessionOperationAction,
        requested_agent_session_ref: Option<String>,
    ) -> SessionOperationOutcome {
        let startup = self.startup_snapshot();
        let transcript = self.active_visible_transcript().await;
        SessionOperationOutcome {
            action,
            session_ref: startup.active_session_ref.clone(),
            active_agent_session_ref: startup.root_agent_session_id.clone(),
            requested_agent_session_ref,
            startup,
            transcript,
        }
    }
}

fn new_live_task_id() -> String {
    format!("task_{}", new_opaque_id())
}

fn live_task_summary(handle: &agent::types::AgentHandle) -> LiveTaskSummary {
    LiveTaskSummary {
        agent_id: handle.agent_id.to_string(),
        task_id: handle.task_id.clone(),
        role: handle.role.clone(),
        status: handle.status.clone(),
        session_ref: handle.session_id.to_string(),
        agent_session_ref: handle.agent_session_id.to_string(),
    }
}

fn live_task_summaries(handles: &[agent::types::AgentHandle]) -> Vec<LiveTaskSummary> {
    let mut summaries = handles.iter().map(live_task_summary).collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.task_id
            .cmp(&right.task_id)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    summaries
}

fn resolve_live_task_reference<'a>(
    handles: &'a [agent::types::AgentHandle],
    task_or_agent_ref: &str,
) -> Result<&'a agent::types::AgentHandle> {
    if let Some(handle) = handles
        .iter()
        .find(|handle| handle.task_id == task_or_agent_ref)
    {
        return Ok(handle);
    }
    if let Some(handle) = handles
        .iter()
        .find(|handle| handle.agent_id.as_str() == task_or_agent_ref)
    {
        return Ok(handle);
    }

    let task_matches = handles
        .iter()
        .filter(|handle| handle.task_id.starts_with(task_or_agent_ref))
        .collect::<Vec<_>>();
    match task_matches.as_slice() {
        [handle] => return Ok(handle),
        [] => {}
        _ => {
            return Err(anyhow::anyhow!(
                "ambiguous live task prefix {task_or_agent_ref}: {}",
                task_matches
                    .iter()
                    .take(6)
                    .map(|handle| handle.task_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let agent_matches = handles
        .iter()
        .filter(|handle| handle.agent_id.as_str().starts_with(task_or_agent_ref))
        .collect::<Vec<_>>();
    match agent_matches.as_slice() {
        [] => Err(anyhow::anyhow!(
            "unknown live task or agent id: {task_or_agent_ref}"
        )),
        [handle] => Ok(handle),
        _ => Err(anyhow::anyhow!(
            "ambiguous live agent prefix {task_or_agent_ref}: {}",
            agent_matches
                .iter()
                .take(6)
                .map(|handle| preview_id(handle.agent_id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodeAgentSession, SessionOperation, SessionOperationAction, SessionStartupSnapshot,
    };
    use crate::backend::{ApprovalCoordinator, SessionEventStream, StartupDiagnosticsSnapshot};
    use agent::runtime::{HookRunner, ModelBackend, Result as RuntimeResult};
    use agent::tools::{
        Result as ToolResult, SubagentExecutor, SubagentParentContext, ToolError,
        ToolExecutionContext,
    };
    use agent::types::{
        AgentHandle, AgentId, AgentResultEnvelope, AgentStatus, AgentTaskSpec, AgentWaitRequest,
        AgentWaitResponse, ModelEvent, ModelRequest, SessionEventKind, SessionId,
    };
    use agent::{AgentRuntimeBuilder, RuntimeCommand, Skill};
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use store::{InMemorySessionStore, SessionStore};

    struct NeverBackend;

    #[async_trait]
    impl ModelBackend for NeverBackend {
        async fn stream_turn(
            &self,
            _request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            unreachable!("session-start tests never execute model turns")
        }
    }

    struct StreamingTextBackend;

    #[async_trait]
    impl ModelBackend for StreamingTextBackend {
        async fn stream_turn(
            &self,
            _request: ModelRequest,
        ) -> RuntimeResult<BoxStream<'static, RuntimeResult<ModelEvent>>> {
            Ok(stream::iter(vec![Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                usage: None,
                reasoning: Vec::new(),
            })])
            .boxed())
        }
    }

    struct NoopSubagentExecutor;

    #[async_trait]
    impl SubagentExecutor for NoopSubagentExecutor {
        async fn spawn(
            &self,
            _parent: SubagentParentContext,
            _tasks: Vec<agent::types::AgentTaskSpec>,
        ) -> ToolResult<Vec<AgentHandle>> {
            Err(ToolError::invalid_state(
                "test executor does not support spawn",
            ))
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _channel: String,
            _payload: Value,
        ) -> ToolResult<AgentHandle> {
            Err(ToolError::invalid_state(
                "test executor does not support send",
            ))
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            _request: AgentWaitRequest,
        ) -> ToolResult<AgentWaitResponse> {
            Ok(AgentWaitResponse {
                completed: Vec::new(),
                pending: Vec::new(),
                results: Vec::<AgentResultEnvelope>::new(),
            })
        }

        async fn list(&self, _parent: SubagentParentContext) -> ToolResult<Vec<AgentHandle>> {
            Ok(Vec::new())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            _agent_id: AgentId,
            _reason: Option<String>,
        ) -> ToolResult<AgentHandle> {
            Err(ToolError::invalid_state(
                "test executor does not support cancel",
            ))
        }
    }

    struct RecordingSubagentExecutor {
        handles: Mutex<Vec<AgentHandle>>,
        spawned_tasks: Mutex<Vec<AgentTaskSpec>>,
        spawn_parents: Mutex<Vec<SubagentParentContext>>,
        sent_messages: Mutex<Vec<(AgentId, String, Value)>>,
        wait_response: Mutex<Option<AgentWaitResponse>>,
    }

    impl RecordingSubagentExecutor {
        fn new(handles: Vec<AgentHandle>) -> Self {
            Self {
                handles: Mutex::new(handles),
                spawned_tasks: Mutex::new(Vec::new()),
                spawn_parents: Mutex::new(Vec::new()),
                sent_messages: Mutex::new(Vec::new()),
                wait_response: Mutex::new(None),
            }
        }

        fn with_wait_response(handles: Vec<AgentHandle>, wait_response: AgentWaitResponse) -> Self {
            Self {
                handles: Mutex::new(handles),
                spawned_tasks: Mutex::new(Vec::new()),
                spawn_parents: Mutex::new(Vec::new()),
                sent_messages: Mutex::new(Vec::new()),
                wait_response: Mutex::new(Some(wait_response)),
            }
        }
    }

    #[async_trait]
    impl SubagentExecutor for RecordingSubagentExecutor {
        async fn spawn(
            &self,
            parent: SubagentParentContext,
            tasks: Vec<agent::types::AgentTaskSpec>,
        ) -> ToolResult<Vec<AgentHandle>> {
            self.spawn_parents.lock().unwrap().push(parent);
            self.spawned_tasks.lock().unwrap().extend(tasks.clone());
            let mut handles = self.handles.lock().unwrap();
            let mut spawned = Vec::with_capacity(tasks.len());
            for task in tasks {
                let handle = AgentHandle {
                    agent_id: AgentId::from(format!("agent-{}", task.task_id)),
                    parent_agent_id: None,
                    session_id: SessionId::from(format!("session-{}", task.task_id)),
                    agent_session_id: agent::types::AgentSessionId::from(format!(
                        "agent-session-{}",
                        task.task_id
                    )),
                    task_id: task.task_id.clone(),
                    role: task.role.clone(),
                    status: AgentStatus::Queued,
                };
                handles.push(handle.clone());
                spawned.push(handle);
            }
            Ok(spawned)
        }

        async fn send(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
            channel: String,
            payload: Value,
        ) -> ToolResult<AgentHandle> {
            let handle = self
                .handles
                .lock()
                .unwrap()
                .iter()
                .find(|handle| handle.agent_id == agent_id)
                .cloned()
                .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
            self.sent_messages
                .lock()
                .unwrap()
                .push((agent_id, channel, payload));
            Ok(handle)
        }

        async fn wait(
            &self,
            _parent: SubagentParentContext,
            _request: AgentWaitRequest,
        ) -> ToolResult<AgentWaitResponse> {
            Ok(self
                .wait_response
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(AgentWaitResponse {
                    completed: Vec::new(),
                    pending: Vec::new(),
                    results: Vec::new(),
                }))
        }

        async fn list(&self, _parent: SubagentParentContext) -> ToolResult<Vec<AgentHandle>> {
            Ok(self.handles.lock().unwrap().clone())
        }

        async fn cancel(
            &self,
            _parent: SubagentParentContext,
            agent_id: AgentId,
            _reason: Option<String>,
        ) -> ToolResult<AgentHandle> {
            let mut handles = self.handles.lock().unwrap();
            let handle = handles
                .iter_mut()
                .find(|handle| handle.agent_id == agent_id)
                .ok_or_else(|| ToolError::invalid_state("unknown agent"))?;
            if !handle.status.is_terminal() {
                handle.status = agent::types::AgentStatus::Cancelled;
            }
            Ok(handle.clone())
        }
    }

    fn startup_snapshot(workspace_root: &std::path::Path) -> SessionStartupSnapshot {
        SessionStartupSnapshot {
            workspace_name: "workspace".to_string(),
            workspace_root: workspace_root.to_path_buf(),
            active_session_ref: "session-active".to_string(),
            root_agent_session_id: "agent-session-active".to_string(),
            provider_label: "provider".to_string(),
            model: "model".to_string(),
            summary_model: "summary".to_string(),
            memory_model: "memory".to_string(),
            tool_names: Vec::new(),
            skill_names: Vec::new(),
            store_label: "memory".to_string(),
            store_warning: None,
            stored_session_count: 0,
            sandbox_summary: "workspace-write".to_string(),
            startup_diagnostics: StartupDiagnosticsSnapshot::default(),
        }
    }

    fn sample_handle(task_id: &str, agent_id: &str, status: AgentStatus) -> AgentHandle {
        AgentHandle {
            agent_id: AgentId::from(agent_id),
            parent_agent_id: None,
            session_id: SessionId::from("session-1"),
            agent_session_id: agent::types::AgentSessionId::from(format!(
                "agent-session-{task_id}"
            )),
            task_id: task_id.to_string(),
            role: "worker".to_string(),
            status,
        }
    }

    #[tokio::test]
    async fn start_new_session_refreshes_backend_snapshot_refs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let initial_session_ref = runtime.session_id().to_string();
        let initial_agent_session_ref = runtime.agent_session_id().to_string();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = initial_session_ref.clone();
        startup.root_agent_session_id = initial_agent_session_ref.clone();
        let session = CodeAgentSession::new(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store.clone(),
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup,
            Vec::<Skill>::new(),
        );

        let outcome = session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();

        assert_eq!(outcome.action, SessionOperationAction::StartedFresh);
        assert_ne!(outcome.startup.active_session_ref, initial_session_ref);
        assert_ne!(
            outcome.startup.root_agent_session_id,
            initial_agent_session_ref
        );
        assert_eq!(outcome.startup.stored_session_count, 1);
        assert!(outcome.transcript.is_empty());

        let new_events = store
            .events(&SessionId::from(outcome.startup.active_session_ref.clone()))
            .await
            .unwrap();
        assert!(new_events.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::SessionStart { reason }
                if reason.as_deref() == Some("operator_new_session")
        )));
    }

    #[tokio::test]
    async fn resume_agent_session_reattaches_archived_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(StreamingTextBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let original_session_ref = runtime.session_id().to_string();
        let original_agent_session_ref = runtime.agent_session_id().to_string();
        let mut startup = startup_snapshot(dir.path());
        startup.active_session_ref = original_session_ref.clone();
        startup.root_agent_session_id = original_agent_session_ref.clone();
        let session = CodeAgentSession::new(
            runtime,
            Arc::new(NoopSubagentExecutor),
            store.clone(),
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup,
            Vec::<Skill>::new(),
        );

        session
            .apply_control(RuntimeCommand::Prompt {
                prompt: "resume me".to_string(),
            })
            .await
            .unwrap();
        session
            .apply_session_operation(SessionOperation::StartFresh)
            .await
            .unwrap();

        let outcome = session
            .apply_session_operation(SessionOperation::ResumeAgentSession {
                agent_session_ref: original_agent_session_ref.clone(),
            })
            .await
            .unwrap();

        assert_eq!(outcome.action, SessionOperationAction::Reattached);
        assert_eq!(
            outcome.requested_agent_session_ref.as_deref(),
            Some(original_agent_session_ref.as_str())
        );
        assert_eq!(outcome.session_ref, original_session_ref);
        assert_ne!(outcome.active_agent_session_ref, original_agent_session_ref);
        assert_eq!(outcome.startup.active_session_ref, outcome.session_ref);
        assert_eq!(
            outcome.startup.root_agent_session_id,
            outcome.active_agent_session_ref
        );
        assert_eq!(outcome.transcript.len(), 1);
        assert_eq!(outcome.transcript[0].text_content(), "resume me");
    }

    #[tokio::test]
    async fn live_task_listing_projects_sorted_child_handles() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let executor = Arc::new(RecordingSubagentExecutor::new(vec![
            AgentHandle {
                role: "reviewer".to_string(),
                ..sample_handle("task-b", "agent-b", AgentStatus::Running)
            },
            AgentHandle {
                role: "researcher".to_string(),
                ..sample_handle("task-a", "agent-a", AgentStatus::Queued)
            },
        ]));
        let session = CodeAgentSession::new(
            runtime,
            executor,
            store,
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup_snapshot(dir.path()),
            Vec::<Skill>::new(),
        );

        let live_tasks = session.list_live_tasks().await.unwrap();

        assert_eq!(live_tasks.len(), 2);
        assert_eq!(live_tasks[0].task_id, "task-a");
        assert_eq!(live_tasks[1].task_id, "task-b");
        assert_eq!(live_tasks[0].role, "researcher");
    }

    #[tokio::test]
    async fn spawn_live_task_returns_handle_and_tracks_active_parent_context() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let startup = startup_snapshot(dir.path());
        let active_session_ref = startup.active_session_ref.clone();
        let active_agent_session_ref = startup.root_agent_session_id.clone();
        let executor = Arc::new(RecordingSubagentExecutor::new(Vec::new()));
        let session = CodeAgentSession::new(
            runtime,
            executor.clone(),
            store,
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup,
            Vec::<Skill>::new(),
        );

        let outcome = session
            .spawn_live_task("reviewer", "inspect the failing tests")
            .await
            .unwrap();

        assert_eq!(outcome.task.role, "reviewer");
        assert_eq!(outcome.task.status, AgentStatus::Queued);
        assert_eq!(outcome.prompt, "inspect the failing tests");
        assert!(outcome.task.task_id.starts_with("task_"));
        let spawned_tasks = executor.spawned_tasks.lock().unwrap();
        assert_eq!(spawned_tasks.len(), 1);
        assert_eq!(spawned_tasks[0].role, "reviewer");
        assert_eq!(spawned_tasks[0].prompt, "inspect the failing tests");
        let spawn_parents = executor.spawn_parents.lock().unwrap();
        assert_eq!(spawn_parents.len(), 1);
        assert_eq!(
            spawn_parents[0]
                .session_id
                .as_ref()
                .map(|value| value.as_str()),
            Some(active_session_ref.as_str())
        );
        assert_eq!(
            spawn_parents[0]
                .agent_session_id
                .as_ref()
                .map(|value| value.as_str()),
            Some(active_agent_session_ref.as_str())
        );
        assert!(spawn_parents[0].parent_agent_id.is_none());
    }

    #[tokio::test]
    async fn cancel_live_task_updates_backend_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let session = CodeAgentSession::new(
            runtime,
            Arc::new(RecordingSubagentExecutor::new(vec![AgentHandle {
                role: "editor".to_string(),
                ..sample_handle("task-cancel", "agent-cancel", AgentStatus::Running)
            }])),
            store,
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup_snapshot(dir.path()),
            Vec::<Skill>::new(),
        );

        let outcome = session
            .cancel_live_task("task-cancel", Some("operator_cancel".to_string()))
            .await
            .unwrap();

        assert_eq!(outcome.action, super::LiveTaskControlAction::Cancelled);
        assert_eq!(outcome.task_id, "task-cancel");
        assert_eq!(outcome.status, AgentStatus::Cancelled);
    }

    #[tokio::test]
    async fn send_live_task_routes_steer_message_to_child_agent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let executor = Arc::new(RecordingSubagentExecutor::new(vec![sample_handle(
            "task-send",
            "agent-send",
            AgentStatus::Running,
        )]));
        let session = CodeAgentSession::new(
            runtime,
            executor.clone(),
            store,
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup_snapshot(dir.path()),
            Vec::<Skill>::new(),
        );

        let outcome = session
            .send_live_task("task-send", "focus on tests")
            .await
            .unwrap();

        assert_eq!(outcome.action, super::LiveTaskMessageAction::Sent);
        assert_eq!(outcome.task_id, "task-send");
        let sent_messages = executor.sent_messages.lock().unwrap();
        assert_eq!(sent_messages.len(), 1);
        assert_eq!(sent_messages[0].0, AgentId::from("agent-send"));
        assert_eq!(sent_messages[0].1, "steer");
        assert_eq!(sent_messages[0].2["text"], "focus on tests");
    }

    #[tokio::test]
    async fn wait_live_task_returns_terminal_result_summary() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemorySessionStore::new());
        let runtime = AgentRuntimeBuilder::new(Arc::new(NeverBackend), store.clone())
            .hook_runner(Arc::new(HookRunner::default()))
            .tool_context(ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            })
            .build();
        let completed_handle = sample_handle("task-wait", "agent-wait", AgentStatus::Completed);
        let wait_response = AgentWaitResponse {
            completed: vec![completed_handle.clone()],
            pending: Vec::new(),
            results: vec![AgentResultEnvelope {
                agent_id: AgentId::from("agent-wait"),
                task_id: "task-wait".to_string(),
                status: AgentStatus::Completed,
                summary: "finished child task".to_string(),
                text: "done".to_string(),
                artifacts: Vec::new(),
                claimed_files: vec!["src/lib.rs".to_string()],
                structured_payload: None,
            }],
        };
        let session = CodeAgentSession::new(
            runtime,
            Arc::new(RecordingSubagentExecutor::with_wait_response(
                vec![sample_handle(
                    "task-wait",
                    "agent-wait",
                    AgentStatus::Running,
                )],
                wait_response,
            )),
            store,
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            startup_snapshot(dir.path()),
            Vec::<Skill>::new(),
        );

        let outcome = session.wait_live_task("task-wait").await.unwrap();

        assert_eq!(outcome.task_id, "task-wait");
        assert_eq!(outcome.status, AgentStatus::Completed);
        assert_eq!(outcome.summary, "finished child task");
        assert_eq!(outcome.claimed_files, vec!["src/lib.rs".to_string()]);
    }
}
