use crate::backend::session_catalog;
use crate::backend::session_history::{
    self, LoadedAgentSession, LoadedSession, SessionExportArtifact,
};
use crate::backend::session_resume;
use crate::backend::{
    ApprovalCoordinator, ApprovalDecision, ApprovalPrompt, LoadedMcpPrompt, LoadedMcpResource,
    McpPromptSummary, McpResourceSummary, McpServerSummary, ResumeSupport, SessionEvent,
    SessionEventObserver, SessionEventStream, StartupDiagnosticsSnapshot, list_mcp_prompts,
    list_mcp_resources, list_mcp_servers, load_mcp_prompt, load_mcp_resource,
};
use agent::mcp::ConnectedMcpServer;
use agent::runtime::Result as RuntimeResult;
use agent::types::{AgentSessionId, Message, SessionId};
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
        self.sync_runtime_session_refs(&runtime);
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{
        CodeAgentSession, SessionOperation, SessionOperationAction, SessionStartupSnapshot,
    };
    use crate::backend::{ApprovalCoordinator, SessionEventStream, StartupDiagnosticsSnapshot};
    use agent::runtime::{HookRunner, ModelBackend, Result as RuntimeResult};
    use agent::tools::ToolExecutionContext;
    use agent::types::{ModelEvent, ModelRequest, SessionEventKind, SessionId};
    use agent::{AgentRuntimeBuilder, RuntimeCommand, Skill};
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use std::sync::Arc;
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
        let session = CodeAgentSession::new(
            runtime,
            store.clone(),
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            SessionStartupSnapshot {
                workspace_name: "workspace".to_string(),
                workspace_root: dir.path().to_path_buf(),
                active_session_ref: initial_session_ref.clone(),
                root_agent_session_id: initial_agent_session_ref.clone(),
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
            },
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
        let session = CodeAgentSession::new(
            runtime,
            store.clone(),
            Vec::new(),
            ApprovalCoordinator::default(),
            SessionEventStream::default(),
            SessionStartupSnapshot {
                workspace_name: "workspace".to_string(),
                workspace_root: dir.path().to_path_buf(),
                active_session_ref: original_session_ref.clone(),
                root_agent_session_id: original_agent_session_ref.clone(),
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
            },
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
}
