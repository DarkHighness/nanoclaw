use agent::runtime::{Result as RuntimeResult, RuntimeObserver};
use agent::{AgentRuntime, RuntimeCommand, Skill};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/// The backend session owns runtime state so frontends can speak to a stable
/// host contract instead of sharing `AgentRuntime` directly.
#[derive(Clone)]
pub(crate) struct CodeAgentSession {
    runtime: Arc<AsyncMutex<AgentRuntime>>,
    workspace_root: PathBuf,
    provider_label: String,
    model: String,
    summary_model: String,
    memory_model: String,
    tool_names: Vec<String>,
    skills: Vec<Skill>,
}

impl CodeAgentSession {
    pub(crate) fn new(
        runtime: AgentRuntime,
        workspace_root: PathBuf,
        provider_label: String,
        model: String,
        summary_model: String,
        memory_model: String,
        skills: Vec<Skill>,
    ) -> Self {
        let tool_names = runtime.tool_registry_names();
        Self {
            runtime: Arc::new(AsyncMutex::new(runtime)),
            workspace_root,
            provider_label,
            model,
            summary_model,
            memory_model,
            tool_names,
            skills,
        }
    }

    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub(crate) fn provider_label(&self) -> &str {
        &self.provider_label
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn summary_model(&self) -> &str {
        &self.summary_model
    }

    pub(crate) fn memory_model(&self) -> &str {
        &self.memory_model
    }

    pub(crate) fn tool_names(&self) -> &[String] {
        &self.tool_names
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
