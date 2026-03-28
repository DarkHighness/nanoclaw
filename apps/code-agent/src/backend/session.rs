use crate::backend::run_history::{self, LoadedRun, RunExportArtifact};
use agent::runtime::{Result as RuntimeResult, RuntimeObserver};
use agent::{AgentRuntime, RuntimeCommand, Skill};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use store::{RunSearchResult, RunStore, RunSummary};
use tokio::sync::Mutex as AsyncMutex;

/// This snapshot is the frontend-facing startup contract. It keeps stable host
/// facts separate from the mutable runtime handle so new frontends can render
/// the same session metadata without reconstructing boot logic locally.
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionStartupSnapshot {
    pub(crate) workspace_name: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) provider_label: String,
    pub(crate) model: String,
    pub(crate) summary_model: String,
    pub(crate) memory_model: String,
    pub(crate) tool_names: Vec<String>,
    pub(crate) skill_names: Vec<String>,
    pub(crate) store_label: String,
    pub(crate) store_warning: Option<String>,
    pub(crate) stored_run_count: usize,
    pub(crate) sandbox_summary: String,
}

/// The backend session owns runtime state so frontends can speak to a stable
/// host contract instead of sharing `AgentRuntime` directly.
#[derive(Clone)]
pub(crate) struct CodeAgentSession {
    runtime: Arc<AsyncMutex<AgentRuntime>>,
    store: Arc<dyn RunStore>,
    workspace_root: PathBuf,
    startup: Arc<RwLock<SessionStartupSnapshot>>,
    skills: Arc<Vec<Skill>>,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        store: Arc<dyn RunStore>,
        startup: SessionStartupSnapshot,
        skills: Vec<Skill>,
    ) -> Self {
        let workspace_root = startup.workspace_root.clone();
        Self {
            runtime: Arc::new(AsyncMutex::new(runtime)),
            store,
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

    pub(crate) async fn end_session(&self, reason: Option<String>) -> RuntimeResult<()> {
        let mut runtime = self.runtime.lock().await;
        runtime.end_session(reason).await
    }

    pub(crate) async fn apply_control_with_observer<O>(
        &self,
        command: RuntimeCommand,
        observer: &mut O,
    ) -> Result<()>
    where
        O: RuntimeObserver,
    {
        let mut runtime = self.runtime.lock().await;
        runtime
            .apply_control_with_observer(command, observer)
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from)
    }

    pub(crate) async fn steer(&self, message: String, reason: Option<String>) -> RuntimeResult<()> {
        let mut runtime = self.runtime.lock().await;
        runtime.steer(message, reason).await
    }

    pub(crate) async fn compact_now(&self, notes: Option<String>) -> RuntimeResult<bool> {
        let mut runtime = self.runtime.lock().await;
        runtime.compact_now(notes).await
    }

    pub(crate) async fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let runs = run_history::list_runs(&self.store).await?;
        self.set_stored_run_count(runs.len());
        Ok(runs)
    }

    pub(crate) async fn search_runs(&self, query: &str) -> Result<Vec<RunSearchResult>> {
        run_history::search_runs(&self.store, query).await
    }

    pub(crate) async fn load_run(&self, run_ref: &str) -> Result<LoadedRun> {
        run_history::load_run(&self.store, run_ref).await
    }

    pub(crate) async fn export_run_events(
        &self,
        run_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<RunExportArtifact> {
        run_history::export_run_events(
            &self.store,
            self.workspace_root(),
            run_ref,
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn export_run_transcript(
        &self,
        run_ref: &str,
        relative_or_absolute: &str,
    ) -> Result<RunExportArtifact> {
        run_history::export_run_transcript(
            &self.store,
            self.workspace_root(),
            run_ref,
            relative_or_absolute,
        )
        .await
    }

    pub(crate) async fn refresh_stored_run_count(&self) -> Result<usize> {
        let count = run_history::list_runs(&self.store).await?.len();
        self.set_stored_run_count(count);
        Ok(count)
    }

    fn set_stored_run_count(&self, count: usize) {
        self.startup.write().unwrap().stored_run_count = count;
    }
}
