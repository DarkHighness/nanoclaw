use crate::backend::session_catalog;
use crate::backend::session_history::{self, LoadedSession, SessionExportArtifact};
use crate::backend::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, LoadedMcpPrompt, LoadedMcpResource,
    McpPromptSummary, McpResourceSummary, McpServerSummary, SessionEvent, SessionEventObserver,
    SessionEventStream, StartupDiagnosticsSnapshot, list_mcp_prompts, list_mcp_resources,
    list_mcp_servers, load_mcp_prompt, load_mcp_resource,
};
use agent::mcp::ConnectedMcpServer;
use agent::runtime::Result as RuntimeResult;
use agent::{AgentRuntime, RuntimeCommand, Skill};
use anyhow::Result;
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

/// The backend session owns runtime state so frontends can speak to a stable
/// host contract instead of sharing `AgentRuntime` directly.
#[derive(Clone)]
pub(crate) struct CodeAgentSession {
    runtime: Arc<AsyncMutex<AgentRuntime>>,
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
        self.set_active_agent_session_ref(runtime.agent_session_id().to_string());
        Ok(())
    }

    pub(crate) async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        let mut runtime = self.runtime.lock().await;
        let mut observer = SessionEventObserver::new(self.events.clone());
        let compacted = runtime
            .compact_now_with_observer(notes, &mut observer)
            .await?;
        self.set_active_agent_session_ref(runtime.agent_session_id().to_string());
        Ok(compacted)
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

    pub(crate) async fn load_session(&self, session_ref: &str) -> Result<LoadedSession> {
        session_history::load_session(&self.store, session_ref).await
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

    pub(crate) async fn resume_status(
        &self,
        agent_session_ref: &str,
    ) -> Result<crate::backend::AgentSessionResumeStatus> {
        let agent_sessions = self.list_agent_sessions(None).await?;
        let active_agent_session_ref = self.startup_snapshot().root_agent_session_id;
        session_catalog::resolve_agent_session_resume_status(
            &agent_sessions,
            agent_session_ref,
            &active_agent_session_ref,
        )
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

    fn set_active_agent_session_ref(&self, agent_session_ref: String) {
        self.startup.write().unwrap().root_agent_session_id = agent_session_ref;
    }
}
