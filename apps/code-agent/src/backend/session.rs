use agent::runtime::{Result as RuntimeResult, RuntimeObserver};
use agent::{AgentRuntime, RuntimeCommand, Skill};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    startup: SessionStartupSnapshot,
    skills: Vec<Skill>,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        startup: SessionStartupSnapshot,
        skills: Vec<Skill>,
    ) -> Self {
        Self {
            runtime: Arc::new(AsyncMutex::new(runtime)),
            startup,
            skills,
        }
    }

    pub(crate) fn workspace_root(&self) -> &Path {
        &self.startup.workspace_root
    }

    pub(crate) fn startup_snapshot(&self) -> &SessionStartupSnapshot {
        &self.startup
    }

    pub(crate) fn skills(&self) -> &[Skill] {
        &self.skills
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
}
